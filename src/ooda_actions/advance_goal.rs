//! AdvanceGoal dispatch — routing, subordinate heartbeat, and session-based advancement.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent_roles::AgentRole;
use crate::agent_supervisor::{
    HeartbeatStatus, SubordinateConfig, check_heartbeat, spawn_subordinate,
};
use crate::goal_curation::{GoalProgress, update_goal_progress};
use crate::identity_composition::max_subordinate_depth;
use crate::ooda_loop::{ActionOutcome, OodaBridges, OodaState, PlannedAction};

use super::goal_session::GoalAction;
use super::make_outcome;

/// AdvanceGoal: progress the target goal on the board.
///
/// If the goal has a subordinate assigned, checks the subordinate's
/// heartbeat via the supervisor. If a base-type session is available
/// (e.g. RustyClawd), delegates the goal to the agent via `run_turn`
/// for real autonomous work. Otherwise, falls back to bumping the
/// progress percentage.
pub(super) fn dispatch_advance_goal(
    action: &PlannedAction,
    bridges: &mut OodaBridges,
    state: &mut OodaState,
) -> ActionOutcome {
    let goal_id = match &action.goal_id {
        Some(id) => id.clone(),
        None => {
            return make_outcome(action, false, "advance-goal requires a goal_id".to_string());
        }
    };

    // Find the goal on the board.
    let goal = match state.active_goals.active.iter().find(|g| g.id == goal_id) {
        Some(g) => g.clone(),
        None => {
            return make_outcome(
                action,
                false,
                format!("goal '{goal_id}' not found on active board"),
            );
        }
    };

    // If the goal has a subordinate, check heartbeat.
    if let Some(ref sub_name) = goal.assigned_to {
        return advance_goal_with_subordinate(action, bridges, state, &goal_id, sub_name);
    }

    // Blocked and completed goals short-circuit before session dispatch.
    match &goal.status {
        GoalProgress::Blocked(reason) => {
            return make_outcome(
                action,
                false,
                format!("goal '{goal_id}' is blocked: {reason}"),
            );
        }
        GoalProgress::Completed => {
            return make_outcome(
                action,
                true,
                format!("goal '{goal_id}' is already completed"),
            );
        }
        _ => {}
    }

    // If a base-type session is available, use run_turn for real agent work.
    if let Some(ref mut session) = bridges.session {
        let result =
            super::goal_session::advance_goal_with_session(action, session.as_mut(), state, &goal);

        // For spawn_engineer the dispatcher must perform the actual fork
        // (it owns the state mutation needed to set goal.assigned_to).
        if let Some(GoalAction::SpawnEngineer {
            task,
            files: _,
            issue: _,
        }) = result.action
        {
            return dispatch_spawn_engineer(action, state, &goal_id, &task);
        }

        return result.outcome;
    }

    // No session = cannot advance. Fail visibly per PHILOSOPHY.md.
    make_outcome(
        action,
        false,
        format!(
            "goal '{goal_id}' cannot advance: no LLM session available. Check SIMARD_LLM_PROVIDER and auth config."
        ),
    )
}

