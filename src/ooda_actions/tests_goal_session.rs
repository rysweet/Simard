use crate::goal_curation::GoalProgress;
use crate::ooda_actions::dispatch_actions;
use crate::ooda_actions::test_helpers::*;
use crate::ooda_loop::{ActionKind, OodaState, PlannedAction};

#[test]
fn session_identity_describes_pm_architect_not_coder() {
    let (session, captured) = MockSession::new_ok("PROGRESS: 25", vec![]);
    let mut bridges = bridges_with_session(session);
    let board = board_with_goal("g1", GoalProgress::InProgress { percent: 20 }, None);
    let mut state = OodaState::new(board);
    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g1".into()),
        description: "advance".into(),
    };
    dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

    let input = captured.borrow();
    let input = input.as_ref().expect("session should have received input");
    let id = &input.identity_context;

    // Must describe PM architect role, not a coder.
    assert!(
        id.contains("PM architect"),
        "identity should mention PM architect, got: {id}"
    );
    assert!(
        id.contains("amplihack") || id.contains("coding sessions"),
        "identity should mention managing coding sessions, got: {id}"
    );
    assert!(
        !id.to_lowercase().contains("you write code")
            && !id.to_lowercase().contains("you are a coder"),
        "identity must NOT describe Simard as a coder, got: {id}"
    );
}

#[test]
fn session_objective_includes_assessment_steps() {
    let (session, captured) = MockSession::new_ok(
        r#"{"action":"assess_only","assessment":"checked","progress_pct":30}"#,
        vec![],
    );
    let mut bridges = bridges_with_session(session);
    let board = board_with_goal("g1", GoalProgress::InProgress { percent: 10 }, None);
    let mut state = OodaState::new(board);
    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g1".into()),
        description: "advance".into(),
    };
    dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

    let input = captured.borrow();
    let input = input.as_ref().expect("session should have received input");
    let obj = &input.objective;

    // Objective must include the goal ID.
    assert!(obj.contains("g1"), "objective should contain goal ID");

    // Must teach the structured-action contract.
    assert!(
        obj.contains("spawn_engineer") && obj.contains("gh_issue_create"),
        "objective should teach the JSON action schemas, got: {obj}"
    );

    // Must mention issue-first orchestration.
    assert!(
        obj.to_lowercase().contains("issue"),
        "objective should mention GitHub issues, got: {obj}"
    );

    // Must reject the legacy PROGRESS-line scraping contract.
    assert!(
        !obj.contains("PROGRESS:"),
        "objective MUST NOT request a PROGRESS line (the legacy fallback was removed), got: {obj}"
    );
}

#[test]
fn session_progress_comes_from_agent_response_not_auto_bump() {
    // Agent returns the new structured action with progress=55 — goal becomes 55%.
    let (session, _captured) = MockSession::new_ok(
        r#"{"action":"assess_only","assessment":"made progress","progress_pct":55}"#,
        vec![],
    );
    let mut bridges = bridges_with_session(session);
    let board = board_with_goal("g1", GoalProgress::InProgress { percent: 20 }, None);
    let mut state = OodaState::new(board);
    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g1".into()),
        description: "advance".into(),
    };
    let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

    assert!(outcomes[0].success);
    assert_eq!(
        state.active_goals.active[0].status,
        GoalProgress::InProgress { percent: 55 },
        "progress should come from assess_only.progress_pct, not auto-bump"
    );
}

#[test]
fn session_no_progress_marker_preserves_current() {
    // Agent returns prose instead of valid JSON — action MUST fail loudly,
    // progress MUST be preserved, no silent fallback.
    let (session, _captured) = MockSession::new_ok(
        "Checked the repo. Everything looks fine.",
        vec!["no markers here".to_string()],
    );
    let mut bridges = bridges_with_session(session);
    let board = board_with_goal("g1", GoalProgress::InProgress { percent: 40 }, None);
    let mut state = OodaState::new(board);
    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g1".into()),
        description: "advance".into(),
    };
    let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

    // Non-JSON response = parse failure = failed outcome.
    assert!(
        !outcomes[0].success,
        "non-JSON LLM response must produce a failed outcome (no silent fallback)"
    );
    assert!(
        outcomes[0].detail.contains("parse failed"),
        "outcome detail should explain the parse failure, got: {}",
        outcomes[0].detail,
    );
    // Progress MUST stay at 40% (no silent mutation).
    assert_eq!(
        state.active_goals.active[0].status,
        GoalProgress::InProgress { percent: 40 },
        "progress must be preserved when LLM emits invalid action"
    );
}

