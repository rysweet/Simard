//! Outer OODA cycle implementation extracted from mod.rs (#1266).

use std::time::Instant;

use crate::error::{SimardError, SimardResult};
use crate::goal_curation::{load_goal_board, save_goal_board_with_removals};
use crate::gym_client::ScoreDimensions;
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
/// Takes `&mut OodaClients` so that the optional session can be used for
/// `run_turn` calls during `AdvanceGoal` dispatch.
#[tracing::instrument(skip_all, fields(cycle = state.cycle_count))]
pub fn run_ooda_cycle(
    state: &mut OodaState,
    memories: &mut OodaClients,
    config: &OodaConfig,
) -> SimardResult<CycleReport> {
    // Install per-cycle brain-judgment task-local. Was a `thread_local!`
    // (PR #1472), but brain LLM calls drive Tokio worker threads via the
    // session adapter, so pushes landed on different OS threads than the
    // eventual `take_all()` — daemon `d69c411c52f1` cycle_2 showed
    // `planned_actions: 3` but `brain_judgments: []`.
    crate::ooda_brain::with_brain_judgment_scope(|| run_ooda_cycle_inner(state, memories, config))
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
    memories: &mut OodaClients,
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
    } else if let Ok(board) = load_goal_board(&*memories.memory)
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

    // Seed with the identity's seed goals (override) or the defaults if the
    // board is still empty (#3125). When the resolved identity declares its own
    // `seed_goals` they REPLACE Simard's baked-in DEFAULT_SEED_GOALS at this
    // cold-start seeding site; with no identity (or a `full` identity that
    // declares none) the five defaults seed exactly as before — Simard herself
    // is unchanged. Goals carry the identity's target-repo slug, so they are
    // scoped to its targets, never to rysweet/Simard.
    let identity_seed_goals = &state.identity_cognition.seed_goals;
    if !identity_seed_goals.is_empty() {
        let goals = crate::goal_curation::resolve_seed_goals(identity_seed_goals);
        let n = crate::goal_curation::seed_board_from_seed_goals(&mut state.active_goals, &goals);
        if n > 0 {
            let who = state
                .identity_cognition
                .identity_name
                .as_deref()
                .unwrap_or("identity");
            eprintln!(
                "[simard] OODA start: seeded {n} identity seed goal(s) ({who}) — overriding defaults"
            );
        }
    } else {
        let n = crate::goal_curation::seed_default_board(&mut state.active_goals);
        if n > 0 {
            eprintln!("[simard] OODA start: seeded {n} default goal(s)");
        }
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
        &*memories.memory,
    ) {
        eprintln!("[simard] OODA consolidation: intake failed: {e}");
    }
    // Hydrate prior-session facts into working memory for cross-cycle recall.
    match memory_consolidation::consolidation_intake(
        &cycle_session_id,
        &cycle_objective,
        &*memories.memory,
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
    let observation = observe(state, memories)?;
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
        crate::goals::reconcile_board_prospectives(&state.active_goals, &*memories.memory)
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
        &*memories.memory,
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
    let priorities = match memories.orient_brain.as_ref() {
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
    let mut planned_actions = match memories.decide_brain.as_ref() {
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
    let outcomes = act(&planned_actions, memories, state, coverage_cap)?;
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
                eprintln!(
                    "[simard] OODA cycle: goal '{goal_id}' failure detail: {}",
                    truncate_detail(&outcome.detail, 240)
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
            &*memories.memory,
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
            let already_present =
                memories
                    .memory
                    .procedure_exists(&proc_name)
                    .unwrap_or_else(|e| {
                        eprintln!("[simard] OODA consolidation: procedural recall failed: {e}");
                        false
                    });
            match memories.memory.store_procedure(&proc_name, &steps, &[]) {
                Err(e) => {
                    tracing::warn!(
                        procedure_name = %proc_name,
                        error = %e,
                        "OODA consolidation: procedural memory store failed",
                    );
                    eprintln!("[simard] OODA consolidation: procedural memory failed: {e}");
                }
                Ok(_) if already_present => {
                    eprintln!("[simard] OODA consolidation: reinforced procedure '{proc_name}'");
                }
                Ok(proc_id) => {
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
                    // #2441/#2458: a brand-new skill/lesson procedure was stored
                    // (not a reinforcing re-store) — emit the brain_new_procedure
                    // metric. Best-effort and a `cfg!(test)` no-op.
                    crate::memory_consolidation::reflection_lessons::record_new_procedure(
                        &proc_id, &proc_name,
                    );
                }
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
            &*memories.memory,
        ) {
            eprintln!("[simard] OODA consolidation: reflection failed: {e}");
        }
    }

    // --- Consolidate: best-effort memory maintenance after each cycle ---
    if let Err(e) = memories.memory.consolidate_episodes(10) {
        eprintln!("[simard] OODA consolidate: episode consolidation failed: {e}");
    }
    if let Err(e) = memories.memory.prune_expired_sensory() {
        eprintln!("[simard] OODA consolidate: sensory prune failed: {e}");
    }

    if !review_proposals.is_empty() {
        eprintln!(
            "[simard] OODA review: generated {} improvement proposal(s)",
            review_proposals.len()
        );
        // Persist proposals to cognitive memory (best-effort).
        for directive in &review_proposals {
            if let Err(e) = memories.memory.store_fact(
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

    // --- Per-goal, per-cycle agentic decision (issue #4453) ---
    // Run EXACTLY ONE reasoned decision per active goal per cycle
    // (continue / spawn / reorient / investigate / wait / complete), replacing
    // the imperative never-idle / reap / grace-window predicates with a thin
    // deterministic rail that dispatches to the reasoner. A non-destructive
    // verdict (continue / spawn / wait / investigate) PRESERVES the goal's
    // in-flight `wip_refs` — the root-cause fix for the 70ab8541 idle->reset
    // fault-loop: a standing research goal holding a live PR is never silently
    // reset by a threshold. Destructive ref mutation is reachable only via a
    // reasoned `reorient` / `complete` (a stale-worker concern goes through
    // `investigate` first). NO silent fallback: a reasoner `Err` fails the cycle
    // loudly (#1711) rather than masquerading as a no-op decision.
    let per_goal_outcomes = drive_per_goal_cycle(state, memories.brain.as_ref())?;
    if !per_goal_outcomes.is_empty() {
        tracing::info!(
            target: "simard::ooda",
            decisions = per_goal_outcomes.len(),
            reoriented = per_goal_outcomes.iter().filter(|o| o.touched_refs).count(),
            "OODA per-goal-cycle: one agentic decision per active goal",
        );
    }

    // --- Fix 3: no-progress breaker (issue #1) ---
    // Before curation, bound the *no-action* livelock: a goal that produced a
    // no-shippable-progress cycle (`NO ACTION` / rejected progress claim) has
    // its consecutive-no-action counter bumped; at the threshold the done-gate
    // runs once and the goal is resolved definitively — marked DONE (archived
    // below), DROPPED as obsolete, or BLOCKED + escalated to a tracking issue —
    // instead of being re-selected forever. Only runs with an evidence source
    // (production daemon); tests/non-daemon callers leave it `None`.
    //
    // `breaker_dropped` carries goals removed from the board as obsolete so the
    // corruption guard treats them as a legitimate departure (not a "vanished"
    // goal) and the persist step force-removes them from the snapshot.
    let breaker_dropped: Vec<String> = if let Some(source) = &memories.completion_evidence {
        // NEW-1 Prong 2 (PR #4428): reconcile PR-ref liveness BEFORE the breaker
        // classifies. A standing research goal that merged/closed its PR keeps a
        // stale `pr` wip_ref that `has_live_in_flight_ref` reads as LIVE, so the
        // breaker would classify it `ResearchInFlight` forever and it would idle
        // silently. Pruning merged/closed PR refs here makes the kind-based guard
        // sound; a genuinely-open PR survives (finding #1 intact).
        let pr_client = crate::stewardship::RealPrGhClient::new();
        reconcile_merged_prs(&mut state.active_goals, &pr_client);
        if crate::ooda_loop::no_progress::no_progress_investigation_enabled() {
            // Root-cause investigation (issue #16): before authoring any block,
            // the breaker classifies WHY a stalled goal made no shippable
            // progress and routes it down the self-resolving ladder —
            // auto-complete, heal a missing precondition, defer behind an
            // upstream, or spawn ONE guided engineer — escalating to a human
            // (WITH the concrete WHY + evidence) only as a last resort. Routing
            // is deterministic (evidence-driven): the brain is failing on this
            // goal, so it must not decide its own recovery.
            let source_ref = source.as_ref();
            let reasoner =
                crate::ooda_loop::no_progress::DeterministicNoProgressReasoner::new(source_ref);
            let healer = crate::ooda_loop::no_progress::CloneRepoHealer::new("rysweet");
            let transition_dispatcher =
                crate::ooda_loop::no_progress::QueueingEngineerDispatcher::new();

            let report = crate::ooda_loop::no_progress::apply_no_progress_breaker_investigated(
                state,
                &outcomes,
                source_ref,
                &reasoner,
                &healer,
                &transition_dispatcher,
                &crate::ooda_loop::no_progress::GhIssueFiler,
                crate::ooda_loop::no_progress::INVESTIGATED_BREAKER_THRESHOLD,
            );
            if report.is_noteworthy() {
                tracing::info!(
                    target: "simard::ooda",
                    summary = %report.log_line(),
                    "OODA no-progress breaker (root-cause) ran",
                );
            }

            // Already-blocked re-investigation (issue #17): after the
            // on-transition breaker, scan the board for goals still parked in a
            // BARE `[OODA-SAFEGUARD] … needs human review` block — parked by a
            // pre-#16 daemon build, or left bare by a reasoner error on the
            // transition cycle — and re-run the SAME WHY reasoner + ladder over
            // them, so no goal is ever stranded with a bare, unexplained block.
            // Uses its own spawn queue; both queues drain through the shared
            // dispatch below.
            let reinvestigate_dispatcher =
                crate::ooda_loop::no_progress::QueueingEngineerDispatcher::new();
            let reinvestigate_report =
                crate::ooda_loop::no_progress::reinvestigate_bare_blocked_goals(
                    state,
                    source_ref,
                    &reasoner,
                    &healer,
                    &reinvestigate_dispatcher,
                    &crate::ooda_loop::no_progress::GhIssueFiler,
                    crate::ooda_loop::no_progress::INVESTIGATED_BREAKER_THRESHOLD,
                );
            if reinvestigate_report.fired()
                || !reinvestigate_report.reinvestigated.is_empty()
                || !reinvestigate_report.investigation_errors.is_empty()
            {
                tracing::info!(
                    target: "simard::ooda",
                    summary = %reinvestigate_report.log_line(),
                    "OODA no-progress re-investigation (already-blocked) ran",
                );
            }

            let mut dropped = report.dropped.clone();
            dropped.extend(reinvestigate_report.dropped.clone());

            // Drain the guided-engineer spawn requests from BOTH passes and
            // dispatch each through the SAME `dispatch_spawn_engineer` the Act
            // phase uses (the state borrow is free now that both passes have
            // returned). Reuses the existing capability rather than building a
            // parallel spawner.
            let mut requests = transition_dispatcher.into_requests();
            requests.extend(reinvestigate_dispatcher.into_requests());
            if !requests.is_empty() {
                let brain = memories.brain.clone();
                let repo_root = memories.repo_root.clone();
                let guarded = std::sync::Mutex::new(&mut *state);
                for (goal_id, task) in requests {
                    let action = PlannedAction {
                        kind: ActionKind::AdvanceGoal,
                        goal_id: Some(goal_id.clone()),
                        description: task.clone(),
                    };
                    let outcome = crate::ooda_actions::advance_goal::spawn::dispatch_spawn_engineer(
                        &action,
                        &guarded,
                        &goal_id,
                        &task,
                        brain.as_ref(),
                        &repo_root,
                    );
                    tracing::info!(
                        target: "simard::ooda",
                        goal = %goal_id,
                        success = outcome.success,
                        detail = %outcome.detail,
                        "no-progress breaker: dispatched guided engineer via shared spawn",
                    );
                }
            }
            dropped
        } else {
            // Kill-switch: fall back to the base verify-once ladder.
            let report = crate::ooda_loop::no_progress::apply_no_progress_breaker(
                state,
                &outcomes,
                source.as_ref(),
                &crate::ooda_loop::no_progress::GhIssueFiler,
            );
            if report.fired() {
                tracing::info!(
                    target: "simard::ooda",
                    summary = %report.log_line(),
                    "OODA no-progress breaker fired",
                );
            }
            report.dropped
        }
    } else {
        Vec::new()
    };

    // --- Outcome verification: gate archival on a verified LIVE effect (#2751) ---
    // When the outcome-verify memory pair is wired (production daemon,
    // `SIMARD_OUTCOME_VERIFY` on), every completion-candidate goal is verified
    // LIVE before the archive step below can complete it. The framing invariant:
    // an ARTIFACT (a merged PR / a deploy) is NOT an OUTCOME — a goal is
    // "achieved" only once a verified live signal corroborates its real success
    // criteria (the kgpacks E2BIG regression: artifact present, effect absent).
    // Non-achieved / errored goals are re-opened in place; only rail-passed
    // `mark_achieved` goals stay `Completed` and thus archivable by the step
    // below. Absent the pair (tests / non-daemon callers), this is a no-op.
    if let (Some(ov_brain), Some(signals)) = (
        memories.outcome_verify_brain.clone(),
        memories.live_signals.clone(),
    ) {
        let reports = crate::goal_curation::verify_completion_candidates(
            &mut state.active_goals,
            ov_brain.as_ref(),
            signals.as_ref(),
            memories.completion_evidence.as_deref(),
        );
        for r in &reports {
            match &r.error {
                Some(err) => {
                    // NO-FALLBACK: a signal-source or brain error is a visible
                    // cycle failure. The goal is kept open, never archived.
                    tracing::error!(
                        target: "simard::ooda",
                        goal = %r.goal_id,
                        error = %err,
                        "OODA outcome-verify FAILED (no-fallback) — goal kept open",
                    );
                    eprintln!(
                        "[simard] OODA outcome-verify: FAILED for goal '{}' — {} (kept open, not archived)",
                        r.goal_id, err
                    );
                }
                None => {
                    eprintln!(
                        "[simard] OODA outcome-verify: goal '{}' -> {} ({} verified live signal(s))",
                        r.goal_id,
                        r.decision.variant_label(),
                        r.verified_signal_count,
                    );
                }
            }
        }
    }

    // --- Curate: archive completed goals, promote from backlog ---
    // With a deploy-aware done-gate installed (production daemon, issue #2419),
    // a completed goal archives only with hard evidence — merged PR, closed
    // issue, and (for self-affecting changes) a verified deploy; blocked goals
    // stay active with a recorded blocker. Without one (tests / non-daemon
    // callers), this is the legacy unguarded archive.
    let archived = match &memories.completion_evidence {
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

            // #2458: close the failure→lesson loop on the FU1 (#2456) external
            // signal. A goal the completion gate *refuted* (a derivable external
            // postcondition contradicted the done-claim) is a genuine,
            // non-self-judged failure — the only signal allowed to drive a
            // Reflexion-style reflection (R10). Recurring refutations on the same
            // (goal-type, error-class) distil into a `lesson:` procedure later
            // objectives recall. Best-effort: never blocks curation.
            learn_from_refuted_goals(
                &blocked,
                &*memories.memory,
                config.lesson_recurrence_threshold,
            );

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
    // in archived — or via the Fix-3 no-progress breaker DROP (`breaker_dropped`).
    // Any goal that is missing from active AND was neither archived nor dropped
    // this cycle is a corruption signal; restore the board from the snapshot.
    {
        let archived_ids: std::collections::HashSet<&str> =
            archived.iter().map(|g| g.id.as_str()).collect();
        let dropped_ids: std::collections::HashSet<&str> =
            breaker_dropped.iter().map(|s| s.as_str()).collect();
        let post_active_ids: std::collections::HashSet<&str> = state
            .active_goals
            .active
            .iter()
            .map(|g| g.id.as_str())
            .collect();
        let vanished: Vec<&str> = pre_cycle_active_ids
            .iter()
            .map(|s| s.as_str())
            .filter(|id| {
                !post_active_ids.contains(*id)
                    && !archived_ids.contains(*id)
                    && !dropped_ids.contains(*id)
            })
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
            // When goals were archived (or DROPPED by the Fix-3 breaker), use
            // save_goal_board_with_removals so that the merge-on-write step
            // cannot resurrect them from the persisted snapshot (issue #2264 —
            // archived/dropped goals reappearing every cycle).
            let mut archived_goal_ids: Vec<String> =
                archived.iter().map(|g| g.id.clone()).collect();
            archived_goal_ids.extend(breaker_dropped.iter().cloned());
            let persist_result = if archived_goal_ids.is_empty() {
                crate::goal_curation::persist_board(&state.active_goals, &*memories.memory)
            } else {
                save_goal_board_with_removals(
                    &state.active_goals,
                    &archived_goal_ids,
                    &*memories.memory,
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
                        memories
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
        memory_consolidation::consolidation_persistence(&cycle_session_id, &*memories.memory)
    {
        eprintln!("[simard] OODA consolidation: flush failed: {e}");
    }
    if let Err(e) =
        memory_consolidation::persistence_memory_operations(&cycle_session_id, &*memories.memory)
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
            &*memories.memory,
            &memories.repo_root,
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

    // Kinds whose liveness is bound to the owning engineer's session: once that
    // session is gone they are dead artifacts. `pr`/`issue` refs OUTLIVE the
    // session and are KEPT here (a merged PR is pruned by the separate per-cycle
    // PR-liveness reconcile; an `issue` is a durable record).
    const DEAD_SESSION_KINDS: [&str; 3] = ["session", "branch", "engineer"];

    for goal in board.active.iter_mut() {
        // (A) Stale-ASSIGNMENT reset (unchanged): a goal assigned to a session
        // that is no longer live has its assignment cleared and is reset to
        // `NotStarted` so it is re-dispatched.
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

        // (B) FIX-1 (OBSERVATION 1, hardening): prune dead-session
        // session/branch/engineer wip_refs for EVERY goal, keyed on whether the
        // goal's session is actually alive — NOT on whether it carries a stale
        // ASSIGNMENT. An UNASSIGNED goal (`assigned_to == None`) that still holds
        // such a ref (e.g. `clear_goal_assignment` cleared the assignment but left
        // the ref) otherwise reads as live in-flight forever through the kind-based
        // `has_live_in_flight_ref` guard and suppresses the never-idle fault.
        //
        // A `branch`/`engineer` ref_id is a branch name / engineer id, NOT a
        // session id, so the group's liveness is keyed on the goal's session
        // anchor — a live `assigned_to`, or a `session`/`engineer` ref pointing at
        // a live session. When no anchor is live the whole session-bound group is
        // dropped; when one IS live the group is preserved untouched. Pure/total:
        // no session lookup panics (plain `HashSet` membership).
        let session_is_live = goal
            .assigned_to
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| live_sessions.contains(s))
            || goal.wip_refs.iter().any(|wip| {
                let kind = wip.kind.trim();
                (kind.eq_ignore_ascii_case("session") || kind.eq_ignore_ascii_case("engineer"))
                    && live_sessions.contains(wip.ref_id.trim())
            });
        if !session_is_live {
            let before = goal.wip_refs.len();
            goal.wip_refs.retain(|wip| {
                let kind = wip.kind.trim();
                !DEAD_SESSION_KINDS
                    .iter()
                    .any(|dead| kind.eq_ignore_ascii_case(dead))
            });
            let dropped = before - goal.wip_refs.len();
            if dropped > 0 {
                eprintln!(
                    "[simard] OODA start: pruned {} dead-session wip_ref(s) for goal '{}' (owning session not live)",
                    dropped, goal.id
                );
            }
        }
    }
}

// ============================================================================
// NEW-1 Prong 2 (PR #4428): per-cycle PR-liveness reconcile — production wiring
// ============================================================================

/// Upper bound for the per-cycle open-PR fetch. MUST stay well ABOVE the repo's
/// realistic simultaneous-open-PR count.
///
/// `list_open_prs` forwards this to `gh pr list --limit`, which TRUNCATES the
/// result set. The pure prune treats the returned set as authoritative, so a
/// `pr` ref whose PR is genuinely open but BEYOND the limit would be absent from
/// the set and wrongly pruned — wiping a live in-flight ref, i.e. the exact
/// round-1 finding-#1 (F1) regression. Fail-open covers a fetch `Err` and an
/// unparseable id, but NOT a silently truncated `Ok(...)`, so this high cap is
/// the guarantee. `1000` is far above any realistic open-PR count for this repo.
const OPEN_PR_RECONCILE_LIMIT: u32 = 1000;

/// Conservative validation of a single `owner` or `repo` path segment for the
/// `gh pr list --repo` slug (FIX-2). Mirrors the shape of `repo_resolver`'s
/// `validate_repo_slug`: ASCII alphanumeric plus `.`/`_`/`-`, 1..=64 chars, no
/// `..` traversal, no leading `-`/`.`. Every slug is already passed to `gh` as a
/// discrete argv element (never string-interpolated into a shell line), but a
/// malformed segment is still rejected so we never issue a nonsense query and
/// fail open on it instead.
fn valid_repo_segment(seg: &str) -> bool {
    !seg.is_empty()
        && seg.len() <= 64
        && !seg.contains("..")
        && !seg.starts_with('-')
        && !seg.starts_with('.')
        && seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Canonical `owner/repo` slug for a goal's PR reconcile (FIX-2, OBSERVATION 2).
/// `None`, `""`, or `"simard"` (case-insensitive) fold to the canonical
/// [`crate::stewardship::TargetRepo::Simard`] slug; a value already containing
/// `'/'` is treated as an `owner/repo` slug (both components validated); a bare
/// name maps to `rysweet/{name}`. Returns `None` for any slug that fails
/// validation so the caller fails open and prunes NOTHING for that goal rather
/// than issuing a malformed `gh` query.
fn repo_slug_for_goal(goal: &crate::goal_curation::ActiveGoal) -> Option<String> {
    let simard = crate::stewardship::TargetRepo::Simard.slug();
    match goal.repo.as_deref().map(str::trim) {
        None | Some("") => Some(simard.to_string()),
        Some(s) if s.eq_ignore_ascii_case("simard") => Some(simard.to_string()),
        Some(s) if s.contains('/') => {
            let mut parts = s.splitn(2, '/');
            let owner = parts.next().unwrap_or_default();
            let repo = parts.next().unwrap_or_default();
            // `splitn(2, '/')` leaves any 3rd+ segment inside `repo`; reject it.
            if repo.contains('/') || !valid_repo_segment(owner) || !valid_repo_segment(repo) {
                None
            } else {
                Some(format!("{owner}/{repo}"))
            }
        }
        Some(name) if valid_repo_segment(name) => Some(format!("rysweet/{name}")),
        Some(_) => None,
    }
}

/// Fetch the open-PR set ONCE per distinct repo among the active goals holding a
/// `pr` ref and prune every merged/closed `pr`
/// [`crate::goal_curation::WipRef`] from the active board via the pure
/// [`crate::ooda_loop::no_progress::prune_merged_pr_refs_scoped`], BEFORE the
/// never-idle breaker classifies (NEW-1 Prong 2, PR #4428; FIX-2, OBSERVATION 2).
///
/// FIX-2 (OBSERVATION 2): each `pr` ref is reconciled against the open-PR set of
/// ITS OWN repo ([`goal_repo_slug`]), NOT a hardcoded `TargetRepo::Simard`. A
/// standing goal tracking a still-OPEN PR in another repo therefore survives
/// instead of being wrongly pruned against Simard's open set. The `gh pr list`
/// calls are DEDUPED to one per distinct repo.
///
/// Reuses the existing `gh pr list` path
/// ([`crate::stewardship::PrGhClient::list_open_prs`]) — NO new brittle shell/gh
/// parse. **Fail-open per repo**: if a repo's fetch errors it is omitted from
/// the open-set map, so [`prune_merged_pr_refs_scoped`] prunes NOTHING for that
/// repo's goals this cycle — a `gh` blip can never wipe a live PR ref (which
/// would reintroduce the round-1 finding-#1 regression). A merged-PR fault is
/// merely delayed a cycle, never suppressed forever. On a genuine empty open set
/// (`Ok([])`) that repo's `pr` refs prune (correct — nothing is open).
///
/// The `client` is injected so production wires the concrete
/// [`crate::stewardship::RealPrGhClient`] while the pure core is unit-tested
/// directly with an in-memory set (IO-free).
///
/// Fast path: when no active goal carries a `pr` wip_ref the open-PR fetch is
/// skipped entirely (the pure prune would be a no-op), avoiding the `gh pr
/// list` subprocess on the common cycle.
fn reconcile_merged_prs(
    board: &mut crate::goal_curation::GoalBoard,
    client: &dyn crate::stewardship::PrGhClient,
) {
    use std::collections::{HashMap, HashSet};

    // Skip the `gh pr list` subprocess+network round-trip entirely when no
    // active goal carries a `pr` wip_ref: the pure prune would drop nothing,
    // so the fetch is pure waste. Behaviourally identical, avoids a process
    // spawn on the common (no-PR-ref) cycle.
    let has_pr = |goal: &crate::goal_curation::ActiveGoal| {
        goal.wip_refs
            .iter()
            .any(|wip| wip.kind.trim().eq_ignore_ascii_case("pr"))
    };
    if !board.active.iter().any(has_pr) {
        return;
    }

    // Distinct, safe repo slugs among the goals that actually hold a `pr` ref —
    // dedup the `gh pr list` calls and never fetch a repo we won't prune.
    let mut slugs: Vec<String> = Vec::new();
    for goal in board.active.iter().filter(|g| has_pr(g)) {
        if let Some(slug) = repo_slug_for_goal(goal)
            && !slugs.contains(&slug)
        {
            slugs.push(slug);
        }
    }

    // Fetch each distinct repo's open-PR set once. Fail-open per repo: on `Err`
    // the repo is simply omitted, so its goals' `pr` refs are left untouched.
    let mut open_by_repo: HashMap<String, HashSet<u32>> = HashMap::new();
    for slug in slugs {
        match client.list_open_prs(&slug, OPEN_PR_RECONCILE_LIMIT) {
            Ok(open) => {
                open_by_repo.insert(slug, open.iter().map(|s| s.number).collect());
            }
            Err(e) => {
                eprintln!(
                    "[simard] merged-PR reconcile: list_open_prs failed for repo '{slug}' — \
                     skipping PR-ref prune for that repo this cycle (fail-open — never wipe a \
                     possibly-live ref): {e}"
                );
            }
        }
    }

    let pruned = crate::ooda_loop::no_progress::prune_merged_pr_refs_scoped(
        board,
        |goal| repo_slug_for_goal(goal).unwrap_or_default(),
        &open_by_repo,
    );
    for (goal_id, ref_id) in pruned {
        tracing::info!(
            target: "simard::ooda",
            goal = %goal_id,
            ref_id = %ref_id,
            "pruned merged/closed PR wip_ref (not in its repo's open-PR set)",
        );
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

/// Drive the #2458 failure→lesson loop from the FU1 (#2456) completion gate's
/// blocked-goal list.
///
/// Filters `blocked` down to the goals the gate **refuted** — a derivable
/// external postcondition contradicted the done-claim
/// ([`VerificationOutcome::Refuted`](crate::goal_curation::VerificationOutcome)),
/// the only failure signal allowed to drive a Reflexion-style reflection (R10).
/// `UnverifiedNoSignal` (nothing to check) and `Error` (could-not-verify) goals
/// are skipped: they are not genuine failures. Each refuted goal becomes a
/// [`VerifiedFailureObservation`](crate::memory_consolidation::reflection_lessons::VerifiedFailureObservation)
/// keyed by `(goal description, refuting error class)` and handed to
/// [`learn_from_verified_failures`](crate::memory_consolidation::reflection_lessons::learn_from_verified_failures).
///
/// Best-effort and side-effecting only on cognitive memory; it never returns an
/// error and never blocks curation. Extracted from the cycle body so the wiring
/// is unit-testable against an in-memory backend.
fn learn_from_refuted_goals(
    blocked: &[(
        crate::goal_curation::ActiveGoal,
        Vec<crate::goal_curation::MissingEvidence>,
    )],
    memory: &dyn crate::cognitive_memory::CognitiveMemoryOps,
    threshold: u32,
) {
    use crate::memory_consolidation::reflection_lessons::{
        VerifiedFailureObservation, learn_from_verified_failures,
    };

    let verified_failures: Vec<VerifiedFailureObservation> = blocked
        .iter()
        .filter(|(goal, missing)| {
            matches!(
                crate::goal_curation::classify_from_missing(goal, missing),
                crate::goal_curation::VerificationOutcome::Refuted
            )
        })
        .map(|(goal, missing)| {
            VerifiedFailureObservation::deduped(
                goal.description.clone(),
                crate::goal_curation::error_class_from_missing(missing),
                goal.id.clone(),
            )
        })
        .collect();

    if verified_failures.is_empty() {
        return;
    }

    let report = learn_from_verified_failures(memory, &verified_failures, threshold);
    eprintln!(
        "[simard] OODA curate: failure-reflection pass over {} refuted goal(s) — \
         {} reflection(s), {} lesson(s) distilled, {} repeat-failure(s)",
        verified_failures.len(),
        report.reflections_recorded,
        report.lessons_distilled,
        report.repeat_failures,
    );
}

// ===========================================================================
// Per-goal, per-cycle agentic decision driver (issue #4453)
//
// Runs EXACTLY ONE agentic reasoning decision per active goal per cycle
// (continue / spawn / reorient / investigate / wait / complete), replacing the
// imperative never-idle / reap / grace-window predicates with a thin
// deterministic rail that dispatches to the reasoner and executes the returned
// action. The three former imperative deciders (classify_standing_idle, the
// claim-reaper staleness sweep, and the effect board-miss) survive ONLY as
// read-only INPUTS on `PerGoalCycleCtx` — never as the decision.
// ===========================================================================

/// The observable outcome of one per-goal, per-cycle decision. Returned per
/// active goal so the caller (and the regression tests) can assert routing and
/// the A6 invariant (only `reorient`/`complete` touch refs).
#[derive(Clone, Debug)]
pub struct PerGoalDecisionOutcome {
    /// The goal this decision was made for.
    pub goal_id: String,
    /// Stable snake_case label of the chosen action (`"continue"`, `"spawn"`, …).
    pub action_label: String,
    /// The reasoner's mandatory reason for the chosen action.
    pub reason: String,
    /// `true` when the applied action performed a DESTRUCTIVE ref mutation
    /// (cleared `wip_refs` / rolled or completed the goal). Only `reorient` and
    /// `complete` do so — the code-level guarantee that a `continue` / `spawn` /
    /// `wait` / `investigate` verdict never reproduces the 70ab8541 idle→reset
    /// loop.
    pub touched_refs: bool,
}

/// Gather the DURABLE per-goal context for one per-cycle reasoning decision
/// (issue #4453). Best-effort and total: a goal id absent from the board yields
/// a defaulted ctx (never panics). Reads only in-memory durable state — never a
/// live worker's mere presence as the decision input. The three demoted
/// imperative deciders are surfaced here as read-only signals.
pub(crate) fn gather_per_goal_cycle_ctx(
    state: &OodaState,
    goal_id: &str,
) -> crate::ooda_brain::PerGoalCycleCtx {
    use crate::ooda_brain::PerGoalCycleCtx;

    let Some(goal) = state.active_goals.active.iter().find(|g| g.id == goal_id) else {
        return PerGoalCycleCtx {
            goal_id: goal_id.to_string(),
            cycle_number: state.cycle_count,
            ..PerGoalCycleCtx::default()
        };
    };

    // Durable in-flight work (NOT live-worktree presence).
    let open_pr_refs: Vec<String> = goal
        .wip_refs
        .iter()
        .filter(|w| w.kind.trim().eq_ignore_ascii_case("pr"))
        .map(|w| w.ref_id.clone())
        .collect();
    let worker_present = state.engineer_worktrees.contains_key(goal_id);

    // DEMOTED decider #1 — classify_standing_idle: a standing goal that looks
    // idle (no live in-flight ref) becomes a SIGNAL, not a roll.
    let standing_idle_signal = matches!(
        crate::ooda_loop::no_progress::classify_standing_idle(goal),
        Some(crate::ooda_loop::no_progress::StandingIdle::ResearchFault { .. })
    ) || (goal.is_perpetual() && !goal.has_live_in_flight_ref());

    // DEMOTED decider #2 — claim-reaper staleness: when a worker claim is
    // EXPECTED (assignment or engineer/session/branch ref) but no live worktree
    // is present, surface how long since the claim was last observed alive as a
    // read-only INPUT. STALE_SECS survives only as the threshold that populates
    // this field, never as the reap trigger; no new SIMARD_*_SECS is added.
    let expects_worker = goal.assigned_to.is_some()
        || goal.wip_refs.iter().any(|w| {
            let kind = w.kind.trim();
            kind.eq_ignore_ascii_case("engineer")
                || kind.eq_ignore_ascii_case("session")
                || kind.eq_ignore_ascii_case("branch")
        });
    let stale_claim_secs = if expects_worker && !worker_present {
        Some(claim_age_secs(goal))
    } else {
        None
    };

    PerGoalCycleCtx {
        goal_id: goal.id.clone(),
        goal_description: goal.description.clone(),
        goal_status: goal.status.to_string(),
        cycle_number: state.cycle_count,
        history_summary: goal.current_activity.clone().unwrap_or_default(),
        effect_jobs_in_flight: 0,
        open_pr_refs,
        last_outcomes: Vec::new(),
        wip_ref_count: goal.wip_refs.len() as u32,
        worker_present,
        worker_log_tail: String::new(),
        standing_idle_signal,
        stale_claim_secs,
        // DEMOTED decider #3 — effect board-miss: gathered by the caller from
        // the durable effect-dispatch ledger when available; defaulted false
        // here so the pure gather stays IO-free.
        effect_board_missed: false,
    }
}

/// Seconds since a goal's worker claim was last observed making durable
/// progress. `u64::MAX` means "a claim is expected but was never observed
/// progressing" (no `last_progress_update_at` recorded). A FACT fed to the
/// reasoner — never a reap threshold.
fn claim_age_secs(goal: &crate::goal_curation::ActiveGoal) -> u64 {
    match goal.last_progress_update_at {
        Some(ts) => (chrono::Utc::now() - ts).num_seconds().max(0) as u64,
        None => u64::MAX,
    }
}

/// Run EXACTLY ONE agentic reasoning decision per active goal for this cycle
/// (issue #4453), routing each outcome through the thin deterministic state
/// rail [`crate::ooda_brain::apply_per_goal_action_to_state`] and recording a
/// [`BrainJudgmentRecord`] for EVERY goal (an action + a reason each cycle —
/// none left idle without both).
///
/// NO silent fallback: an `Err` from the reasoner surfaces as a cycle failure
/// (#1711). Destructive ref mutation is reachable only via a reasoned
/// `reorient`/`complete` (a worker-health concern goes through `investigate`
/// first) — never a threshold/counter/grace-window.
pub fn drive_per_goal_cycle<B: crate::ooda_brain::OodaBrain + ?Sized>(
    state: &mut OodaState,
    brain: &B,
) -> SimardResult<Vec<PerGoalDecisionOutcome>> {
    use crate::ooda_brain::{
        BrainJudgmentRecord, apply_per_goal_action_to_state, push_brain_judgment,
    };

    // Snapshot the active goal ids first (cf. the pre-cycle active-id snapshot)
    // so the per-goal `apply_*` mutation cannot invalidate the iteration.
    let active_ids: Vec<String> = state
        .active_goals
        .active
        .iter()
        .map(|g| g.id.clone())
        .collect();

    let mut outcomes = Vec::with_capacity(active_ids.len());
    for goal_id in active_ids {
        let ctx = gather_per_goal_cycle_ctx(state, &goal_id);
        // One agentic decision for this goal this cycle. Err => cycle failure.
        let action = brain.decide_per_goal_cycle(&ctx)?;
        let touched_refs = action.mutates_refs();

        // Thin deterministic rail: apply the chosen action to in-memory state.
        let detail = apply_per_goal_action_to_state(&action, state, &goal_id);

        // Record the judgment for EVERY goal — an action + a reason each cycle.
        push_brain_judgment(BrainJudgmentRecord::from_per_goal_cycle(
            &goal_id, &action, false, "",
        ));

        eprintln!("[simard] OODA per-goal-cycle: {} -> {}", goal_id, detail);

        outcomes.push(PerGoalDecisionOutcome {
            goal_id,
            action_label: action.variant_label().to_string(),
            reason: action.reason().to_string(),
            touched_refs,
        });
    }
    Ok(outcomes)
}

#[cfg(test)]
mod tests_refuted_lessons {
    use super::learn_from_refuted_goals;
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    use crate::goal_curation::{ActiveGoal, GoalProgress, MissingEvidence, WipRef};
    use crate::memory_consolidation::reflection_lessons::{
        LESSON_RECURRENCE_THRESHOLD, has_lesson_for, lesson_name,
    };

    /// A goal carrying a tracked PR `wip_ref` — so `has_derivable_signal` holds
    /// and a `PrNotMerged` blocker classifies as `Refuted` (a real failure
    /// signal), not `UnverifiedNoSignal`. `id` is the per-occurrence dedup key.
    fn refuted_goal(id: &str, desc: &str) -> ActiveGoal {
        ActiveGoal {
            labels: Vec::new(),
            parent_goal_id: None,
            priority_explicit: false,
            repo: None,
            id: id.to_string(),
            description: desc.to_string(),
            priority: 1,
            status: GoalProgress::Completed,
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![WipRef {
                kind: "pr".to_string(),
                ref_id: "4242".to_string(),
                label: "PR #4242".to_string(),
                url: None,
            }],
            last_progress_update_at: None,
        }
    }

    const DESC: &str = "Ship the websocket reconnect backoff for the dashboard";

    /// A single refuted goal records a reflection but distils no lesson (below
    /// the recurrence threshold). The cycle glue selected it as a real failure.
    #[test]
    fn one_refuted_goal_reflects_but_no_lesson_yet() {
        let mem = LibraryCognitiveMemory::in_memory().expect("db");
        let blocked = vec![(refuted_goal("g1", DESC), vec![MissingEvidence::PrNotMerged])];
        learn_from_refuted_goals(&blocked, &mem, LESSON_RECURRENCE_THRESHOLD);
        assert!(
            !has_lesson_for(&mem, DESC, "pr_not_merged").expect("ok"),
            "one refutation is not yet a lesson"
        );
    }

    /// **Distinct** goals of the same type, each refuted, accumulate a recurrence
    /// that distils a recallable lesson — the end-to-end loop the OODA curate
    /// phase drives across attempts.
    #[test]
    fn recurring_refutation_distills_recallable_lesson() {
        let mem = LibraryCognitiveMemory::in_memory().expect("db");
        for i in 0..LESSON_RECURRENCE_THRESHOLD {
            let id = format!("ship-attempt-{i}");
            let blocked = vec![(refuted_goal(&id, DESC), vec![MissingEvidence::PrNotMerged])];
            learn_from_refuted_goals(&blocked, &mem, LESSON_RECURRENCE_THRESHOLD);
        }
        assert!(
            has_lesson_for(&mem, DESC, "pr_not_merged").expect("ok"),
            "a recurring refutation across distinct goals must become a lesson"
        );
        let expected = lesson_name(DESC, "pr_not_merged");
        let recalled = mem.recall_procedure("reconnect", 10).expect("recall");
        assert!(
            recalled.iter().any(|p| p.name == expected),
            "lesson {expected:?} must surface for a related objective; got {:?}",
            recalled.iter().map(|p| &p.name).collect::<Vec<_>>()
        );
    }

    /// The **same** goal refuted across many cycles is one occurrence — it must
    /// reflect exactly once and never distil a lesson on its own. This is the
    /// bounded-growth guard: a normal in-flight PR (blocked but not yet merged)
    /// cannot accrue an unbounded reflection trail or a per-cycle lesson.
    #[test]
    fn same_blocked_goal_across_cycles_reflects_once_and_no_lesson() {
        let mem = LibraryCognitiveMemory::in_memory().expect("db");
        for _ in 0..LESSON_RECURRENCE_THRESHOLD + 3 {
            let blocked = vec![(
                refuted_goal("g-stuck", DESC),
                vec![MissingEvidence::PrNotMerged],
            )];
            learn_from_refuted_goals(&blocked, &mem, LESSON_RECURRENCE_THRESHOLD);
        }
        assert!(
            !has_lesson_for(&mem, DESC, "pr_not_merged").expect("ok"),
            "one goal stuck across many cycles is a single occurrence, never a lesson"
        );
    }

    /// A goal with **no** derivable signal classifies as `UnverifiedNoSignal`,
    /// not `Refuted`, so the glue must skip it entirely (no lesson accrues). A
    /// non-Simard repo with no PR/issue ref is not self-affecting, so no external
    /// postcondition is derivable.
    #[test]
    fn no_signal_goal_is_skipped() {
        let mem = LibraryCognitiveMemory::in_memory().expect("db");
        let no_signal_goal = |i: u32| {
            let mut g = refuted_goal(&format!("ns-{i}"), DESC);
            g.repo = Some("some-other-service".to_string()); // not Simard ⇒ not self-affecting
            g.wip_refs.clear(); // no PR/issue ⇒ nothing external to verify
            g
        };
        // Even across enough distinct goals to clear the threshold, nothing
        // accrues because none is a genuine (refuted) failure.
        for i in 0..LESSON_RECURRENCE_THRESHOLD + 1 {
            let blocked = vec![(no_signal_goal(i), vec![MissingEvidence::PrNotMerged])];
            learn_from_refuted_goals(&blocked, &mem, LESSON_RECURRENCE_THRESHOLD);
        }
        assert!(
            !has_lesson_for(&mem, DESC, "pr_not_merged").expect("ok"),
            "an unverifiable goal must never produce a lesson"
        );
    }

    /// A `CouldNotVerify` blocker classifies as `Error`, never `Refuted` — the
    /// glue skips it so an unverifiable cycle never fabricates a failure.
    #[test]
    fn could_not_verify_goal_is_skipped() {
        let mem = LibraryCognitiveMemory::in_memory().expect("db");
        for i in 0..LESSON_RECURRENCE_THRESHOLD + 1 {
            let blocked = vec![(
                refuted_goal(&format!("cnv-{i}"), DESC),
                vec![MissingEvidence::CouldNotVerify {
                    detail: "gh timeout".to_string(),
                }],
            )];
            learn_from_refuted_goals(&blocked, &mem, LESSON_RECURRENCE_THRESHOLD);
        }
        assert!(
            !has_lesson_for(&mem, DESC, "refuted_unknown").expect("ok"),
            "an Error outcome must not drive the failure→lesson loop"
        );
    }
}

#[cfg(test)]
mod tests_sweep {
    use std::collections::HashSet;

    use super::sweep_stale_assignments_with_sessions;
    use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, add_active_goal};

    fn make_goal(id: &str, session: Option<&str>) -> ActiveGoal {
        ActiveGoal {
            labels: Vec::new(),
            parent_goal_id: None,
            priority_explicit: false,
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

    /// NEW-1 Prong 1 (PR #4428): a dead session's assignment sweep must ALSO drop
    /// that goal's session/branch/engineer wip_refs — they belong to the dead
    /// engineer — so `has_live_in_flight_ref` no longer reads them as live and the
    /// never-idle breaker can fault. Durable `pr`/`issue` refs are KEPT: they
    /// outlive the session, and a merged PR is pruned by the separate per-cycle
    /// PR-liveness reconcile (`prune_merged_pr_refs`), not here.
    #[test]
    fn drops_dead_session_wip_refs_but_keeps_pr_and_issue() {
        use crate::goal_curation::WipRef;
        let wref = |kind: &str, id: &str| WipRef {
            kind: kind.to_string(),
            ref_id: id.to_string(),
            label: format!("{kind} {id}"),
            url: None,
        };
        let mut board = GoalBoard::new();
        let mut goal = make_goal("g1", Some("dead-session"));
        goal.wip_refs = vec![
            wref("session", "dead-session"),
            wref("branch", "feat/x"),
            wref("engineer", "engineer-7"),
            wref("pr", "42"),
            wref("issue", "100"),
        ];
        add_active_goal(&mut board, goal).unwrap();

        sweep_stale_assignments_with_sessions(&mut board, &live(&["other"]));

        let goal = &board.active[0];
        assert!(
            goal.assigned_to.is_none(),
            "assignment must be cleared for the dead session"
        );
        let kinds: Vec<&str> = goal.wip_refs.iter().map(|w| w.kind.as_str()).collect();
        assert!(
            !kinds.contains(&"session"),
            "the dead session's session ref must be dropped, got {kinds:?}"
        );
        assert!(
            !kinds.contains(&"branch"),
            "the dead session's branch ref must be dropped, got {kinds:?}"
        );
        assert!(
            !kinds.contains(&"engineer"),
            "the dead session's engineer ref must be dropped, got {kinds:?}"
        );
        assert!(
            kinds.contains(&"pr"),
            "a durable PR ref must be kept (pruned separately by the PR-liveness reconcile), got {kinds:?}"
        );
        assert!(
            kinds.contains(&"issue"),
            "a durable issue ref must be kept, got {kinds:?}"
        );
    }

    /// A LIVE session must keep its session/branch wip_refs untouched — the ref
    /// drop is scoped strictly to DEAD sessions.
    #[test]
    fn keeps_wip_refs_for_live_session() {
        use crate::goal_curation::WipRef;
        let wref = |kind: &str, id: &str| WipRef {
            kind: kind.to_string(),
            ref_id: id.to_string(),
            label: format!("{kind} {id}"),
            url: None,
        };
        let mut board = GoalBoard::new();
        let mut goal = make_goal("g1", Some("live-session"));
        goal.wip_refs = vec![wref("session", "live-session"), wref("branch", "feat/y")];
        add_active_goal(&mut board, goal).unwrap();

        sweep_stale_assignments_with_sessions(&mut board, &live(&["live-session"]));

        let goal = &board.active[0];
        assert_eq!(goal.assigned_to.as_deref(), Some("live-session"));
        assert_eq!(
            goal.wip_refs.len(),
            2,
            "a live session's wip_refs must be preserved untouched"
        );
    }

    /// FIX-1 (OBSERVATION 1, hardening): the dead-session ref prune must run for
    /// an UNASSIGNED goal too, keyed on whether the session is actually alive —
    /// NOT gated on the goal carrying a stale ASSIGNMENT.
    ///
    /// Scenario: a standing *research* goal with `assigned_to == None` whose only
    /// wip_ref is an `engineer` ref for a session that is NOT in the live set
    /// (e.g. `clear_goal_assignment` cleared the assignment but left the ref).
    /// After the sweep the dead ref must be dropped, so
    /// [`ActiveGoal::has_live_in_flight_ref`] reads `false` and the never-idle
    /// breaker classifies it as a [`ResearchFault`] — the goal is re-oriented,
    /// stays active, and is never wrongly held as `ResearchInFlight` forever.
    ///
    /// **RED before FIX-1:** the pre-fix sweep prunes session/branch/engineer
    /// refs only INSIDE the `if is_stale` block, which requires a stale
    /// *assignment*. An unassigned goal (`assigned_to == None`) is never stale,
    /// so the dead-session ref survives, `has_live_in_flight_ref()` stays `true`,
    /// and `classify_standing_idle` returns `ResearchInFlight` (the fault is
    /// suppressed) — this assertion fails.
    #[test]
    fn prunes_dead_session_ref_for_unassigned_standing_research_goal() {
        use crate::goal_curation::WipRef;
        use crate::ooda_loop::no_progress::{
            ResearchIdleFault, StandingIdle, classify_standing_idle,
        };

        // Unassigned (assigned_to == None) standing research goal.
        let mut goal = make_goal("standing-research", None);
        goal.description = "[standing] improve memory recall quality".to_string();
        // Its only ref is a dead engineer/session artifact (session not live).
        goal.wip_refs = vec![WipRef {
            kind: "engineer".to_string(),
            ref_id: "dead-session-xyz".to_string(),
            label: "engineer dead-session-xyz".to_string(),
            url: None,
        }];

        let mut board = GoalBoard::new();
        add_active_goal(&mut board, goal).unwrap();
        assert!(
            board.active[0].is_standing_research_goal(),
            "precondition: the test goal must read as a standing research goal"
        );
        assert!(
            board.active[0].has_live_in_flight_ref(),
            "precondition: before the sweep the dead-session ref reads as live in-flight"
        );

        // A non-empty live-session set that does NOT contain the dead session.
        sweep_stale_assignments_with_sessions(&mut board, &live(&["some-other-live-session"]));

        let goal = &board.active[0];
        assert!(
            goal.wip_refs.is_empty(),
            "the dead-session engineer ref must be pruned even though the goal is UNASSIGNED, got {:?}",
            goal.wip_refs
        );
        assert!(
            !goal.has_live_in_flight_ref(),
            "with the dead ref pruned the goal must no longer read as holding a live in-flight ref"
        );
        assert_eq!(
            classify_standing_idle(goal),
            Some(StandingIdle::ResearchFault {
                fault: ResearchIdleFault::NoNovelActionProduced,
            }),
            "an idle standing research goal with no live ref must classify as a ResearchFault (active, re-oriented, never Blocked)"
        );
    }

    /// FIX-1 (OBSERVATION 1, hardening) — keyed-not-drop-all divergence: the
    /// dead-session prune is keyed on whether the goal's OWNING session is alive,
    /// NOT a blanket drop of every working ref whenever the assignment is stale.
    ///
    /// Scenario: a goal whose ASSIGNMENT is stale (`assigned_to = "dead-sess"`,
    /// absent from `live_sessions`) but which ALSO carries a `session` ref for a
    /// DIFFERENT, still-live session (`"live-sess"`). After the sweep the stale
    /// assignment is cleared and the goal reset to `NotStarted` (branch (a),
    /// unchanged) — but the live `session` ref MUST survive, so the goal still
    /// reads `has_live_in_flight_ref() == true` (its live engineer session is not
    /// discarded as collateral of the stale assignment pointer).
    ///
    /// **RED against NEW-1's drop-all:** the base #4428 sweep dropped ALL
    /// session/branch/engineer refs inside the `if is_stale` block, so the live
    /// ref would be wiped and `has_live_in_flight_ref()` would read `false`.
    #[test]
    fn keeps_live_session_ref_when_assignment_is_stale() {
        use crate::goal_curation::WipRef;
        let mut board = GoalBoard::new();
        let mut goal = make_goal("g1", Some("dead-sess"));
        goal.wip_refs = vec![WipRef {
            kind: "session".to_string(),
            ref_id: "live-sess".to_string(),
            label: "session live-sess".to_string(),
            url: None,
        }];
        add_active_goal(&mut board, goal).unwrap();

        sweep_stale_assignments_with_sessions(&mut board, &live(&["live-sess"]));

        let goal = &board.active[0];
        assert!(
            goal.assigned_to.is_none(),
            "the stale assignment must be cleared"
        );
        assert!(
            matches!(goal.status, GoalProgress::NotStarted),
            "a stale-assignment goal must be reset to NotStarted"
        );
        assert_eq!(
            goal.wip_refs.len(),
            1,
            "the ref for a DIFFERENT, still-live session must survive, got {:?}",
            goal.wip_refs
        );
        assert!(
            goal.has_live_in_flight_ref(),
            "a live session ref must keep the goal reading as holding live in-flight work"
        );
    }
}

#[cfg(test)]
mod tests_reconcile_fetch_guard {
    use std::cell::Cell;

    use super::reconcile_merged_prs;
    use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, WipRef, add_active_goal};
    use crate::stewardship::merge_authority::{OpenPrSummary, PrGhClient, PrSnapshot};

    /// Fake that records how many times `list_open_prs` (the `gh pr list`
    /// subprocess in prod) was invoked, so we can assert the fast-path guard
    /// skips the fetch when there is nothing to reconcile.
    struct CountingClient {
        calls: Cell<u32>,
        open: Vec<u32>,
    }

    impl PrGhClient for CountingClient {
        fn view_pr(&self, _repo: &str, _pr_number: u32) -> crate::error::SimardResult<PrSnapshot> {
            unimplemented!("not exercised by the reconcile fetch-guard tests")
        }
        fn squash_merge(&self, _repo: &str, _pr_number: u32) -> crate::error::SimardResult<()> {
            unimplemented!("not exercised by the reconcile fetch-guard tests")
        }
        fn list_open_prs(
            &self,
            _repo: &str,
            _limit: u32,
        ) -> crate::error::SimardResult<Vec<OpenPrSummary>> {
            self.calls.set(self.calls.get() + 1);
            Ok(self
                .open
                .iter()
                .map(|n| OpenPrSummary {
                    number: *n,
                    ..Default::default()
                })
                .collect())
        }
    }

    fn goal_with_refs(id: &str, refs: Vec<WipRef>) -> ActiveGoal {
        ActiveGoal {
            labels: Vec::new(),
            parent_goal_id: None,
            priority_explicit: false,
            repo: None,
            id: id.to_string(),
            description: format!("Goal {id}"),
            priority: 1,
            status: GoalProgress::InProgress { percent: 50 },
            assigned_to: None,
            current_activity: None,
            wip_refs: refs,
            last_progress_update_at: None,
        }
    }

    fn wref(kind: &str, ref_id: &str) -> WipRef {
        WipRef {
            kind: kind.to_string(),
            ref_id: ref_id.to_string(),
            label: format!("{kind} {ref_id}"),
            url: None,
        }
    }

    /// No active goal carries a `pr` wip_ref → the `gh pr list` fetch must be
    /// SKIPPED entirely (the pure prune would be a no-op).
    #[test]
    fn skips_open_pr_fetch_when_no_pr_ref() {
        let mut board = GoalBoard::new();
        add_active_goal(
            &mut board,
            goal_with_refs("g1", vec![wref("branch", "feat/x"), wref("issue", "100")]),
        )
        .unwrap();
        let client = CountingClient {
            calls: Cell::new(0),
            open: vec![],
        };

        reconcile_merged_prs(&mut board, &client);

        assert_eq!(
            client.calls.get(),
            0,
            "list_open_prs (the gh subprocess) must not run when no goal has a pr ref"
        );
        assert_eq!(
            board.active[0].wip_refs.len(),
            2,
            "non-pr refs must be untouched"
        );
    }

    /// An empty active board → also no fetch.
    #[test]
    fn skips_open_pr_fetch_on_empty_board() {
        let mut board = GoalBoard::new();
        let client = CountingClient {
            calls: Cell::new(0),
            open: vec![],
        };

        reconcile_merged_prs(&mut board, &client);

        assert_eq!(client.calls.get(), 0, "no goals → no fetch");
    }

    /// A `pr` wip_ref present → fetch runs exactly once and merged/closed refs
    /// (not in the open set) are pruned while a genuinely-open one survives.
    #[test]
    fn fetches_once_and_prunes_when_pr_ref_present() {
        let mut board = GoalBoard::new();
        add_active_goal(
            &mut board,
            goal_with_refs(
                "g1",
                vec![wref("pr", "42"), wref("pr", "99"), wref("branch", "feat/x")],
            ),
        )
        .unwrap();
        // 42 is open, 99 is merged/closed (absent).
        let client = CountingClient {
            calls: Cell::new(0),
            open: vec![42],
        };

        reconcile_merged_prs(&mut board, &client);

        assert_eq!(
            client.calls.get(),
            1,
            "the open-PR fetch must run exactly once per cycle when a pr ref exists"
        );
        let kinds_ids: Vec<(&str, &str)> = board.active[0]
            .wip_refs
            .iter()
            .map(|w| (w.kind.as_str(), w.ref_id.as_str()))
            .collect();
        assert!(
            kinds_ids.contains(&("pr", "42")),
            "the open PR ref must survive, got {kinds_ids:?}"
        );
        assert!(
            !kinds_ids.contains(&("pr", "99")),
            "the merged/closed PR ref must be pruned, got {kinds_ids:?}"
        );
        assert!(
            kinds_ids.contains(&("branch", "feat/x")),
            "non-pr refs must be untouched, got {kinds_ids:?}"
        );
    }
}

/// FIX-2 (OBSERVATION 2): the merged-PR reconcile must query each goal's OWN
/// repo, not a hardcoded `TargetRepo::Simard`. These tests drive the new,
/// repo-scoped [`reconcile_merged_prs`] (which derives the `owner/repo` slug
/// per active goal and prunes each `pr` ref only against ITS OWN repo's open
/// set) with a repo-keyed fake so the scoping is asserted directly, IO-free.
#[cfg(test)]
mod tests_reconcile_repo_scope {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::reconcile_merged_prs;
    use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, WipRef, add_active_goal};
    use crate::stewardship::merge_authority::{OpenPrSummary, PrGhClient, PrSnapshot};

    /// Fake keyed by the `owner/repo` slug: `list_open_prs(repo, _)` returns the
    /// open-PR set registered for THAT repo (empty for an unregistered repo) and
    /// records the per-repo call count, so we can assert each distinct goal repo
    /// is queried against its OWN slug (deduped).
    struct RepoKeyedClient {
        open_by_repo: HashMap<String, Vec<u32>>,
        calls_by_repo: RefCell<HashMap<String, u32>>,
    }

    impl PrGhClient for RepoKeyedClient {
        fn view_pr(&self, _repo: &str, _pr_number: u32) -> crate::error::SimardResult<PrSnapshot> {
            unimplemented!("not exercised by the repo-scope reconcile test")
        }
        fn squash_merge(&self, _repo: &str, _pr_number: u32) -> crate::error::SimardResult<()> {
            unimplemented!("not exercised by the repo-scope reconcile test")
        }
        fn list_open_prs(
            &self,
            repo: &str,
            _limit: u32,
        ) -> crate::error::SimardResult<Vec<OpenPrSummary>> {
            *self
                .calls_by_repo
                .borrow_mut()
                .entry(repo.to_string())
                .or_insert(0) += 1;
            let open = self.open_by_repo.get(repo).cloned().unwrap_or_default();
            Ok(open
                .into_iter()
                .map(|number| OpenPrSummary {
                    number,
                    ..Default::default()
                })
                .collect())
        }
    }

    fn goal_in_repo(id: &str, repo: Option<&str>, refs: Vec<WipRef>) -> ActiveGoal {
        ActiveGoal {
            labels: Vec::new(),
            parent_goal_id: None,
            priority_explicit: false,
            repo: repo.map(str::to_string),
            id: id.to_string(),
            description: format!("Goal {id}"),
            priority: 1,
            status: GoalProgress::InProgress { percent: 50 },
            assigned_to: None,
            current_activity: None,
            wip_refs: refs,
            last_progress_update_at: None,
        }
    }

    fn pr_ref(number: &str) -> WipRef {
        WipRef {
            kind: "pr".to_string(),
            ref_id: number.to_string(),
            label: format!("pr {number}"),
            url: None,
        }
    }

    /// A goal in a repo OTHER than Simard, holding an OPEN pr ref, must have that
    /// ref reconciled against ITS OWN repo — so the open PR survives — while a
    /// Simard goal's merged pr ref is still pruned (NEW-1 not regressed).
    ///
    /// **RED before FIX-2:** the pre-fix `reconcile_merged_prs` hardcodes
    /// `TargetRepo::Simard` and ignores `goal.repo`, so the other-repo goal's
    /// OPEN pr (#500) is checked against Simard's open set (which does not
    /// contain it) and wrongly pruned — the exact F1-style false prune this fix
    /// closes. (The pre-fix signature also still takes a `repo` argument, so this
    /// test — written against the target repo-scoped signature — does not even
    /// compile until FIX-2 lands.)
    #[test]
    fn scopes_open_pr_reconcile_to_each_goals_own_repo() {
        let mut board = GoalBoard::new();
        // Simard goal (repo: None → rysweet/Simard) with a MERGED pr (#77).
        add_active_goal(
            &mut board,
            goal_in_repo("simard-goal", None, vec![pr_ref("77")]),
        )
        .unwrap();
        // Other-repo goal (repo: Some("other") → rysweet/other) with an OPEN pr (#500).
        add_active_goal(
            &mut board,
            goal_in_repo("other-goal", Some("other"), vec![pr_ref("500")]),
        )
        .unwrap();

        let mut open_by_repo = HashMap::new();
        // Simard: #77 is NOT open (merged) → must be pruned.
        open_by_repo.insert("rysweet/Simard".to_string(), Vec::new());
        // rysweet/other: #500 IS open → must survive.
        open_by_repo.insert("rysweet/other".to_string(), vec![500]);
        let client = RepoKeyedClient {
            open_by_repo,
            calls_by_repo: RefCell::new(HashMap::new()),
        };

        reconcile_merged_prs(&mut board, &client);

        let simard = board.active.iter().find(|g| g.id == "simard-goal").unwrap();
        assert!(
            !simard.wip_refs.iter().any(|w| w.kind == "pr"),
            "the Simard goal's merged PR ref must still be pruned (NEW-1 intact), got {:?}",
            simard.wip_refs
        );

        let other = board.active.iter().find(|g| g.id == "other-goal").unwrap();
        assert!(
            other
                .wip_refs
                .iter()
                .any(|w| w.kind == "pr" && w.ref_id == "500"),
            "the other-repo goal's OPEN PR ref must survive — it must be queried against its OWN repo (rysweet/other), got {:?}",
            other.wip_refs
        );

        // Each distinct goal repo is queried against its own slug, deduped.
        let calls = client.calls_by_repo.borrow();
        assert_eq!(
            calls.get("rysweet/other").copied(),
            Some(1),
            "the other-repo goal's PR must be checked against rysweet/other exactly once, calls={calls:?}"
        );
        assert_eq!(
            calls.get("rysweet/Simard").copied(),
            Some(1),
            "the Simard goal's PR must be checked against rysweet/Simard exactly once, calls={calls:?}"
        );
    }
}

#[cfg(test)]
mod tests_board_integrity {
    use super::*;
    use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, add_active_goal};

    fn make_goal(id: &str, desc: &str) -> ActiveGoal {
        ActiveGoal {
            labels: Vec::new(),
            parent_goal_id: None,
            priority_explicit: false,
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
            labels: Vec::new(),
            parent_goal_id: None,
            priority_explicit: false,
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