/// Spawn a subordinate engineer for a goal that the LLM picked
/// `spawn_engineer` for, then mutate the active board to record the
/// assignment.
///
/// Honours `SIMARD_SUBORDINATE_DEPTH` vs. `SIMARD_MAX_SUBORDINATE_DEPTH`
/// so a recursing supervisor does not spawn forever.
fn dispatch_spawn_engineer(
    action: &PlannedAction,
    state: &mut OodaState,
    goal_id: &str,
    task: &str,
) -> ActionOutcome {
    // Re-check assignment under exclusive state borrow to prevent a
    // double-spawn race (two cycles parsing spawn_engineer back-to-back).
    if let Some(g) = state.active_goals.active.iter().find(|g| g.id == goal_id)
        && g.assigned_to.is_some()
    {
        return make_outcome(
            action,
            true,
            format!(
                "spawn_engineer skipped: goal '{goal_id}' already assigned to subordinate '{}'",
                g.assigned_to.as_deref().unwrap_or("?"),
            ),
        );
    }

    // Recursion guard. Default current depth = 0 (top-level supervisor).
    let current_depth: u32 = std::env::var("SIMARD_SUBORDINATE_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let depth_limit = max_subordinate_depth();
    if depth_limit < u32::MAX && current_depth >= depth_limit {
        eprintln!(
            "[simard] spawn_engineer DENIED for goal '{goal_id}': depth {current_depth} >= limit {depth_limit}"
        );
        return make_outcome(
            action,
            false,
            format!(
                "spawn_engineer denied for goal '{goal_id}': subordinate depth {current_depth} >= configured limit {depth_limit}"
            ),
        );
    }

    let agent_name = build_engineer_name(goal_id);
    let parent_repo = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            return make_outcome(
                action,
                false,
                format!(
                    "spawn_engineer failed for goal '{goal_id}': cannot resolve current_dir: {e}"
                ),
            );
        }
    };

    // Allocate a per-engineer git worktree (issue #1197) so concurrent
    // engineers never share the same checkout. The worktree lives under
    // `<state_root>/engineer-worktrees/` and is cleaned up when the
    // subordinate is reaped (or via Drop as a safety net).
    let state_root = engineer_worktree_state_root();
    let worktree = match crate::engineer_worktree::EngineerWorktree::allocate(
        &parent_repo,
        &state_root,
        goal_id,
    ) {
        Ok(w) => w,
        Err(e) => {
            eprintln!(
                "[simard] spawn_engineer FAILED for goal '{goal_id}': worktree allocation: {e}"
            );
            return make_outcome(
                action,
                false,
                format!("spawn_engineer failed for goal '{goal_id}': worktree allocation: {e}"),
            );
        }
    };
    let worktree_path = worktree.path().to_path_buf();

    let config = SubordinateConfig {
        agent_name: agent_name.clone(),
        goal: task.to_string(),
        role: AgentRole::Engineer,
        worktree_path,
        current_depth,
    };

    match spawn_subordinate(&config) {
        Ok(handle) => {
            // Record the assignment so subsequent cycles take the
            // heartbeat-checking path instead of re-spawning.
            if let Some(g) = state
                .active_goals
                .active
                .iter_mut()
                .find(|g| g.id == goal_id)
            {
                g.assigned_to = Some(agent_name.clone());
            }
            // Take ownership of the worktree on the OODA state so the reaper
            // path can clean it up after the subordinate exits. Drop is the
            // safety net if the entry leaves the map without explicit cleanup.
            state
                .engineer_worktrees
                .insert(goal_id.to_string(), worktree);

            // WS-2: persist the tmux session into the dashboard registry so
            // the Recent Actions feed can render Attach deep-links. Failures
            // are logged but never block subagent execution.
            if !handle.session_name.is_empty() {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let record = crate::subagent_sessions::SubagentSession {
                    agent_id: agent_name.clone(),
                    session_name: handle.session_name.clone(),
                    host: "local".to_string(),
                    pid: handle.pid,
                    created_at: now,
                    ended_at: None,
                    goal_id: goal_id.to_string(),
                };
                if let Err(e) = crate::subagent_sessions::record_spawn(record) {
                    tracing::warn!(
                        target: "simard::subagent_sessions",
                        agent = %agent_name,
                        session = %handle.session_name,
                        error = %e,
                        "failed to persist subagent session registry entry; spawn proceeds",
                    );
                }
            }

            eprintln!(
                "[simard] spawn_engineer dispatched: goal='{goal_id}', agent='{agent_name}', pid={}",
                handle.pid,
            );
            make_outcome(
                action,
                true,
                format!(
                    "spawn_engineer dispatched: agent='{agent_name}', task='{}' (goal '{goal_id}', pid={})",
                    truncate_for_log(task),
                    handle.pid,
                ),
            )
        }
        Err(e) => {
            // Explicitly cleanup the worktree we just allocated; Drop is the
            // safety net but explicit cleanup gives observable failure logs.
            if let Err(ce) = worktree.cleanup() {
                tracing::warn!(
                    target: "simard::engineer_worktree",
                    goal = %goal_id,
                    error = %ce,
                    "explicit worktree cleanup after spawn failure failed",
                );
            }
            eprintln!("[simard] spawn_engineer FAILED for goal '{goal_id}': {e}");
            make_outcome(
                action,
                false,
                format!("spawn_engineer failed for goal '{goal_id}': {e}"),
            )
        }
    }
}

