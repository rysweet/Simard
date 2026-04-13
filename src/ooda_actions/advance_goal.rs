//! AdvanceGoal dispatch — routing, subordinate heartbeat, and session-based advancement.

use crate::agent_supervisor::{HeartbeatStatus, check_heartbeat};
use crate::goal_curation::{GoalProgress, update_goal_progress};
use crate::ooda_loop::{ActionOutcome, OodaBridges, OodaState, PlannedAction};

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
        return super::goal_session::advance_goal_with_session(
            action,
            session.as_mut(),
            state,
            &goal,
        );
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

/// Advance a goal that has a subordinate assigned by checking heartbeat.
fn advance_goal_with_subordinate(
    action: &PlannedAction,
    bridges: &mut OodaBridges,
    state: &mut OodaState,
    goal_id: &str,
    sub_name: &str,
) -> ActionOutcome {
    // Build a minimal handle for heartbeat checking.
    let handle = crate::agent_supervisor::SubordinateHandle {
        pid: 0,
        agent_name: sub_name.to_string(),
        goal: goal_id.to_string(),
        worktree_path: std::path::PathBuf::from("."),
        spawn_time: 0,
        retry_count: 0,
        killed: false,
    };

    match check_heartbeat(&handle, &bridges.memory) {
        Ok(HeartbeatStatus::Alive { phase, .. }) => {
            // Subordinate is alive; update goal to in-progress if not already.
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
        Ok(HeartbeatStatus::Stale { seconds_since }) => make_outcome(
            action,
            false,
            format!(
                "subordinate '{sub_name}' stale ({seconds_since}s), goal '{goal_id}' may need reassignment"
            ),
        ),
        Ok(HeartbeatStatus::Dead) => {
            let _ = update_goal_progress(
                &mut state.active_goals,
                goal_id,
                GoalProgress::Blocked(format!("subordinate '{sub_name}' is dead")),
            );
            make_outcome(
                action,
                false,
                format!("subordinate '{sub_name}' is dead, goal '{goal_id}' blocked"),
            )
        }
        Err(e) => make_outcome(
            action,
            false,
            format!("heartbeat check failed for subordinate '{sub_name}': {e}"),
        ),
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
        // No progress facts in memory means Dead heartbeat.
        assert!(!outcomes[0].success);
        assert!(outcomes[0].detail.contains("dead"));
    }
}
