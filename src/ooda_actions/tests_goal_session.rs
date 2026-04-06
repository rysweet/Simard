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
    let (session, captured) = MockSession::new_ok("PROGRESS: 30", vec![]);
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

    // Objective must include the goal ID and description.
    assert!(obj.contains("g1"), "objective should contain goal ID");

    // Must instruct assessment of goal status.
    assert!(
        obj.to_lowercase().contains("assess") || obj.to_lowercase().contains("check"),
        "objective should instruct assessment, got: {obj}"
    );

    // Must mention creating GitHub issues for work.
    assert!(
        obj.to_lowercase().contains("github issue") || obj.to_lowercase().contains("issue"),
        "objective should mention creating issues, got: {obj}"
    );

    // Must mention launching amplihack sessions.
    assert!(
        obj.contains("simard engineer") || obj.contains("amplihack copilot"),
        "objective should mention delegation commands, got: {obj}"
    );

    // Must request a PROGRESS line in the response.
    assert!(
        obj.contains("PROGRESS"),
        "objective should request PROGRESS assessment, got: {obj}"
    );
}

#[test]
fn session_progress_comes_from_agent_response_not_auto_bump() {
    // Agent reports PROGRESS: 55 — goal should become 55%, not current+10.
    let (session, _captured) = MockSession::new_ok(
        "Assessed the goal. Created issue #42.\nPROGRESS: 55",
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
    // Progress must be 55 (from agent response), NOT 30 (20+10 auto-bump).
    assert_eq!(
        state.active_goals.active[0].status,
        GoalProgress::InProgress { percent: 55 },
        "progress should come from agent's PROGRESS line, not auto-bump"
    );
}

#[test]
fn session_no_progress_marker_preserves_current() {
    // Agent does NOT include a PROGRESS line — current progress must be preserved.
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

    assert!(outcomes[0].success);
    // Must stay at 40%, NOT bumped to 50%.
    assert_eq!(
        state.active_goals.active[0].status,
        GoalProgress::InProgress { percent: 40 },
        "without PROGRESS marker, progress must be preserved (not auto-bumped)"
    );
}

#[test]
fn session_progress_100_completes_goal() {
    let (session, _captured) = MockSession::new_ok("PROGRESS: 100", vec![]);
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
    assert_eq!(state.active_goals.active[0].status, GoalProgress::Completed,);
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
fn session_outcome_includes_verification_counts() {
    let (session, _) = MockSession::new_ok("Created an issue. PROGRESS: 20", vec![]);
    let mut bridges = bridges_with_session(session);
    let board = board_with_goal("g1", GoalProgress::NotStarted, None);
    let mut state = OodaState::new(board);
    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g1".into()),
        description: "advance".into(),
    };
    let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
    // The outcome detail should include verification counts.
    assert!(
        outcomes[0].detail.contains("verified="),
        "outcome should include verification counts, got: {}",
        outcomes[0].detail,
    );
}

#[test]
fn objective_includes_concrete_commands() {
    let (session, captured) = MockSession::new_ok("PROGRESS: 10", vec![]);
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
    assert!(
        input.objective.contains("gh issue create"),
        "objective should include concrete gh issue create command"
    );
    assert!(
        input.objective.contains("amplihack copilot"),
        "objective should include amplihack copilot command"
    );
    assert!(
        input.objective.contains("cargo test"),
        "objective should include cargo test command"
    );
}