#[test]
fn session_progress_100_completes_goal() {
    let (session, _captured) = MockSession::new_ok(
        r#"{"action":"assess_only","assessment":"done","progress_pct":100}"#,
        vec![],
    );
    let mut bridges = bridges_with_session(session);
    let board = board_with_goal("g1", GoalProgress::InProgress { percent: 80 }, None);
    let mut state = OodaState::new(board);
    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g1".into()),
        description: "advance".into(),
    };
    let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

    assert!(outcomes[0].success);
    assert_eq!(state.active_goals.active[0].status, GoalProgress::Completed);
}

#[test]
fn session_run_turn_failure_returns_error_outcome() {
    let session = MockSession::new_err("connection lost");
    let mut bridges = bridges_with_session(session);
    let board = board_with_goal("g1", GoalProgress::InProgress { percent: 10 }, None);
    let mut state = OodaState::new(board);
    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g1".into()),
        description: "advance".into(),
    };
    let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

    assert!(!outcomes[0].success);
    assert!(outcomes[0].detail.contains("session run_turn failed"));
    // Progress must NOT change on error.
    assert_eq!(
        state.active_goals.active[0].status,
        GoalProgress::InProgress { percent: 10 },
    );
}

#[test]
fn session_objective_includes_environment_context() {
    let (session, captured) = MockSession::new_ok("PROGRESS: 20", vec![]);
    let mut bridges = bridges_with_session(session);
    let board = board_with_goal("g1", GoalProgress::NotStarted, None);
    let mut state = OodaState::new(board);
    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g1".into()),
        description: "advance".into(),
    };
    dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

    let input = captured.borrow();
    let input = input.as_ref().expect("session should have received input");
    let obj = &input.objective;

    // Objective should include environment context (git status, issues, commits).
    assert!(
        obj.contains("Git status") || obj.contains("git status"),
        "objective should include environment context"
    );
}

#[test]
fn session_not_started_goal_reports_0_percent_in_objective() {
    let (session, captured) = MockSession::new_ok("PROGRESS: 5", vec![]);
    let mut bridges = bridges_with_session(session);
    let board = board_with_goal("g1", GoalProgress::NotStarted, None);
    let mut state = OodaState::new(board);
    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g1".into()),
        description: "advance".into(),
    };
    dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

    let input = captured.borrow();
    let input = input.as_ref().unwrap();
    // NotStarted should show 0% in the objective.
    assert!(
        input.objective.contains("0% complete"),
        "NotStarted goal should report 0% in objective"
    );
}

#[test]
fn session_outcome_describes_action_taken() {
    // Replaces the old `verified=` assertion: outcome detail must describe
    // which structured action was executed, not legacy verification counts.
    let (session, _) = MockSession::new_ok(
        r#"{"action":"assess_only","assessment":"made some progress","progress_pct":20}"#,
        vec![],
    );
    let mut bridges = bridges_with_session(session);
    let board = board_with_goal("g1", GoalProgress::NotStarted, None);
    let mut state = OodaState::new(board);
    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g1".into()),
        description: "advance".into(),
    };
    let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
    assert!(
        outcomes[0].detail.contains("assess_only"),
        "outcome should name the action taken, got: {}",
        outcomes[0].detail,
    );
    assert!(
        outcomes[0].detail.contains("progress=20%"),
        "outcome should include the new progress, got: {}",
        outcomes[0].detail,
    );
}

#[test]
fn objective_includes_concrete_commands() {
    let (session, captured) =
        MockSession::new_ok(r#"{"action":"noop","reason":"nothing to do"}"#, vec![]);
    let mut bridges = bridges_with_session(session);
    let board = board_with_goal("g1", GoalProgress::NotStarted, None);
    let mut state = OodaState::new(board);
    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g1".into()),
        description: "advance".into(),
    };
    dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

    let input = captured.borrow();
    let input = input.as_ref().unwrap();
    // New prompt teaches the JSON action surface, not raw shell commands.
    assert!(
        input.objective.contains("gh_issue_create"),
        "objective should teach the gh_issue_create action"
    );
    assert!(
        input.objective.contains("spawn_engineer"),
        "objective should teach the spawn_engineer action"
    );
    assert!(
        input.objective.contains("gh_issue_close"),
        "objective should teach the gh_issue_close action"
    );
}

// ===== Issue #929: dispatch outcome wiring tests =====
//
// These tests verify that dispatch_advance_goal records descriptive outcome
// detail strings for each LLM-action branch — no longer leaving outcomes
// empty. They MUST fail until advance_goal_with_session is refactored to
// parse JSON actions and emit branch-specific outcome strings.