/// Resolve the supervisor state root for engineer worktrees.
///
/// Honors `SIMARD_STATE_ROOT` then falls back to `$HOME/.simard`, matching
/// the supervisor's own resolution to keep all per-engineer state in a
/// single discoverable tree.
fn engineer_worktree_state_root() -> std::path::PathBuf {
    std::env::var("SIMARD_STATE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
            std::path::PathBuf::from(home).join(".simard")
        })
}

/// Build a unique subordinate agent name for a goal.
///
/// The epoch suffix prevents collisions when a goal's previous engineer
/// died and a fresh one needs to be spawned in the same process.
fn build_engineer_name(goal_id: &str) -> String {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("engineer-{goal_id}-{epoch}")
}

/// Truncate a user-derived string for safe inclusion in outcome detail / logs.
fn truncate_for_log(s: &str) -> String {
    const MAX: usize = 256;
    if s.len() <= MAX {
        s.to_string()
    } else {
        let mut end = MAX;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// Advance a goal that has a subordinate assigned by checking heartbeat
/// and validating output artifacts.
fn advance_goal_with_subordinate(
    action: &PlannedAction,
    bridges: &mut OodaBridges,
    state: &mut OodaState,
    goal_id: &str,
    sub_name: &str,
) -> ActionOutcome {
    // Build a minimal handle for heartbeat checking. The worktree path is
    // taken from the OODA-owned EngineerWorktree (issue #1197) when
    // available so artifact validation looks at the engineer's own scope,
    // not the parent checkout. Falls back to "." for legacy/manual paths
    // that pre-date worktree isolation.
    let worktree_path = state
        .engineer_worktrees
        .get(goal_id)
        .map(|w| w.path().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let handle = crate::agent_supervisor::SubordinateHandle {
        pid: 0,
        agent_name: sub_name.to_string(),
        goal: goal_id.to_string(),
        worktree_path,
        spawn_time: 0,
        retry_count: 0,
        killed: false,
        session_name: String::new(),
    };

    match check_heartbeat(&handle, &*bridges.memory) {
        Ok(HeartbeatStatus::Alive { phase, .. }) => {
            // Check if subordinate reported completion with an outcome.
            if let Ok(Some(progress)) =
                crate::agent_goal_assignment::poll_progress(sub_name, &*bridges.memory)
                && progress.outcome.is_some()
            {
                // Subordinate claims completion — validate artifacts.
                return validate_subordinate_completion(
                    action, state, goal_id, sub_name, &progress,
                );
            }

            // Subordinate is alive and still working.
            let new_progress = GoalProgress::InProgress { percent: 50 };
            let _ = update_goal_progress(&mut state.active_goals, goal_id, new_progress);
            make_outcome(
                action,
                true,
                format!(
                    "subordinate '{sub_name}' alive (phase={phase}), goal '{goal_id}' in-progress"
                ),
            )
        }
        Ok(HeartbeatStatus::Stale { seconds_since }) => {
            // Subordinate is stale — check if it left behind any artifacts
            // before marking as failed.
            if let Ok(Some(progress)) =
                crate::agent_goal_assignment::poll_progress(sub_name, &*bridges.memory)
                && progress.outcome.is_some()
            {
                return validate_subordinate_completion(
                    action, state, goal_id, sub_name, &progress,
                );
            }

            eprintln!(
                "[simard] WARNING: subordinate '{sub_name}' stale ({seconds_since}s) \
                 with no completion outcome — goal '{goal_id}' needs reassignment"
            );
            make_outcome(
                action,
                false,
                format!(
                    "subordinate '{sub_name}' stale ({seconds_since}s) with no artifacts, \
                     goal '{goal_id}' needs reassignment"
                ),
            )
        }
        Ok(HeartbeatStatus::Dead) => {
            // Subordinate is dead — check if it produced anything before dying.
            if let Ok(Some(progress)) =
                crate::agent_goal_assignment::poll_progress(sub_name, &*bridges.memory)
            {
                if progress.outcome.is_some() {
                    return validate_subordinate_completion(
                        action, state, goal_id, sub_name, &progress,
                    );
                }
                // Subordinate reported progress but no outcome — silent exit.
                eprintln!(
                    "[simard] WARNING: subordinate '{sub_name}' died without reporting \
                     an outcome — last phase='{}', last action='{}', \
                     exit_status={:?}, commits={}, prs={}",
                    progress.phase,
                    progress.last_action,
                    progress.exit_status,
                    progress.commits_produced,
                    progress.prs_produced,
                );
            } else {
                eprintln!(
                    "[simard] WARNING: subordinate '{sub_name}' is dead with no progress \
                     reports at all — it may have exited immediately without doing any work"
                );
            }

            let _ = update_goal_progress(
                &mut state.active_goals,
                goal_id,
                GoalProgress::Blocked(format!(
                    "subordinate '{sub_name}' exited without producing commits or PRs"
                )),
            );
            // Reap the per-engineer worktree (issue #1197).
            cleanup_engineer_worktree_for_goal(state, goal_id);
            make_outcome(
                action,
                false,
                format!(
                    "subordinate '{sub_name}' exited with no output artifacts, \
                     goal '{goal_id}' marked failed for retry"
                ),
            )
        }
        Err(e) => make_outcome(
            action,
            false,
            format!("heartbeat check failed for subordinate '{sub_name}': {e}"),
        ),
    }
}

/// Cleanup the per-goal engineer worktree owned by the OODA state.
///
/// Called from terminal paths (subordinate completed, dead, or stale-failed)
/// so the worktree dir + branch are reaped within one OODA cycle of the
/// engineer's exit. Idempotent — missing entries are silently a no-op.
fn cleanup_engineer_worktree_for_goal(state: &mut OodaState, goal_id: &str) {
    if let Some(worktree) = state.engineer_worktrees.remove(goal_id) {
        if let Err(e) = worktree.cleanup() {
            tracing::warn!(
                target: "simard::engineer_worktree",
                goal = %goal_id,
                error = %e,
                "engineer worktree cleanup failed; Drop will run as a safety net",
            );
        }
        // worktree drops here; if cleanup() already ran the swap guard
        // ensures Drop is a no-op.
    }
}

/// Validate that a subordinate's claimed completion produced real artifacts.
///
/// If the subordinate reports success but has zero commits and zero PRs,
/// the action is marked as failed so the OODA cycle can retry with a
/// different approach.
fn validate_subordinate_completion(
    action: &PlannedAction,
    state: &mut OodaState,
    goal_id: &str,
    sub_name: &str,
    progress: &crate::agent_goal_assignment::SubordinateProgress,
) -> ActionOutcome {
    let has_artifacts = progress.has_artifacts();
    let outcome_text = progress.outcome.as_deref().unwrap_or("unknown");

    if has_artifacts {
        let new_progress = GoalProgress::Completed;
        let _ = update_goal_progress(&mut state.active_goals, goal_id, new_progress);
        eprintln!(
            "[simard] subordinate '{sub_name}' completed goal '{goal_id}' — \
             {} commit(s), {} PR(s), outcome='{outcome_text}'",
            progress.commits_produced, progress.prs_produced,
        );
        // Reap the per-engineer worktree (issue #1197).
        cleanup_engineer_worktree_for_goal(state, goal_id);
        make_outcome(
            action,
            true,
            format!(
                "subordinate '{sub_name}' completed goal '{goal_id}' with \
                 {} commit(s) and {} PR(s)",
                progress.commits_produced, progress.prs_produced,
            ),
        )
    } else {
        // Subordinate claims success but produced nothing — this is the
        // silent exit bug (issue #905). Mark as failed for retry.
        eprintln!(
            "[simard] WARNING: subordinate '{sub_name}' reported outcome \
             '{outcome_text}' for goal '{goal_id}' but produced 0 commits \
             and 0 PRs — marking as failed for OODA retry"
        );
        let _ = update_goal_progress(
            &mut state.active_goals,
            goal_id,
            GoalProgress::Blocked(format!(
                "subordinate '{sub_name}' exited with outcome '{outcome_text}' \
                 but produced no commits or PRs"
            )),
        );
        // Reap the per-engineer worktree (issue #1197).
        cleanup_engineer_worktree_for_goal(state, goal_id);
        make_outcome(
            action,
            false,
            format!(
                "subordinate '{sub_name}' claimed '{outcome_text}' for goal '{goal_id}' \
                 but produced 0 commits and 0 PRs — action failed, eligible for retry"
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::goal_curation::GoalProgress;
    use crate::ooda_actions::dispatch_actions;
    use crate::ooda_actions::test_helpers::*;
    use crate::ooda_loop::{ActionKind, OodaState, PlannedAction};

    #[test]
    fn dispatch_advance_goal_without_session_fails() {
        let mut bridges = test_bridges(); // session: None
        let board = board_with_goal("g1", GoalProgress::NotStarted, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert!(
            !outcomes[0].success,
            "advance without LLM session must fail"
        );
        assert!(outcomes[0].detail.contains("no LLM session available"));
    }

    #[test]
    fn dispatch_advance_goal_blocked_fails() {
        let mut bridges = test_bridges();
        let board = board_with_goal("g1", GoalProgress::Blocked("waiting".into()), None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert!(!outcomes[0].success);
        assert!(outcomes[0].detail.contains("blocked"));
    }

    #[test]
    fn dispatch_advance_goal_missing_id_fails() {
        let mut bridges = test_bridges();
        let mut state = OodaState::new(crate::goal_curation::GoalBoard::new());
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: None,
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert!(!outcomes[0].success);
        assert!(outcomes[0].detail.contains("requires a goal_id"));
    }

    #[test]
    fn dispatch_advance_goal_with_dead_subordinate_blocks() {
        let mut bridges = test_bridges();
        let board = board_with_goal("g1", GoalProgress::NotStarted, Some("sub-1"));
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        // No progress facts in memory means Dead heartbeat — should report no artifacts.
        assert!(!outcomes[0].success);
        assert!(
            outcomes[0].detail.contains("no output artifacts"),
            "expected 'no output artifacts' in detail, got: {}",
            outcomes[0].detail
        );
    }

    #[test]
    fn validate_subordinate_completion_with_artifacts_succeeds() {
        let progress = crate::agent_goal_assignment::SubordinateProgress {
            sub_id: "sub-ok".to_string(),
            phase: "done".to_string(),
            steps_completed: 5,
            steps_total: 5,
            last_action: "pushed PR".to_string(),
            heartbeat_epoch: 1000,
            outcome: Some("success".to_string()),
            commits_produced: 3,
            prs_produced: 1,
            exit_status: Some(0),
        };
        let board = board_with_goal("g1", GoalProgress::InProgress { percent: 50 }, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcome =
            super::validate_subordinate_completion(&action, &mut state, "g1", "sub-ok", &progress);
        assert!(
            outcome.success,
            "should succeed with artifacts: {}",
            outcome.detail
        );
        assert!(outcome.detail.contains("3 commit(s)"));
        assert!(outcome.detail.contains("1 PR(s)"));
    }

    #[test]
    fn validate_subordinate_completion_without_artifacts_fails() {
        let progress = crate::agent_goal_assignment::SubordinateProgress {
            sub_id: "sub-empty".to_string(),
            phase: "done".to_string(),
            steps_completed: 5,
            steps_total: 5,
            last_action: "exited".to_string(),
            heartbeat_epoch: 1000,
            outcome: Some("success".to_string()),
            commits_produced: 0,
            prs_produced: 0,
            exit_status: Some(0),
        };
        let board = board_with_goal("g1", GoalProgress::InProgress { percent: 50 }, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcome = super::validate_subordinate_completion(
            &action,
            &mut state,
            "g1",
            "sub-empty",
            &progress,
        );
        assert!(
            !outcome.success,
            "should fail when no artifacts: {}",
            outcome.detail
        );
        assert!(outcome.detail.contains("0 commits"));
        assert!(outcome.detail.contains("0 PRs"));
    }
}
