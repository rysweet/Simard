//! Outer OODA cycle implementation extracted from mod.rs (#1266).

use std::time::Instant;

use crate::error::{SimardError, SimardResult};
use crate::goal_curation::{load_goal_board, save_goal_board_with_removals};
use crate::gym_bridge::ScoreDimensions;
use crate::gym_scoring::GymSuiteScore;
use crate::memory_consolidation;
use crate::memory_consolidation::preparation_memory_operations_with_active_slugs_phased;
use crate::self_improve::{ImprovementCycle, ImprovementPhase};

use super::types::*;
use super::{
    act, check_meeting_handoffs, decide, decide_with_brain, observe, orient, orient_with_brain,
    promote_from_backlog, review_outcomes,
};

/// Run one complete OODA cycle: Observe -> Orient -> Decide -> Act -> Curate.
///
/// After dispatching actions, the cycle archives completed goals and promotes
/// the highest-scoring backlog items to fill any freed active slots. This
/// implements the meta-goal of continually seeking the best goals to pursue.
///
/// Takes `&mut OodaBridges` so that the optional session can be used for
/// `run_turn` calls during `AdvanceGoal` dispatch.
#[tracing::instrument(skip_all, fields(cycle = state.cycle_count))]
pub fn run_ooda_cycle(
    state: &mut OodaState,
    bridges: &mut OodaBridges,
    config: &OodaConfig,
) -> SimardResult<CycleReport> {
    // Install per-cycle brain-judgment task-local. Was a `thread_local!`
    // (PR #1472), but brain LLM calls drive Tokio worker threads via the
    // session adapter, so pushes landed on different OS threads than the
    // eventual `take_all()` — daemon `d69c411c52f1` cycle_2 showed
    // `planned_actions: 3` but `brain_judgments: []`.
    crate::ooda_brain::with_brain_judgment_scope(|| run_ooda_cycle_inner(state, bridges, config))
}

/// Build the OODA objective probe from the active goals.
///
/// The probe is fed to `check_triggers` (and fact/episode recall) during the
/// Prepare phase. It concatenates, per active goal:
///
///   1. the free-text `description` — drives targeted fact/episode recall, and
///   2. the goal's **slug-phrase** — `goal_slug(id)` with dashes→spaces.
///
/// The slug-phrase is exactly what `prospective_trigger_for`
/// (`src/goals/cognitive_memory_store.rs`) writes as a goal's
/// `trigger_condition`. Including it here is what lets `check_triggers` fire a
/// goal's prospective memory (#2300, root cause (b)). Before this, the probe
/// carried only free-text descriptions, which never contain the slug-derived
/// trigger substring — so `check_triggers` matched nothing and the
/// prospective subsystem was dead ("0 triggers" every OODA cycle).
fn build_objective_probe(active: &[crate::goal_curation::ActiveGoal]) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(active.len() * 2);
    for g in active {
        let description = g.description.trim();
        if !description.is_empty() {
            parts.push(description.to_string());
        }
        // Byte-identical to the written `trigger_condition`: the store path
        // uses `goal_slug(&active.id).replace('-', " ")` via
        // `active_goals_as_records` + `prospective_trigger_for`.
        let trigger_phrase = crate::goals::goal_slug(&g.id).replace('-', " ");
        if !trigger_phrase.is_empty() {
            parts.push(trigger_phrase);
        }
    }
    parts.join("; ")
}

