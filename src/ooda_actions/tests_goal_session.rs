//! Integration tests for `advance_goal_with_session` covering the strict
//! goal-session response contract: explicit spawn markers, NO ACTION with
//! REASON, bounded PROGRESS markers, and loud invalid-response failures.

use crate::goal_curation::progress_evidence::{EvidenceDecision, ProgressEvidenceChecker};
use crate::goal_curation::{GoalBoard, GoalProgress};
use crate::ooda_actions::goal_session::{GoalAction, GoalSessionResult, advance_goal_with_session};
use crate::ooda_actions::test_helpers::*;
use crate::ooda_loop::{ActionKind, OodaState, PlannedAction};
use chrono::{DateTime, Utc};
use std::sync::{Mutex, MutexGuard, OnceLock};

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

fn lock_env_for_test() -> MutexGuard<'static, ()> {
    env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct ObserveOnlyEnvGuard {
    previous: Option<std::ffi::OsString>,
}

impl Drop for ObserveOnlyEnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var("SIMARD_OBSERVE_ONLY", value),
                None => std::env::remove_var("SIMARD_OBSERVE_ONLY"),
            }
        }
    }
}

fn set_observe_only_for_test(value: Option<&str>) -> ObserveOnlyEnvGuard {
    let previous = std::env::var_os("SIMARD_OBSERVE_ONLY");
    unsafe {
        match value {
            Some(value) => std::env::set_var("SIMARD_OBSERVE_ONLY", value),
            None => std::env::remove_var("SIMARD_OBSERVE_ONLY"),
        }
    }
    ObserveOnlyEnvGuard { previous }
}

fn run_goal_session_response(response: &str) -> (GoalSessionResult, OodaState) {
    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);
    let (mut session, _captured) = MockSession::new_ok(response, vec![]);

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

    (result, state)
}

