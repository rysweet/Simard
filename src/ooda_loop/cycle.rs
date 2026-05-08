//! Outer OODA cycle implementation extracted from mod.rs (#1266).

use std::time::Instant;

use crate::error::{SimardError, SimardResult};
use crate::goal_curation::load_goal_board;
use crate::gym_bridge::ScoreDimensions;
use crate::gym_scoring::GymSuiteScore;
use crate::memory_consolidation;
use crate::memory_consolidation::preparation_memory_operations;
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
        state.active_goals = board;
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
    match check_meeting_handoffs(&mut state.active_goals, &handoff_dir) {
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
    }

    // --- Observe ---
    state.current_phase = OodaPhase::Observe;
    eprintln!("[simard] OODA cycle: entering Observe phase");
    let observation = observe(state, bridges)?;
    eprintln!("[simard] OODA cycle: Observe complete");

    // --- Prepare: gather relevant context from cognitive memory ---
    // Build an objective summary from active goals so memory retrieval is targeted.
    let objective_summary: String = state
        .active_goals
        .active
        .iter()
        .map(|g| g.description.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    // Reuse cycle_session_id established above — the entire cycle is one logical session.
    let ctx =
        preparation_memory_operations(&objective_summary, &cycle_session_id, &*bridges.memory)?;
    eprintln!(
        "[simard] OODA cycle: prepared context ({} facts, {} triggers, {} procedures)",
        ctx.relevant_facts.len(),
        ctx.triggered_prospectives.len(),
        ctx.recalled_procedures.len(),
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

    // --- Memory consolidation: preparation (cross-session recall) ---
    if let Err(e) = memory_consolidation::preparation_memory_operations(
        &cycle_objective,
        &cycle_session_id,
        &*bridges.memory,
    ) {
        eprintln!("[simard] OODA consolidation: preparation failed: {e}");
    }

    // --- Decide ---
    state.current_phase = OodaPhase::Decide;
    eprintln!("[simard] OODA cycle: entering Decide phase");
    let planned_actions = match bridges.decide_brain.as_ref() {
        Some(brain) => decide_with_brain(&priorities, config, brain.as_ref())?,
        None => decide(&priorities, config)?,
    };
    eprintln!(
        "[simard] OODA cycle: Decide complete ({} actions)",
        planned_actions.len()
    );

    // --- Act ---
    state.current_phase = OodaPhase::Act;
    eprintln!("[simard] OODA cycle: entering Act phase");
    let act_start = Instant::now();
    let outcomes = act(&planned_actions, bridges, state)?;
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
    let archived = crate::goal_curation::archive_completed(&mut state.active_goals);
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

    // Persist the updated board to cognitive memory and disk (best-effort).
    if let Err(e) = crate::goal_curation::persist_board(&state.active_goals, &*bridges.memory) {
        eprintln!("[simard] OODA curate: failed to persist goal board: {e}");
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

#[cfg(test)]
mod tests_sweep {
    use std::collections::HashSet;

    use super::sweep_stale_assignments_with_sessions;
    use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, add_active_goal};

    fn make_goal(id: &str, session: Option<&str>) -> ActiveGoal {
        ActiveGoal {
            id: id.to_string(),
            description: format!("Goal {id}"),
            priority: 1,
            status: GoalProgress::InProgress { percent: 50 },
            assigned_to: session.map(str::to_string),
            current_activity: None,
            wip_refs: vec![],
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