fn run_ooda_cycle_inner(
    state: &mut OodaState,
    bridges: &mut OodaBridges,
    config: &OodaConfig,
) -> SimardResult<CycleReport> {
    crate::ooda_brain::clear_brain_judgments();

    // Budget enforcement: refuse to run if daily or weekly spend is exceeded.
    if let Ok(daily) = crate::cost_tracking::daily_summary()
        && daily.total_cost_usd >= config.daily_budget_usd
    {
        return Err(SimardError::BudgetExceeded {
            period: "daily".to_string(),
            spent: format!("${:.4}", daily.total_cost_usd),
            limit: format!("${:.2}", config.daily_budget_usd),
        });
    }
    if let Ok(weekly) = crate::cost_tracking::weekly_summary()
        && weekly.total_cost_usd >= config.weekly_budget_usd
    {
        return Err(SimardError::BudgetExceeded {
            period: "weekly".to_string(),
            spent: format!("${:.4}", weekly.total_cost_usd),
            limit: format!("${:.2}", config.weekly_budget_usd),
        });
    }

    // Only replace board if loaded one is non-empty (cold memory = keep local).
    // A `.reseed_goals` marker file forces re-seeding from DEFAULT_SEED_GOALS,
    // ignoring the stale cognitive memory snapshot.
    let reseed_marker = crate::goal_curation::simard_state_root().join(".reseed_goals");
    if reseed_marker.exists() {
        eprintln!(
            "[simard] OODA start: .reseed_goals marker found — ignoring cognitive memory board"
        );
        if let Err(e) = std::fs::remove_file(&reseed_marker) {
            eprintln!("[simard] OODA start: failed to remove .reseed_goals marker: {e}");
        }
        state.active_goals = crate::goal_curation::GoalBoard::new();
    } else if let Ok(board) = load_goal_board(&*bridges.memory)
        && !board.active.is_empty()
    {
        if let Some(reason) = board_integrity_suspect(&board) {
            eprintln!(
                "[simard] OODA start: rejecting loaded board — integrity suspect: {reason}; \
                 falling back to default seed"
            );
        } else {
            state.active_goals = board;
        }
    }

    // Sweep stale assigned_to fields against live tmux sessions.
    // Best-effort: if tmux is absent or returns no sessions, skip entirely
    // to avoid false-positive clearing in non-tmux environments.
    sweep_stale_assignments(&mut state.active_goals);

    // Seed with default goals if the board is still empty.
    let seeded = crate::goal_curation::seed_default_board(&mut state.active_goals);
    if seeded > 0 {
        eprintln!("[simard] OODA start: seeded {seeded} default goal(s)");
    }

    // Ingest meeting handoff decisions as new goals.
    let handoff_dir = crate::meeting_facilitator::default_handoff_dir();
    match check_meeting_handoffs(
        &mut state.active_goals,
        &handoff_dir,
        &crate::goal_curation::simard_state_root(),
    ) {
        Ok(n) if n > 0 => {
            eprintln!(
                "[simard] OODA start: ingested {n} goal/backlog item(s) from meeting handoff"
            );
        }
        Err(e) => {
            eprintln!("[simard] OODA start: meeting handoff check failed: {e}");
        }
        _ => {}
    }

    // --- Memory consolidation: intake at cycle start ---
    let cycle_session_id = crate::session::SessionId::from_uuid(uuid::Uuid::now_v7());
    let cycle_objective = state
        .active_goals
        .active
        .first()
        .map(|g| g.description.clone())
        .unwrap_or_else(|| "ooda-cycle".to_string());
    if let Err(e) = memory_consolidation::intake_memory_operations(
        &cycle_objective,
        &cycle_session_id,
        &*bridges.memory,
    ) {
        eprintln!("[simard] OODA consolidation: intake failed: {e}");
    }
    // Hydrate prior-session facts into working memory for cross-cycle recall.
    match memory_consolidation::consolidation_intake(
        &cycle_session_id,
        &cycle_objective,
        &*bridges.memory,
    ) {
        Ok(n) if n > 0 => {
            eprintln!("[simard] OODA consolidation: hydrated {n} prior-session facts");
        }
        Err(e) => {
            eprintln!("[simard] OODA consolidation: cross-session hydration failed: {e}");
        }
        _ => {}
    }

    // --- Resource cleanup: proactive disk/process management (issue #373) ---
    {
        use crate::cmd_cleanup::handle_cleanup;
        eprintln!("[simard] OODA cycle: running resource cleanup");
        if let Err(e) = handle_cleanup() {
            eprintln!("[simard] OODA cycle: resource cleanup had errors: {e}");
        }

        // Reap old processed handoff files to prevent indefinite disk
        // accumulation (issue #2268).
        let reap_dir = crate::meeting_facilitator::default_handoff_dir();
        match crate::ooda_loop::reap_old_handoffs(&reap_dir) {
            Ok(n) if n > 0 => {
                eprintln!("[simard] OODA cycle: reaped {n} old processed handoff file(s)");
            }
            Err(e) => {
                eprintln!("[simard] OODA cycle: handoff reap had errors: {e}");
            }
            _ => {}
        }
    }

    // De-fork Phase 2b (issue #2307): native lbug-WAL backup pruning has been
    // removed — the library backend owns its own durability and there is no
    // native `backups/` directory to prune.

    // Snapshot active goal ids before the core OODA phases run.
    // Used at the end of the cycle to detect unexpected goal disappearance
    // before persisting — see corruption guard near persist_board.
    let pre_cycle_active_ids: std::collections::HashSet<String> = state
        .active_goals
        .active
        .iter()
        .map(|g| g.id.clone())
        .collect();

    // --- Observe ---
    state.current_phase = OodaPhase::Observe;
    eprintln!("[simard] OODA cycle: entering Observe phase");
    let observation = observe(state, bridges)?;
    eprintln!("[simard] OODA cycle: Observe complete");

    // --- Prepare: gather relevant context from cognitive memory ---
    // Build an objective summary from active goals so memory retrieval is
    // targeted. Includes each goal's slug-phrase so prospective `check_triggers`
    // can fire the goal's trigger (#2300) — see `build_objective_probe`.
    let objective_summary: String = build_objective_probe(&state.active_goals.active);
    // PR-A (issue #2281): build the live `active_slugs` set from
    // `active` + `backlog` so `preparation_memory_operations_with_active_slugs`
    // can drop stale `goal-store:record` facts whose slug is no longer
    // on the board. Using the live board (not snapshot facts) prevents
    // a stale snapshot from resurrecting a deleted slug into recall.
    let active_slugs: std::collections::HashSet<&str> = state
        .active_goals
        .active
        .iter()
        .map(|g| g.id.as_str())
        .chain(state.active_goals.backlog.iter().map(|b| b.id.as_str()))
        .collect();
    // Issue #2308 follow-up: mirror the live board's active goals into
    // prospective memory BEFORE preparation so `check_triggers` can fire them
    // this cycle. The daemon persists goals via the GoalBoard snapshot path,
    // not `CognitiveMemoryGoalStore::put`, so without this reconcile no
    // prospects exist and preparation reports "0 triggers" forever. The
    // reconcile is idempotent and fire-once-safe (it re-establishes a pending
    // prospect for every still-active goal each cycle). Failures are logged but
    // non-fatal — a reconcile hiccup must not abort the cycle.
    if let Err(e) =
        crate::goals::reconcile_board_prospectives(&state.active_goals, &*bridges.memory)
    {
        eprintln!("[simard] OODA cycle: board-sourced prospective reconcile failed: {e}");
    }
    // Reuse cycle_session_id established above — the entire cycle is one logical session.
    // Issue #2329: gather `relevant_facts` with ranked recall biased toward the
    // Observe phase (recency-favoring) so the freshest declarative state surfaces
    // first each cycle.
    let ctx = preparation_memory_operations_with_active_slugs_phased(
        &objective_summary,
        &cycle_session_id,
        &*bridges.memory,
        Some(&active_slugs),
        crate::ooda_loop::phase_weights::weights_for_phase(OodaPhase::Observe),
    )?;
    eprintln!(
        "[simard] OODA cycle: prepared context ({} facts, {} triggers, {} procedures, {} episodes)",
        ctx.relevant_facts.len(),
        ctx.triggered_prospectives.len(),
        ctx.recalled_procedures.len(),
        ctx.episodic_recall.len(),
    );
    state.prepared_context = Some(ctx);

    // --- Orient ---
    state.current_phase = OodaPhase::Orient;
    eprintln!("[simard] OODA cycle: entering Orient phase");
    let priorities = match bridges.orient_brain.as_ref() {
        Some(brain) => orient_with_brain(
            &observation,
            &state.active_goals,
            &state.goal_failure_counts,
            brain.as_ref(),
        )?,
        None => orient(
            &observation,
            &state.active_goals,
            &state.goal_failure_counts,
        )?,
    };
    eprintln!(
        "[simard] OODA cycle: Orient complete ({} priorities)",
        priorities.len()
    );

    // --- Decide ---
    state.current_phase = OodaPhase::Decide;
    eprintln!("[simard] OODA cycle: entering Decide phase");
    let mut planned_actions = match bridges.decide_brain.as_ref() {
        Some(brain) => decide_with_brain(&priorities, config, brain.as_ref())?,
        None => decide(&priorities, config)?,
    };
    eprintln!(
        "[simard] OODA cycle: Decide complete ({} actions)",
        planned_actions.len()
    );

    // --- Coverage (issue #2359, BUG 2) ---
    // Make goal coverage a first-class allocation rule: ensure every
    // incomplete active goal that lacks a live engineer gets exactly one,
    // ahead of any extra parallelism for already-covered goals. The AIMD
    // scaler stays a hard safety cap — `decide_with_brain` already called
    // `scaler.adjust()`, so `current_max()` is the cap it used this cycle.
    let coverage_cap = config
        .scaler
        .as_ref()
        .map(|s| s.current_max() as usize)
        .unwrap_or(config.max_concurrent_actions as usize);
    let coverage_report =
        crate::ooda_loop::coverage::ensure_goal_coverage(state, &mut planned_actions, coverage_cap);
    eprintln!(
        "[simard] OODA cycle: coverage — {} (cap {coverage_cap})",
        coverage_report.log_line()
    );

    // --- Act ---
    state.current_phase = OodaPhase::Act;
    eprintln!("[simard] OODA cycle: entering Act phase");
    let act_start = Instant::now();
    // Bound concurrent engineer starts to the same AIMD cap coverage used to
    // allocate them, so dispatch concurrency stays resource-aware.
    let outcomes = act(&planned_actions, bridges, state, coverage_cap)?;
    let act_elapsed = act_start.elapsed();
    eprintln!(
        "[simard] OODA cycle: Act complete ({} outcomes, {:.1}s)",
        outcomes.len(),
        act_elapsed.as_secs_f64()
    );

    // --- WS-2: poll subagent tmux sessions and GC ended entries (>24h) ---
    if let Err(e) = crate::subagent_sessions::poll_and_gc(&crate::subagent_sessions::TmuxProbe) {
        eprintln!("[simard] OODA cycle: subagent_sessions poll/gc failed: {e}");
    }

    // --- Update goal current_activity from outcomes ---
    for outcome in &outcomes {
        // Report errors to AIMD scaler for rate-limit backoff (issue #2182).
        if !outcome.success
            && let Some(ref scaler) = config.scaler
        {
            scaler.report_reason(&outcome.detail);
        }

        if let Some(goal_id) = &outcome.action.goal_id {
            // Update per-goal failure cooldown counter.
            if outcome.success {
                state.goal_failure_counts.remove(goal_id);
            } else {
                let entry = state
                    .goal_failure_counts
                    .entry(goal_id.clone())
                    .or_insert(0);
                *entry = entry.saturating_add(1);
                eprintln!(
                    "[simard] OODA cycle: goal '{goal_id}' consecutive failures = {} (cooldown will demote urgency)",
                    *entry
                );
            }

            if let Some(goal) = state
                .active_goals
                .active
                .iter_mut()
                .find(|g| g.id == *goal_id)
            {
                let activity = if outcome.success {
                    format!(
                        "{}: {}",
                        outcome.action.kind,
                        truncate_detail(&outcome.detail, 120)
                    )
                } else {
                    format!(
                        "{} (failed): {}",
                        outcome.action.kind,
                        truncate_detail(&outcome.detail, 120)
                    )
                };
                goal.current_activity = Some(activity);
            }
        }
    }

    // --- Memory consolidation: execution (record per-action output) ---
    for outcome in &outcomes {
        if let Err(e) = memory_consolidation::execution_memory_operations(
            &outcome.detail,
            &cycle_session_id,
            &*bridges.memory,
        ) {
            eprintln!("[simard] OODA consolidation: execution memory failed: {e}");
        }
    }

    // --- Memory consolidation: procedural learning from successful actions ---
    for outcome in &outcomes {
        if outcome.success {
            // PR-C (issue #2281, problem 3): store procedures under a
            // goal-scoped, trigger-bearing name so `recall_procedure`'s
            // `CONTAINS` matcher can hit them when a future objective
            // mentions the same triggers, PR number, or file extension.
            // Pre-PR-C this was `format!("ooda:{}", outcome.action.kind)`
            // which never matched any natural-language objective.
            let proc_name = compose_procedure_name(
                outcome.action.kind.clone(),
                outcome.action.goal_id.as_deref(),
                &objective_summary,
                &outcome.action.description,
            );
            let steps = [outcome.action.description.clone(), outcome.detail.clone()];
            // Issue #2298: `store_procedure` is an idempotent upsert, so an
            // existing procedure is only reinforced (its `usage_count` bumps),
            // never re-created. Probe first so the log distinguishes the two —
            // otherwise frozen procedural memory reads as fresh learning. A
            // recall failure is non-fatal and defaults to the "stored" wording.
            let already_present = bridges
                .memory
                .procedure_exists(&proc_name)
                .unwrap_or_else(|e| {
                    eprintln!("[simard] OODA consolidation: procedural recall failed: {e}");
                    false
                });
            if let Err(e) = bridges.memory.store_procedure(&proc_name, &steps, &[]) {
                tracing::warn!(
                    procedure_name = %proc_name,
                    error = %e,
                    "OODA consolidation: procedural memory store failed",
                );
                eprintln!("[simard] OODA consolidation: procedural memory failed: {e}");
            } else if already_present {
                eprintln!("[simard] OODA consolidation: reinforced procedure '{proc_name}'");
            } else {
                // ws2 #2295: structured tracing event in addition to the
                // eprintln! line. The structured `procedure_name` field is
                // written verbatim by every fmt layer (JSON and the
                // default human formatter) and bypasses any line-length
                // truncation a downstream log shipper might apply to the
                // free-form message — making "is my trigger list
                // truncated?" an answerable question from the journal.
                tracing::info!(
                    procedure_name = %proc_name,
                    "OODA consolidation: stored procedure",
                );
                eprintln!("[simard] OODA consolidation: stored procedure '{proc_name}'");
            }
        }
    }

    // --- Review: analyze outcomes and propose improvements ---
    let review_proposals = review_outcomes(&outcomes, act_elapsed);

    // --- Memory consolidation: reflection ---
    {
        let transcript = outcomes
            .iter()
            .map(|o| format!("{}: {}", o.action.description, o.detail))
            .collect::<Vec<_>>()
            .join("\n");
        if let Err(e) = memory_consolidation::reflection_memory_operations(
            &transcript,
            &[],
            &cycle_session_id,
            &*bridges.memory,
        ) {
            eprintln!("[simard] OODA consolidation: reflection failed: {e}");
        }
    }

    // --- Consolidate: best-effort memory maintenance after each cycle ---
    if let Err(e) = bridges.memory.consolidate_episodes(10) {
        eprintln!("[simard] OODA consolidate: episode consolidation failed: {e}");
    }
    if let Err(e) = bridges.memory.prune_expired_sensory() {
        eprintln!("[simard] OODA consolidate: sensory prune failed: {e}");
    }

    if !review_proposals.is_empty() {
        eprintln!(
            "[simard] OODA review: generated {} improvement proposal(s)",
            review_proposals.len()
        );
        // Persist proposals to cognitive memory (best-effort).
        for directive in &review_proposals {
            if let Err(e) = bridges.memory.store_fact(
                &format!("improvement-{}", crate::goals::goal_slug(&directive.title)),
                &format!(
                    "priority={} status={} rationale={}",
                    directive.priority, directive.status, directive.rationale
                ),
                0.8,
                &["improvement".to_string(), "ooda-review".to_string()],
                "ooda-review",
            ) {
                eprintln!("[simard] OODA review: failed to persist proposal: {e}");
            }
        }
        // Convert to ImprovementCycle signals for the next observe() pass.
        let gym_baseline = observation
            .gym_health
            .clone()
            .unwrap_or_else(|| GymSuiteScore {
                suite_id: "ooda-review".to_string(),
                overall: 0.0,
                dimensions: ScoreDimensions::default(),
                scenario_count: 0,
                scenarios_passed: 0,
                pass_rate: 0.0,
                recorded_at_unix_ms: None,
            });
        for _proposal in &review_proposals {
            state.review_improvements.push(ImprovementCycle {
                baseline: gym_baseline.clone(),
                proposed_changes: Vec::new(),
                post_score: None,
                regressions: Vec::new(),
                decision: None,
                final_phase: ImprovementPhase::Eval,
                weak_dimensions: Vec::new(),
                weak_dimension_details: Vec::new(),
                target_dimension: None,
            });
        }
    }

    // --- Curate: archive completed goals, promote from backlog ---
    // With a deploy-aware done-gate installed (production daemon, issue #2419),
    // a completed goal archives only with hard evidence — merged PR, closed
    // issue, and (for self-affecting changes) a verified deploy; blocked goals
    // stay active with a recorded blocker. Without one (tests / non-daemon
    // callers), this is the legacy unguarded archive.
    let archived = match &bridges.completion_evidence {
        Some(source) => {
            let (archived, blocked) = crate::goal_curation::archive_completed_evidence_aware(
                &mut state.active_goals,
                source.as_ref(),
            );
            for (goal, missing) in &blocked {
                eprintln!(
                    "[simard] OODA curate: completion BLOCKED for goal '{}' — missing {}",
                    goal.id,
                    missing
                        .iter()
                        .map(crate::goal_curation::MissingEvidence::label)
                        .collect::<Vec<_>>()
                        .join("; "),
                );
            }
            archived
        }
        None => crate::goal_curation::archive_completed(&mut state.active_goals),
    };
    if !archived.is_empty() {
        eprintln!(
            "[simard] OODA curate: archived {} completed goal(s): {}",
            archived.len(),
            archived
                .iter()
                .map(|g| g.id.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }

    // Promote highest-scoring backlog items to fill freed slots.
    promote_from_backlog(&mut state.active_goals);

    // Corruption guard: check that no pre-cycle active goal disappeared
    // without going through archive_completed. A goal may legitimately leave
    // active via archival — those will no longer be in active but will appear
    // in archived. Any goal that is missing from active AND was not archived
    // this cycle is a corruption signal; restore the board from the snapshot.
    {
        let archived_ids: std::collections::HashSet<&str> =
            archived.iter().map(|g| g.id.as_str()).collect();
        let post_active_ids: std::collections::HashSet<&str> = state
            .active_goals
            .active
            .iter()
            .map(|g| g.id.as_str())
            .collect();
        let vanished: Vec<&str> = pre_cycle_active_ids
            .iter()
            .map(|s| s.as_str())
            .filter(|id| !post_active_ids.contains(*id) && !archived_ids.contains(*id))
            .collect();
        if !vanished.is_empty() {
            eprintln!(
                "[simard] OODA curate: CORRUPTION DETECTED — {} goal(s) vanished without \
                 archival: {}; skipping persist to protect board",
                vanished.len(),
                vanished.join(", "),
            );
            // Do not persist — return without calling persist_board so the
            // last-known-good state on disk is preserved.
        } else {
            // Persist the updated board to cognitive memory and disk (best-effort).
            // When goals were archived, use save_goal_board_with_removals so that
            // the merge-on-write step cannot resurrect them from the persisted
            // snapshot (issue #2264 — archived goals reappearing every cycle).
            let archived_goal_ids: Vec<String> = archived.iter().map(|g| g.id.clone()).collect();
            let persist_result = if archived_goal_ids.is_empty() {
                crate::goal_curation::persist_board(&state.active_goals, &*bridges.memory)
            } else {
                save_goal_board_with_removals(
                    &state.active_goals,
                    &archived_goal_ids,
                    &*bridges.memory,
                )
                .and_then(|()| {
                    // Goal-board curation with force-removals is a durable
                    // goal-archival event (issue #2327): the classifier stores
                    // it at full importance with the {importance, event_kind,
                    // goal_id, cycle, is_operational} metadata, merged with the
                    // existing board-count fields so neither signal is lost.
                    let summary = state.active_goals.durable_summary();
                    let ctx = crate::memory_consolidation::classifier::IntakeContext {
                        goal_id: None,
                        cycle: Some(state.cycle_count),
                    };
                    let decision = crate::memory_consolidation::classifier::classify(
                        &summary,
                        "goal-curator",
                        &ctx,
                    );
                    crate::memory_consolidation::classifier::global_intake_counters()
                        .record(&decision);
                    if let Some(meta) = decision.metadata() {
                        let mut json = meta.to_json();
                        if let Some(obj) = json.as_object_mut() {
                            obj.insert(
                                "active_count".to_string(),
                                serde_json::json!(state.active_goals.active.len()),
                            );
                            obj.insert(
                                "backlog_count".to_string(),
                                serde_json::json!(state.active_goals.backlog.len()),
                            );
                            obj.insert(
                                "force_removed".to_string(),
                                serde_json::json!(archived_goal_ids.len()),
                            );
                        }
                        bridges
                            .memory
                            .store_episode(&summary, "goal-curator", Some(&json))?;
                    }
                    Ok(())
                })
            };
            if let Err(e) = persist_result {
                eprintln!("[simard] OODA curate: failed to persist goal board: {e}");
            }
        }
    }

    // --- Memory consolidation: persistence at cycle end ---
    // Flush working memory to episodes before final persistence.
    if let Err(e) =
        memory_consolidation::consolidation_persistence(&cycle_session_id, &*bridges.memory)
    {
        eprintln!("[simard] OODA consolidation: flush failed: {e}");
    }
    if let Err(e) =
        memory_consolidation::persistence_memory_operations(&cycle_session_id, &*bridges.memory)
    {
        eprintln!("[simard] OODA consolidation: persistence failed: {e}");
    }

    state.cycle_count += 1;

    // --- Automatic promotion (distillation) scheduler (issue #2327, R4) ---
    // Fire episode → fact/procedure distillation when the undistilled-episode
    // count crosses the threshold OR the cycle-count interval elapses,
    // decoupled from whether the brain picked `ConsolidateMemory`. Trigger
    // gating is cheap; the recipe runner is only constructed when a trigger
    // fires (and gracefully no-ops when recipe-runner-rs is unavailable).
    {
        let schedule = crate::memory_consolidation::scheduler::DistillSchedule {
            min_episodes: config.distill_min_episodes,
            interval_cycles: config.distill_interval_cycles,
        };
        let cycles_since_last = state.cycle_count.saturating_sub(state.last_distill_cycle);
        match crate::memory_consolidation::scheduler::run_scheduled_distillation(
            &*bridges.memory,
            &bridges.repo_root,
            &schedule,
            cycles_since_last,
        ) {
            Ok(Some(report)) => {
                state.last_distill_cycle = state.cycle_count;
                eprintln!(
                    "[simard] OODA distill scheduler: {} episodes → {} facts, {} procedures, {} marked",
                    report.input_count,
                    report.fact_count,
                    report.procedure_count,
                    report.marked_count
                );
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("[simard] OODA distill scheduler: pass failed (non-fatal): {e}");
            }
        }
    }

    // Emit the per-cycle episode-intake hygiene summary (dropped/stored/
    // down-scoped) accumulated by the ingestion classifier (issue #2327, R3).
    crate::memory_consolidation::classifier::global_intake_counters().log_summary();

    // --- Post-cycle cleanup (issue #2167) ---
    // Prune goal_failure_counts entries for goals no longer on the board.
    state.prune_stale_failure_counts();
    // Release prepared_context so the allocation doesn't persist until the
    // next cycle replaces it.
    state.prepared_context = None;

    let brain_judgments = crate::ooda_brain::take_brain_judgments();
    Ok(CycleReport {
        cycle_number: state.cycle_count,
        observation,
        priorities,
        planned_actions,
        outcomes,
        brain_judgments,
    })
}

/// Truncate a detail string to at most `max_len` characters (Unicode scalar
/// values), appending "…" if truncated.
fn truncate_detail(s: &str, max_len: usize) -> String {
    let trimmed = s.trim();
    let mut chars = trimmed.char_indices();
    match chars.nth(max_len) {
        None => trimmed.to_string(),
        Some((byte_pos, _)) => format!("{}…", &trimmed[..byte_pos]),
    }
}

/// Returns `Some(reason)` if the board contains obviously corrupt or
/// placeholder goals that should not be accepted as valid loaded state.
///
/// Heuristics:
/// - Goal id shorter than 5 chars (catches `g1`, `g12`, `g123`, `g1234`)
/// - Description matches the placeholder pattern `^goal [a-z0-9]{1,4}$` (case-insensitive)
pub(crate) fn board_integrity_suspect(board: &crate::goal_curation::GoalBoard) -> Option<String> {
    for goal in &board.active {
        if goal.id.len() < 5 {
            return Some(format!(
                "goal '{}' has suspiciously short id (len {})",
                goal.id,
                goal.id.len()
            ));
        }
        if is_placeholder_description(&goal.description) {
            return Some(format!(
                "goal '{}' has placeholder description '{}'",
                goal.id, goal.description
            ));
        }
    }
    None
}

/// Returns `true` when `desc` matches the placeholder pattern
/// `^\s*goal\s+[a-z0-9]{1,4}\s*$` (case-insensitive).
///
/// Matches strings like `Goal g1`, `goal g1`, `GOAL abc`.
pub(crate) fn is_placeholder_description(desc: &str) -> bool {
    let s = desc.trim().to_lowercase();
    if let Some(rest) = s.strip_prefix("goal") {
        let rest = rest.trim();
        !rest.is_empty() && rest.len() <= 4 && rest.chars().all(|c| c.is_ascii_alphanumeric())
    } else {
        false
    }
}

/// Clear `assigned_to` for any active goal whose assigned tmux session is no
/// longer alive. Resets the goal status to `NotStarted` so it can be
/// re-dispatched on the next OODA cycle.
///
/// Skipped entirely when:
/// - `tmux list-sessions` fails (tmux absent or permission error)
/// - The live session list is empty (not running inside tmux)
///
/// This prevents false-positive clearing when Simard is run outside a tmux
/// environment (e.g., in CI).
fn sweep_stale_assignments(board: &mut crate::goal_curation::GoalBoard) {
    use std::collections::HashSet;
    use std::process::Command;

    let output = match Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return,
    };

    let live: HashSet<String> = String::from_utf8_lossy(&output)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    sweep_stale_assignments_with_sessions(board, &live);
}

/// Core assignment-sweep logic parameterised on a pre-built live session set.
///
/// Exposed as `pub(crate)` so unit tests can exercise the sweep logic without
/// spawning a real tmux process.  The public entry point is
/// [`sweep_stale_assignments`], which populates `live_sessions` from tmux.
///
/// Skipped (no-op) when `live_sessions` is empty — avoids clearing all
/// assignments when running outside a tmux environment (e.g., CI).
pub(crate) fn sweep_stale_assignments_with_sessions(
    board: &mut crate::goal_curation::GoalBoard,
    live_sessions: &std::collections::HashSet<String>,
) {
    if live_sessions.is_empty() {
        return;
    }

    for goal in board.active.iter_mut() {
        let is_stale = goal
            .assigned_to
            .as_deref()
            .is_some_and(|s| !live_sessions.contains(s));
        if is_stale {
            let session = goal.assigned_to.take().unwrap_or_default();
            eprintln!(
                "[simard] OODA start: cleared stale assignment '{}' for goal '{}'",
                session, goal.id
            );
            goal.status = crate::goal_curation::GoalProgress::NotStarted;
        }
    }
}

// ============================================================================
// PR-C (issue #2281, problem 3): procedure-naming helpers
// ============================================================================
//
// The OODA cycle's `store_procedure` call at the top of this file now
// constructs goal-scoped, trigger-bearing names via these helpers so
// that `recall_procedure`'s `CONTAINS` matcher actually hits when the
// next objective mentions any of the embedded trigger keywords.
//
// Pre-PR-C the writer used `format!("ooda:{}", outcome.action.kind)`
// which produced names like `ooda:advance-goal` that no
// natural-language objective ever contains. See
// `docs/reference/cognitive-memory-bootstrap-procedures.md`.

/// Map an [`ActionKind`] to the short verb-phrase tag used as the
/// `pattern` segment of a procedure name (e.g. `pr-merge`, `ci-fix`,
/// `run-tests`). Centralised so the mapping table in the docs has a
/// single source of truth.
pub fn pattern_for(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::AdvanceGoal => "pr-merge",
        ActionKind::RunImprovement => "ci-fix",
        ActionKind::ConsolidateMemory => "consolidate",
        ActionKind::RunGymEval => "run-tests",
        ActionKind::BuildSkill => "build-skill",
        ActionKind::LaunchSession => "engineer-loop",
        ActionKind::ResearchQuery => "research",
        ActionKind::PollDeveloperActivity => "poll-activity",
        ActionKind::ExtractIdeas => "extract-ideas",
        ActionKind::SafeUpdate => "safe-update",
    }
}