#[test]
fn no_action_response_records_no_action_outcome_without_spawning() {
    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let (mut session, _captured) = MockSession::new_ok(
        "NO ACTION\nREASON: another subordinate (engineer-foo-1234) is already in flight.",
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
#[serial_test::serial(cognitive_memory)]
fn explicit_spawn_response_routes_to_spawn_engineer_and_extracts_task_body() {
    let _guard = lock_env_for_test();
    let _env = set_observe_only_for_test(None);

    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let task_text = "Run cargo test --lib goal_session and report failing tests.";
    let response = format!("ACTION: SPAWN_ENGINEER\nTASK:\n{task_text}\nPROGRESS: 20");
    let (mut session, _captured) = MockSession::new_ok(&response, vec![]);

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
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn observe_only_explicit_spawn_response_records_no_action_without_spawn() {
    let _guard = lock_env_for_test();
    let _env = set_observe_only_for_test(Some("1"));

    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let task_text = "Inspect the repository read-only and report concrete evidence.";
    let response = format!("ACTION: SPAWN_ENGINEER\nTASK:\n{task_text}\nPROGRESS: 10");
    let (mut session, _captured) = MockSession::new_ok(&response, vec![]);

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
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn read_only_identity_explicit_spawn_response_records_no_action_without_env_floor() {
    let _guard = lock_env_for_test();
    let _env = set_observe_only_for_test(None);

    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    state.identity_cognition.authority = Some(crate::identity::IdentityAuthority::read_only());
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let task_text = "Inspect the repository read-only and report concrete evidence.";
    let response = format!("ACTION: SPAWN_ENGINEER\nTASK:\n{task_text}\nPROGRESS: 10");
    let (mut session, captured) = MockSession::new_ok(&response, vec![]);

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
    match result.action {
        Some(GoalAction::NoAction { reason }) => {
            assert!(reason.contains("converted spawn request"));
            assert!(reason.contains(task_text));
        }
        other => panic!("expected NoAction, got {other:?}"),
    }
    let captured = captured.borrow();
    let input = captured.as_ref().expect("session must be invoked once");
    assert!(input.objective.contains("Read-only observer contract"));
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn progress_marker_in_explicit_spawn_updates_goal_progress_before_spawn() {
    let _guard = lock_env_for_test();
    let _env = set_observe_only_for_test(None);

    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let (mut session, _captured) = MockSession::new_ok(
        "ACTION: SPAWN_ENGINEER\nTASK:\nFinish the dashboard.\nPROGRESS: 70",
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
}

#[test]
fn progress_marker_in_no_action_updates_goal_progress() {
    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let (mut session, _captured) = MockSession::new_ok(
        "NO ACTION\nREASON: waiting on PR review.\nPROGRESS: 95",
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
        GoalProgress::InProgress { percent } => assert_eq!(percent, 95),
        other => panic!("expected InProgress(95), got {other:?}"),
    }
}

struct RequiresCurrentEvidence;

impl ProgressEvidenceChecker for RequiresCurrentEvidence {
    fn check(
        &self,
        goal: &crate::goal_curation::ActiveGoal,
        _old_percent: u32,
        _new_percent: u32,
        _since: DateTime<Utc>,
    ) -> EvidenceDecision {
        let activity = goal.current_activity.as_deref().unwrap_or("");
        if activity.contains("EVIDENCE:") {
            EvidenceDecision::Accept {
                reason: "current no-action evidence was visible".to_string(),
            }
        } else {
            EvidenceDecision::Reject {
                reason: format!("missing current no-action evidence; activity={activity:?}"),
            }
        }
    }
}

#[test]
fn no_action_progress_review_sees_current_cycle_evidence() {
    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let (mut session, _captured) = MockSession::new_ok(
        "NO ACTION\nREASON: observed CODEOWNERS is missing.\nPROGRESS: 5\nEVIDENCE:\n- observed CODEOWNERS is missing",
        vec![],
    );

    let mem_box = mock_memory();
    let result = advance_goal_with_session(
        &action,
        &*mem_box,
        &RequiresCurrentEvidence,
        &mut session,
        &mut state,
        &goal,
    );

    assert!(result.outcome.success);
    assert!(
        result.outcome.detail.contains("progress=5%"),
        "expected accepted progress detail, got: {}",
        result.outcome.detail
    );
    let updated = live_goal(&state, goal_id);
    match updated.status {
        GoalProgress::InProgress { percent } => assert_eq!(percent, 5),
        other => panic!("expected InProgress(5), got {other:?}"),
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
fn free_form_prose_response_is_a_visible_invalid_contract_failure() {
    let (result, _state) =
        run_goal_session_response("Run cargo test --lib goal_session and fix any failures.");

    assert!(!result.outcome.success);
    assert!(
        result
            .outcome
            .detail
            .contains("invalid goal-session response")
            || result.outcome.detail.contains("invalid response contract"),
        "expected visible invalid-contract failure, got: {}",
        result.outcome.detail
    );
    assert!(result.action.is_none());
}

#[test]
fn no_action_without_reason_is_a_visible_invalid_contract_failure() {
    let (result, _state) = run_goal_session_response("NO ACTION\nPROGRESS: 0");

    assert!(!result.outcome.success);
    assert!(
        result.outcome.detail.contains("REASON"),
        "expected missing-REASON failure, got: {}",
        result.outcome.detail
    );
    assert!(result.action.is_none());
}

#[test]
fn conflicting_spawn_and_no_action_markers_are_rejected() {
    let (result, _state) = run_goal_session_response(
        "NO ACTION\nREASON: already in flight.\nACTION: SPAWN_ENGINEER\nTASK:\nStart a second engineer.",
    );

    assert!(!result.outcome.success);
    assert!(
        result.outcome.detail.contains("conflicting")
            || result.outcome.detail.contains("multiple action"),
        "expected conflicting-marker failure, got: {}",
        result.outcome.detail
    );
    assert!(result.action.is_none());
}

#[test]
fn unknown_action_marker_is_rejected() {
    let (result, _state) = run_goal_session_response(
        "ACTION: MERGE_PR\nTASK:\nMerge PR #4042 directly.\nPROGRESS: 80",
    );

    assert!(!result.outcome.success);
    assert!(
        result.outcome.detail.contains("unknown action")
            || result.outcome.detail.contains("ACTION"),
        "expected unknown-action failure, got: {}",
        result.outcome.detail
    );
    assert!(result.action.is_none());
}

#[test]
fn progress_above_100_is_rejected_without_mutating_goal() {
    let (result, state) =
        run_goal_session_response("NO ACTION\nREASON: PR is almost landed.\nPROGRESS: 125");

    assert!(!result.outcome.success);
    assert!(
        result.outcome.detail.contains("PROGRESS")
            && (result.outcome.detail.contains("0..=100")
                || result.outcome.detail.contains("out of range")),
        "expected out-of-range PROGRESS failure, got: {}",
        result.outcome.detail
    );
    assert!(result.action.is_none());

    let updated = live_goal(&state, "test-goal");
    assert_eq!(
        updated.status,
        GoalProgress::NotStarted,
        "invalid progress must not be clamped or applied"
    );
}

#[test]
fn duplicate_progress_markers_are_rejected_without_guessing() {
    let (result, _state) = run_goal_session_response(
        "NO ACTION\nREASON: evidence is mixed.\nPROGRESS: 40\nPROGRESS: 80",
    );

    assert!(!result.outcome.success);
    assert!(
        result.outcome.detail.contains("duplicate")
            || result.outcome.detail.contains("multiple PROGRESS"),
        "expected duplicate-progress failure, got: {}",
        result.outcome.detail
    );
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

    let (mut session, captured) = MockSession::new_ok("NO ACTION\nREASON: smoke test.\n", vec![]);

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
#[serial_test::serial(cognitive_memory)]
fn observe_only_objective_forbids_engineer_dispatch_and_requires_evidence_protocol() {
    let _guard = lock_env_for_test();
    let _env = set_observe_only_for_test(Some("1"));

    let goal_id = "test-goal";
    let mut state = state_with_goal(goal_id);
    let goal = live_goal(&state, goal_id);
    let action = planned_action(goal_id);

    let (mut session, captured) = MockSession::new_ok(
        "NO ACTION\nREASON: read-only smoke test.\nPROGRESS: 0",
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

    let captured = captured.borrow();
    let input = captured.as_ref().expect("session must be invoked once");
    assert!(input.objective.contains("Read-only observer contract"));
    assert!(input.objective.contains("Do not ask for"));
    assert!(input.objective.contains("dispatch an engineer"));
    assert!(input.objective.contains("NO ACTION"));
    assert!(input.objective.contains("EVIDENCE/PROPOSALS"));
    assert!(
        input
            .objective
            .contains("Read-only means no writes, not no progress")
    );
    assert!(input.objective.contains("modest positive progress"));
}
