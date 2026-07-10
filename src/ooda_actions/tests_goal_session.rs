//! Integration tests for `advance_goal_with_session` covering the prose
//! dispatch contract: NO ACTION marker, SpawnEngineer prose, PROGRESS
//! marker, and empty-response failure.

use crate::goal_curation::{GoalBoard, GoalProgress};
use crate::ooda_actions::goal_session::{GoalAction, advance_goal_with_session};
use crate::ooda_actions::test_helpers::*;
use crate::ooda_loop::{ActionKind, OodaState, PlannedAction};
use std::sync::{Mutex, OnceLock};

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

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
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

    let mem_box = mock_memory();
    let checker = crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker;
    let result = advance_goal_with_session(
        &action,
        &*mem_box,
        &checker,
        &mut session,
        &mut state,
        &goal,
    );

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
    let _guard = env_lock().lock().unwrap();
    let old_observe = std::env::var_os("SIMARD_OBSERVE_ONLY");
    unsafe {
        std::env::remove_var("SIMARD_OBSERVE_ONLY");
    }

    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let task_text = "Run cargo test --lib goal_session and report failing tests.";
    let (mut session, _captured) = MockSession::new_ok(task_text, vec![]);

    let mem_box = mock_memory();
    let checker = crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker;
    let result = advance_goal_with_session(
        &action,
        &*mem_box,
        &checker,
        &mut session,
        &mut state,
        &goal,
    );

    assert!(result.outcome.success);
    assert!(result.outcome.detail.contains("spawn_engineer"));
    match result.action {
        Some(GoalAction::SpawnEngineer { task, .. }) => {
            assert_eq!(task, task_text);
        }
        other => panic!("expected SpawnEngineer, got {other:?}"),
    }

    unsafe {
        match old_observe {
            Some(value) => std::env::set_var("SIMARD_OBSERVE_ONLY", value),
            None => std::env::remove_var("SIMARD_OBSERVE_ONLY"),
        }
    }
}

#[test]
fn observe_only_prose_response_records_no_action_without_spawn() {
    let _guard = env_lock().lock().unwrap();
    let old_observe = std::env::var_os("SIMARD_OBSERVE_ONLY");
    unsafe {
        std::env::set_var("SIMARD_OBSERVE_ONLY", "1");
    }

    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let task_text = "Spawn engineer to inspect the repository read-only.";
    let (mut session, _captured) = MockSession::new_ok(task_text, vec![]);

    let mem_box = mock_memory();
    let checker = crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker;
    let result = advance_goal_with_session(
        &action,
        &*mem_box,
        &checker,
        &mut session,
        &mut state,
        &goal,
    );

    assert!(result.outcome.success);
    assert!(result.outcome.detail.contains("no-action"));
    assert!(
        !result.outcome.detail.contains("spawn_engineer"),
        "observe-only prose must not surface as a spawn outcome"
    );
    match result.action {
        Some(GoalAction::NoAction { reason }) => {
            assert!(reason.contains("converted spawn request"));
            assert!(reason.contains(task_text));
        }
        other => panic!("expected NoAction, got {other:?}"),
    }

    unsafe {
        match old_observe {
            Some(value) => std::env::set_var("SIMARD_OBSERVE_ONLY", value),
            None => std::env::remove_var("SIMARD_OBSERVE_ONLY"),
        }
    }
}

#[test]
fn progress_marker_in_prose_updates_goal_progress_before_spawn() {
    let _guard = env_lock().lock().unwrap();
    let old_observe = std::env::var_os("SIMARD_OBSERVE_ONLY");
    unsafe {
        std::env::remove_var("SIMARD_OBSERVE_ONLY");
    }

    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let (mut session, _captured) = MockSession::new_ok(
        "Spawn engineer to finish the dashboard. PROGRESS: 70",
        vec![],
    );

    let mem_box = mock_memory();
    let checker = crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker;
    let _ = advance_goal_with_session(
        &action,
        &*mem_box,
        &checker,
        &mut session,
        &mut state,
        &goal,
    );

    let updated = live_goal(&state, goal_id);
    match updated.status {
        GoalProgress::InProgress { percent } => assert_eq!(percent, 70),
        other => panic!("expected InProgress(70), got {other:?}"),
    }

    unsafe {
        match old_observe {
            Some(value) => std::env::set_var("SIMARD_OBSERVE_ONLY", value),
            None => std::env::remove_var("SIMARD_OBSERVE_ONLY"),
        }
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

    let mem_box = mock_memory();
    let checker = crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker;
    let _ = advance_goal_with_session(
        &action,
        &*mem_box,
        &checker,
        &mut session,
        &mut state,
        &goal,
    );

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

    let mem_box = mock_memory();
    let checker = crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker;
    let result = advance_goal_with_session(
        &action,
        &*mem_box,
        &checker,
        &mut session,
        &mut state,
        &goal,
    );

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

    let mem_box = mock_memory();
    let checker = crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker;
    let result = advance_goal_with_session(
        &action,
        &*mem_box,
        &checker,
        &mut session,
        &mut state,
        &goal,
    );

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

    let mem_box = mock_memory();
    let checker = crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker;
    let _ = advance_goal_with_session(
        &action,
        &*mem_box,
        &checker,
        &mut session,
        &mut state,
        &goal,
    );

    let captured = captured.borrow();
    let input = captured.as_ref().expect("session must be invoked once");
    assert!(input.objective.contains(goal_id));
    assert!(input.objective.contains(&format!("Goal {goal_id}")));
    assert!(input.objective.contains("Environment context"));
}

#[test]
fn observe_only_objective_forbids_engineer_dispatch_and_requires_evidence_protocol() {
    let _guard = env_lock().lock().unwrap();
    let old_observe = std::env::var_os("SIMARD_OBSERVE_ONLY");
    unsafe {
        std::env::set_var("SIMARD_OBSERVE_ONLY", "1");
    }

    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let (mut session, captured) = MockSession::new_ok("NO ACTION\nPROGRESS: 0", vec![]);

    let mem_box = mock_memory();
    let checker = crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker;
    let _ = advance_goal_with_session(
        &action,
        &*mem_box,
        &checker,
        &mut session,
        &mut state,
        &goal,
    );

    let captured = captured.borrow();
    let input = captured.as_ref().expect("session must be invoked once");
    assert!(input.objective.contains("Read-only observer contract"));
    assert!(input.objective.contains("Do not ask for"));
    assert!(input.objective.contains("dispatch an engineer"));
    assert!(input.objective.contains("NO ACTION"));
    assert!(input.objective.contains("EVIDENCE/PROPOSALS"));

    unsafe {
        match old_observe {
            Some(value) => std::env::set_var("SIMARD_OBSERVE_ONLY", value),
            None => std::env::remove_var("SIMARD_OBSERVE_ONLY"),
        }
    }
}