/// Always-merged base triggers for each [`ActionKind`]. These guarantee
/// that the most common engineer-loop keywords ("merge", "ci", "test",
/// …) appear in the procedure name even when the objective text adds
/// nothing useful via [`derive_triggers_from_objective`].
pub fn base_triggers_for(kind: ActionKind) -> &'static [&'static str] {
    match kind {
        ActionKind::AdvanceGoal => &["merge", "pr", "review", "ci"],
        ActionKind::RunImprovement => &["ci", "green", "failing", "fix-ci", "improve"],
        ActionKind::ConsolidateMemory => &["consolidate", "memory", "distill"],
        ActionKind::RunGymEval => &["test", "gym", "eval", "benchmark"],
        ActionKind::BuildSkill => &["skill", "build", "scaffold"],
        ActionKind::LaunchSession => &["engineer", "session", "spawn"],
        ActionKind::ResearchQuery => &["research", "investigate", "explore"],
        ActionKind::PollDeveloperActivity => &["poll", "activity", "status"],
        ActionKind::ExtractIdeas => &["idea", "extract", "brainstorm"],
        ActionKind::SafeUpdate => &["update", "upgrade", "version"],
    }
}

/// Scan the objective + action description for the two narrow
/// identifier shapes that empirically improve `recall_procedure`
/// hit rate the most: PR numbers (`#NNNN`) and file extensions
/// (`.<ext>`). Captures are lowercased and deduped against the base
/// trigger list at composition time.
///
/// This is **not** a general tokenizer — `tokenize_objective` in
/// `memory_consolidation` already exists for episodic-recall keyword
/// extraction. The two paths intentionally use different rules.
pub fn derive_triggers_from_objective(objective: &str, action_desc: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let combined = format!("{objective} {action_desc}");

    // Pass 1: PR numbers — `#NNNN` (any positive number of digits).
    {
        let bytes = combined.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'#' {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 {
                    let digits = &combined[i + 1..j];
                    let key = digits.to_ascii_lowercase();
                    if seen.insert(key.clone()) {
                        out.push(key);
                    }
                    i = j;
                    continue;
                }
            }
            i += 1;
        }
    }

    // Pass 2: file extensions — `.<ext>` where `<ext>` is 3..=5 alphanumeric
    // characters starting with a letter, terminated by a non-alphanumeric
    // boundary or end-of-string.
    //
    // The 3-character lower bound aligns with the **read-side** floor
    // enforced by `memory_consolidation::tokenize_objective`, which
    // drops every objective-derived token shorter than 3 chars. A
    // 1- or 2-char derived trigger (`g`, `rs`, …) can therefore never
    // be matched by a future tokenized recall query — it would only
    // sit in the procedure name as visible-but-dead weight and, when
    // it appears as the trailing trigger, look exactly like the
    // mid-word truncation symptom reported in ws2 #2295. Aligning the
    // floors removes that confusion without losing any real recall
    // power.
    {
        let bytes = combined.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'.' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_alphabetic() {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_alphanumeric() && j - i <= 5 {
                    j += 1;
                }
                let ext_len = j - i - 1;
                let at_word_boundary = j == bytes.len() || !bytes[j].is_ascii_alphanumeric();
                if (3..=5).contains(&ext_len) && at_word_boundary {
                    let ext = &combined[i + 1..j];
                    let key = ext.to_ascii_lowercase();
                    if seen.insert(key.clone()) {
                        out.push(key);
                    }
                    i = j;
                    continue;
                }
            }
            i += 1;
        }
    }
    out
}

