//! Integration tests for `advance_goal_with_session` covering the prose
//! dispatch contract: NO ACTION marker, SpawnEngineer prose, PROGRESS
//! marker, and empty-response failure.

use crate::goal_curation::{GoalBoard, GoalProgress};
use crate::ooda_actions::goal_session::{GoalAction, advance_goal_with_session};
use crate::ooda_actions::test_helpers::*;
use crate::ooda_loop::{ActionKind, OodaState, PlannedAction};

fn planned_action(goal_id: &str) -> PlannedAction {
    PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some(goal_id.to_string()),
        description: format!("advance goal {goal_id}"),
    }
}

fn state_with_goal(goal_id: &str) -> OodaState {
    let board: GoalBoard = board_with_goal(goal_id, GoalProgress::NotStarted, None);
    OodaState::new(board)
}

fn live_goal(state: &OodaState, goal_id: &str) -> crate::goal_curation::ActiveGoal {
    state
        .active_goals
        .active
        .iter()
        .find(|g| g.id == goal_id)
        .cloned()
        .expect("seeded goal must exist")
}

#[test]
fn no_action_response_records_no_action_outcome_without_spawning() {
    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let (mut session, _captured) = MockSession::new_ok(
        "NO ACTION\nAnother subordinate (engineer-foo-1234) is already in flight.",
        vec![],
    );

    let result = advance_goal_with_session(&action, &mut session, &mut state, &goal);

    assert!(
        result.outcome.success,
        "no-action should be a success outcome"
    );
    assert!(result.outcome.detail.contains("no-action"));
    match result.action {
        Some(GoalAction::NoAction { reason }) => {
            assert!(reason.contains("subordinate"));
        }
        other => panic!("expected NoAction, got {other:?}"),
    }
}

#[test]
fn prose_response_routes_to_spawn_engineer() {
    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let task_text = "Run cargo test --lib goal_session and report failing tests.";
    let (mut session, _captured) = MockSession::new_ok(task_text, vec![]);

    let result = advance_goal_with_session(&action, &mut session, &mut state, &goal);

    assert!(result.outcome.success);
    assert!(result.outcome.detail.contains("spawn_engineer"));
    match result.action {
        Some(GoalAction::SpawnEngineer { task, .. }) => {
            assert_eq!(task, task_text);
        }
        other => panic!("expected SpawnEngineer, got {other:?}"),
    }
}

#[test]
fn progress_marker_in_prose_updates_goal_progress_before_spawn() {
    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let (mut session, _captured) = MockSession::new_ok(
        "Spawn engineer to finish the dashboard. PROGRESS: 70",
        vec![],
    );

    let _ = advance_goal_with_session(&action, &mut session, &mut state, &goal);

    let updated = live_goal(&state, goal_id);
    match updated.status {
        GoalProgress::InProgress { percent } => assert_eq!(percent, 70),
        other => panic!("expected InProgress(70), got {other:?}"),
    }
}

#[test]
fn progress_marker_in_no_action_updates_goal_progress() {
    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let (mut session, _captured) =
        MockSession::new_ok("NO ACTION\nWaiting on PR review. PROGRESS: 95", vec![]);

    let _ = advance_goal_with_session(&action, &mut session, &mut state, &goal);

    let updated = live_goal(&state, goal_id);
    match updated.status {
        GoalProgress::InProgress { percent } => assert_eq!(percent, 95),
        other => panic!("expected InProgress(95), got {other:?}"),
    }
}

#[test]
fn empty_response_is_a_visible_failure() {
    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let (mut session, _captured) = MockSession::new_ok("   \n\t  ", vec![]);

    let result = advance_goal_with_session(&action, &mut session, &mut state, &goal);

    assert!(!result.outcome.success);
    assert!(result.outcome.detail.contains("empty response"));
    assert!(result.action.is_none());
}

#[test]
fn session_run_turn_error_is_a_visible_failure() {
    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let mut session = MockSession::new_err("LLM provider unavailable");

    let result = advance_goal_with_session(&action, &mut session, &mut state, &goal);

    assert!(!result.outcome.success);
    assert!(result.outcome.detail.contains("session run_turn failed"));
    assert!(result.action.is_none());
}

#[test]
fn objective_includes_goal_metadata_and_environment() {
    // Sanity: the captured BaseTypeTurnInput should contain the goal id,
    // the percent, the description, and the environment context section.
    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let (mut session, captured) = MockSession::new_ok("NO ACTION\n", vec![]);

    let _ = advance_goal_with_session(&action, &mut session, &mut state, &goal);

    let captured = captured.borrow();
    let input = captured.as_ref().expect("session must be invoked once");
    assert!(input.objective.contains(goal_id));
    assert!(input.objective.contains(&format!("Goal {goal_id}")));
    assert!(input.objective.contains("Environment context"));
}