/// Compose the full procedure name a successful OODA outcome should
/// be stored under:
///
/// ```text
/// {pattern}:{scope} | triggers: {base,...,derived,...}
/// ```
///
/// `scope` is the action's `goal_id` when present, else `ad-hoc`.
/// `base` triggers always precede `derived` triggers; the merge
/// dedup drops later duplicates so a derived keyword shadowed by a
/// base keyword only appears once.
pub fn compose_procedure_name(
    kind: ActionKind,
    goal_id: Option<&str>,
    objective: &str,
    action_desc: &str,
) -> String {
    let pattern = pattern_for(kind.clone());
    let scope = goal_id.unwrap_or("ad-hoc");
    let base = base_triggers_for(kind.clone());
    let derived = derive_triggers_from_objective(objective, action_desc);

    // base first, derived appended only if not already in base
    let mut triggers: Vec<String> = base.iter().map(|s| (*s).to_string()).collect();
    let base_set: std::collections::HashSet<&str> = base.iter().copied().collect();
    for d in derived {
        if !base_set.contains(d.as_str()) {
            triggers.push(d);
        }
    }
    format!("{pattern}:{scope} | triggers: {}", triggers.join(","))
}

#[cfg(test)]
mod tests_sweep {
    use std::collections::HashSet;

    use super::sweep_stale_assignments_with_sessions;
    use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, add_active_goal};

    fn make_goal(id: &str, session: Option<&str>) -> ActiveGoal {
        ActiveGoal {
            parent_goal_id: None,
            repo: None,
            id: id.to_string(),
            description: format!("Goal {id}"),
            priority: 1,
            status: GoalProgress::InProgress { percent: 50 },
            assigned_to: session.map(str::to_string),
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        }
    }

    fn live(sessions: &[&str]) -> HashSet<String> {
        sessions.iter().map(|s| s.to_string()).collect()
    }

    /// Dead session → assigned_to cleared, status reset to NotStarted.
    #[test]
    fn clears_dead_session_assignment() {
        let mut board = GoalBoard::new();
        add_active_goal(&mut board, make_goal("g1", Some("dead-session"))).unwrap();

        sweep_stale_assignments_with_sessions(&mut board, &live(&["alive-session"]));

        let goal = &board.active[0];
        assert!(
            goal.assigned_to.is_none(),
            "assigned_to must be cleared for dead session"
        );
        assert!(
            matches!(goal.status, GoalProgress::NotStarted),
            "status must be reset to NotStarted, got {:?}",
            goal.status
        );
    }

    /// Live session → assignment preserved.
    #[test]
    fn preserves_live_session_assignment() {
        let mut board = GoalBoard::new();
        add_active_goal(&mut board, make_goal("g1", Some("live-session"))).unwrap();

        sweep_stale_assignments_with_sessions(&mut board, &live(&["live-session"]));

        let goal = &board.active[0];
        assert_eq!(goal.assigned_to.as_deref(), Some("live-session"));
        assert!(
            matches!(goal.status, GoalProgress::InProgress { .. }),
            "status must not change for live session"
        );
    }

    /// Empty live-session set → skip sweep entirely (non-tmux environment guard).
    #[test]
    fn skips_sweep_when_live_sessions_empty() {
        let mut board = GoalBoard::new();
        add_active_goal(&mut board, make_goal("g1", Some("some-session"))).unwrap();

        sweep_stale_assignments_with_sessions(&mut board, &live(&[]));

        let goal = &board.active[0];
        assert_eq!(
            goal.assigned_to.as_deref(),
            Some("some-session"),
            "must not clear assignments when live_sessions is empty (non-tmux guard)"
        );
    }

    /// Unassigned goal is untouched regardless of live sessions.
    #[test]
    fn ignores_unassigned_goals() {
        let mut board = GoalBoard::new();
        add_active_goal(&mut board, make_goal("g1", None)).unwrap();

        sweep_stale_assignments_with_sessions(&mut board, &live(&["some-session"]));

        let goal = &board.active[0];
        assert!(goal.assigned_to.is_none());
        assert!(
            matches!(goal.status, GoalProgress::InProgress { .. }),
            "status must be unchanged for unassigned goal"
        );
    }

    /// Mixed board: only the goal with a dead session is cleared.
    #[test]
    fn clears_only_dead_assignments_in_mixed_board() {
        let mut board = GoalBoard::new();
        add_active_goal(&mut board, make_goal("live-goal", Some("alive"))).unwrap();
        add_active_goal(&mut board, make_goal("dead-goal", Some("dead"))).unwrap();
        add_active_goal(&mut board, make_goal("unassigned-goal", None)).unwrap();

        sweep_stale_assignments_with_sessions(&mut board, &live(&["alive"]));

        let live_goal = board.active.iter().find(|g| g.id == "live-goal").unwrap();
        assert_eq!(live_goal.assigned_to.as_deref(), Some("alive"));

        let dead_goal = board.active.iter().find(|g| g.id == "dead-goal").unwrap();
        assert!(dead_goal.assigned_to.is_none());
        assert!(matches!(dead_goal.status, GoalProgress::NotStarted));

        let unassigned = board
            .active
            .iter()
            .find(|g| g.id == "unassigned-goal")
            .unwrap();
        assert!(unassigned.assigned_to.is_none());
    }

    /// Goals assigned to the same session that died are all cleared.
    #[test]
    fn clears_all_goals_for_same_dead_session() {
        let mut board = GoalBoard::new();
        add_active_goal(&mut board, make_goal("g1", Some("dead"))).unwrap();
        add_active_goal(&mut board, make_goal("g2", Some("dead"))).unwrap();

        sweep_stale_assignments_with_sessions(&mut board, &live(&["other"]));

        for goal in &board.active {
            assert!(goal.assigned_to.is_none(), "g={}", goal.id);
            assert!(
                matches!(goal.status, GoalProgress::NotStarted),
                "g={}",
                goal.id
            );
        }
    }
}

#[cfg(test)]
mod tests_board_integrity {
    use super::*;
    use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, add_active_goal};

    fn make_goal(id: &str, desc: &str) -> ActiveGoal {
        ActiveGoal {
            parent_goal_id: None,
            repo: None,
            id: id.to_string(),
            description: desc.to_string(),
            priority: 1,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        }
    }

    // --- is_placeholder_description ---

    #[test]
    fn placeholder_description_matches_goal_g1() {
        assert!(is_placeholder_description("Goal g1"));
    }

    #[test]
    fn placeholder_description_matches_lowercase() {
        assert!(is_placeholder_description("goal g1"));
    }

    #[test]
    fn placeholder_description_matches_uppercase() {
        assert!(is_placeholder_description("GOAL abc"));
    }

    #[test]
    fn placeholder_description_ignores_leading_trailing_whitespace() {
        assert!(is_placeholder_description("  goal g1  "));
    }

    #[test]
    fn placeholder_description_rejects_real_description() {
        assert!(!is_placeholder_description("Ship the v1 release"));
    }

    #[test]
    fn placeholder_description_rejects_longer_suffix() {
        // "g12345" has 6 chars — too long
        assert!(!is_placeholder_description("goal g12345"));
    }

    #[test]
    fn placeholder_description_rejects_empty() {
        assert!(!is_placeholder_description(""));
    }

    // --- board_integrity_suspect ---

    #[test]
    fn suspect_board_short_id() {
        let mut board = GoalBoard::new();
        add_active_goal(&mut board, make_goal("g1", "Something meaningful")).unwrap();
        assert!(board_integrity_suspect(&board).is_some());
    }

    #[test]
    fn suspect_board_placeholder_description() {
        let mut board = GoalBoard::new();
        add_active_goal(&mut board, make_goal("ship-v1-release", "Goal g1")).unwrap();
        assert!(board_integrity_suspect(&board).is_some());
    }

    #[test]
    fn clean_board_passes() {
        let mut board = GoalBoard::new();
        add_active_goal(
            &mut board,
            make_goal("ship-v1-feature", "Ship the v1 feature"),
        )
        .unwrap();
        assert!(board_integrity_suspect(&board).is_none());
    }

    #[test]
    fn empty_board_passes() {
        let board = GoalBoard::new();
        assert!(board_integrity_suspect(&board).is_none());
    }

    // --- is_placeholder_description: boundary / edge cases ---

    #[test]
    fn placeholder_description_no_space_between_goal_and_suffix() {
        // "goalg1" — no space; strip_prefix("goal") yields "g1", which is 2-char alphanumeric.
        assert!(is_placeholder_description("goalg1"));
    }

    #[test]
    fn placeholder_description_single_digit_suffix() {
        assert!(is_placeholder_description("goal 1"));
    }

    #[test]
    fn placeholder_description_two_char_alpha_suffix() {
        assert!(is_placeholder_description("goal ab"));
    }

    #[test]
    fn placeholder_description_four_char_suffix_is_accepted() {
        // 4-char token is the maximum accepted (rest.len() <= 4).
        assert!(is_placeholder_description("goal g123"));
    }

    #[test]
    fn placeholder_description_five_char_suffix_is_rejected() {
        // "g1234" is exactly 5 chars — one over the limit.
        assert!(!is_placeholder_description("goal g1234"));
    }

    #[test]
    fn placeholder_description_rejects_goal_alone() {
        // No suffix at all — rest is empty after trim.
        assert!(!is_placeholder_description("goal"));
    }

    #[test]
    fn placeholder_description_rejects_whitespace_only_after_goal() {
        // "goal   " — trim produces "", which is empty → false.
        assert!(!is_placeholder_description("goal   "));
    }

    #[test]
    fn placeholder_description_rejects_non_alphanumeric_suffix() {
        // Hyphen is not alphanumeric; must be rejected.
        assert!(!is_placeholder_description("goal g-1"));
    }

    #[test]
    fn placeholder_description_rejects_mixed_real_and_keyword() {
        // A real description that happens to start with "goal" is not a placeholder.
        assert!(!is_placeholder_description("goal: ship the v2 release"));
    }

    // --- board_integrity_suspect: boundary / edge cases ---

    #[test]
    fn suspect_board_four_char_id_is_flagged() {
        // len == 4 < 5 → suspect.
        let mut board = GoalBoard::new();
        add_active_goal(&mut board, make_goal("abcd", "A real description")).unwrap();
        assert!(board_integrity_suspect(&board).is_some());
    }

    #[test]
    fn clean_board_five_char_id_passes() {
        // len == 5 — exactly at the boundary, should NOT be flagged.
        let mut board = GoalBoard::new();
        add_active_goal(&mut board, make_goal("abcde", "A real description")).unwrap();
        assert!(board_integrity_suspect(&board).is_none());
    }

    #[test]
    fn suspect_board_mixed_goals_first_bad_detected() {
        // Board with one good goal followed by one corrupt goal — suspect detected.
        let mut board = GoalBoard::new();
        add_active_goal(
            &mut board,
            make_goal("ship-v2-feature", "Ship the v2 feature"),
        )
        .unwrap();
        add_active_goal(&mut board, make_goal("g1", "Something meaningful")).unwrap();
        assert!(board_integrity_suspect(&board).is_some());
    }

    #[test]
    fn clean_board_multiple_good_goals() {
        let mut board = GoalBoard::new();
        add_active_goal(
            &mut board,
            make_goal("ship-v1-feature", "Ship the v1 feature"),
        )
        .unwrap();
        add_active_goal(
            &mut board,
            make_goal("fix-db-perf", "Fix database performance regression"),
        )
        .unwrap();
        add_active_goal(
            &mut board,
            make_goal("improve-docs", "Improve onboarding documentation"),
        )
        .unwrap();
        assert!(board_integrity_suspect(&board).is_none());
    }

    // --- curate corruption guard logic ---
    //
    // The curate guard computes:
    //   vanished = pre_cycle_ids - post_active_ids - archived_ids
    // and skips persist_board when vanished is non-empty.
    // These tests verify the set-logic directly.

    #[test]
    fn curate_guard_no_vanished_when_goal_still_active() {
        let pre: std::collections::HashSet<String> = ["goal-abc".to_string()].into_iter().collect();
        let post_active: std::collections::HashSet<&str> = ["goal-abc"].into_iter().collect();
        let archived: std::collections::HashSet<&str> = [].into_iter().collect();
        let vanished: Vec<&str> = pre
            .iter()
            .map(|s| s.as_str())
            .filter(|id| !post_active.contains(*id) && !archived.contains(*id))
            .collect();
        assert!(vanished.is_empty());
    }

    #[test]
    fn curate_guard_no_vanished_when_goal_properly_archived() {
        let pre: std::collections::HashSet<String> = ["goal-abc".to_string()].into_iter().collect();
        let post_active: std::collections::HashSet<&str> = [].into_iter().collect();
        let archived: std::collections::HashSet<&str> = ["goal-abc"].into_iter().collect();
        let vanished: Vec<&str> = pre
            .iter()
            .map(|s| s.as_str())
            .filter(|id| !post_active.contains(*id) && !archived.contains(*id))
            .collect();
        assert!(vanished.is_empty());
    }

    #[test]
    fn curate_guard_detects_vanished_goal() {
        let pre: std::collections::HashSet<String> =
            ["goal-abc".to_string(), "goal-xyz".to_string()]
                .into_iter()
                .collect();
        let post_active: std::collections::HashSet<&str> = ["goal-abc"].into_iter().collect();
        let archived: std::collections::HashSet<&str> = [].into_iter().collect();
        let vanished: Vec<&str> = pre
            .iter()
            .map(|s| s.as_str())
            .filter(|id| !post_active.contains(*id) && !archived.contains(*id))
            .collect();
        assert_eq!(vanished.len(), 1);
        assert!(vanished.contains(&"goal-xyz"));
    }

    #[test]
    fn curate_guard_detects_multiple_vanished_goals() {
        let pre: std::collections::HashSet<String> = [
            "goal-a".to_string(),
            "goal-b".to_string(),
            "goal-c".to_string(),
        ]
        .into_iter()
        .collect();
        let post_active: std::collections::HashSet<&str> = [].into_iter().collect();
        let archived: std::collections::HashSet<&str> = ["goal-a"].into_iter().collect();
        let vanished: Vec<&str> = pre
            .iter()
            .map(|s| s.as_str())
            .filter(|id| !post_active.contains(*id) && !archived.contains(*id))
            .collect();
        assert_eq!(vanished.len(), 2);
        assert!(vanished.contains(&"goal-b"));
        assert!(vanished.contains(&"goal-c"));
    }

    #[test]
    fn curate_guard_empty_pre_cycle_always_clean() {
        let pre: std::collections::HashSet<String> = [].into_iter().collect();
        let post_active: std::collections::HashSet<&str> = [].into_iter().collect();
        let archived: std::collections::HashSet<&str> = [].into_iter().collect();
        let vanished: Vec<&str> = pre
            .iter()
            .map(|s| s.as_str())
            .filter(|id| !post_active.contains(*id) && !archived.contains(*id))
            .collect();
        assert!(vanished.is_empty());
    }
}

#[cfg(test)]
mod tests_objective_probe {
    use super::build_objective_probe;
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    use crate::goal_curation::{ActiveGoal, GoalProgress};

    fn active_goal(id: &str, description: &str) -> ActiveGoal {
        ActiveGoal {
            parent_goal_id: None,
            repo: None,
            id: id.to_string(),
            description: description.to_string(),
            priority: 1,
            status: GoalProgress::InProgress { percent: 0 },
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        }
    }

    /// The probe MUST contain each active goal's slug-phrase — the exact
    /// substring `prospective_trigger_for` writes as the goal's
    /// `trigger_condition`. Without it `check_triggers` can never fire a
    /// goal's prospective memory during OODA preparation (#2300, cause (b)).
    #[test]
    fn probe_contains_goal_slug_phrase() {
        let active = vec![active_goal(
            "fix-authentication-bug",
            "Investigate and repair the broken login path",
        )];
        let probe = build_objective_probe(&active);
        let slug_phrase = crate::goals::goal_slug("fix-authentication-bug").replace('-', " ");
        assert!(
            probe.contains(&slug_phrase),
            "probe must contain slug-phrase {slug_phrase:?}; got {probe:?}"
        );
    }

    /// The probe must still carry the free-text description so targeted
    /// fact/episode recall keeps working.
    #[test]
    fn probe_retains_goal_description() {
        let active = vec![active_goal(
            "deploy-ci-pipeline",
            "Stand up the CI pipeline",
        )];
        let probe = build_objective_probe(&active);
        assert!(
            probe.contains("Stand up the CI pipeline"),
            "probe must retain the goal description; got {probe:?}"
        );
    }

    /// End-to-end (#2300): an Active goal's prospective — stored with the
    /// slug-derived `trigger_condition` exactly as the live write path does —
    /// MUST fire when `check_triggers` is probed with the objective summary
    /// built by `build_objective_probe`. The goal's free-text description is
    /// deliberately unrelated to the slug to prove the slug-phrase enrichment
    /// (not the description) is what makes the trigger fire.
    #[test]
    fn objective_probe_fires_stored_goal_trigger() {
        let mem = LibraryCognitiveMemory::in_memory().expect("in-memory DB");

        let goal_id = "improve-retrieval-latency";
        // Mirror the live write path's trigger_condition derivation
        // (active_goals_as_records + prospective_trigger_for).
        let trigger_condition = crate::goals::goal_slug(goal_id).replace('-', " ");
        mem.store_prospective(
            "goal:Improve retrieval latency",
            &trigger_condition,
            "Pursue goal: Improve retrieval latency",
            1,
        )
        .unwrap();

        // Description shares no words with the slug, so only the appended
        // slug-phrase can satisfy CONTAINS.
        let active = vec![active_goal(goal_id, "Make the system snappier for users")];
        let probe = build_objective_probe(&active);

        let triggered = mem.check_triggers(&probe).unwrap();
        assert!(
            triggered
                .iter()
                .any(|p| p.trigger_condition == trigger_condition),
            "objective probe must fire the stored goal trigger; probe={probe:?}, \
             got {} triggers",
            triggered.len()
        );
    }
}
