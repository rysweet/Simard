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
        obj.contains("simard engineer")
            || obj.contains("simard spawn engineer")
            || obj.contains("amplihack copilot"),
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
        input.objective.contains("amplihack copilot")
            || input.objective.contains("simard spawn engineer"),
        "objective should include delegation command"
    );
    assert!(
        input.objective.contains("cargo test"),
        "objective should include cargo test command"
    );
}

// ===== Issue #929: dispatch outcome wiring tests =====
//
// These tests verify that dispatch_advance_goal records descriptive outcome
// detail strings for each LLM-action branch — no longer leaving outcomes
// empty. They MUST fail until advance_goal_with_session is refactored to
// parse JSON actions and emit branch-specific outcome strings.

#[test]
fn dispatch_records_noop_outcome_detail() {
    // LLM returns a clean noop JSON action.
    let (session, _) = MockSession::new_ok(
        r#"{"action": "noop", "reason": "no work to do this cycle"}"#,
        vec![],
    );
    let mut bridges = bridges_with_session(session);
    let board = board_with_goal("g1", GoalProgress::InProgress { percent: 30 }, None);
    let mut state = OodaState::new(board);
    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g1".into()),
        description: "advance".into(),
    };
    let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

    assert!(outcomes[0].success, "noop should be a successful outcome");
    let detail = outcomes[0].detail.to_lowercase();
    assert!(
        detail.contains("noop"),
        "outcome detail must mention 'noop' branch, got: {}",
        outcomes[0].detail
    );
    assert!(
        detail.contains("no work to do this cycle") || detail.contains("reason"),
        "outcome detail must include the LLM-supplied reason, got: {}",
        outcomes[0].detail
    );
}

#[test]
fn dispatch_records_assess_only_outcome_detail() {
    let (session, _) = MockSession::new_ok(
        r#"{"action": "assess_only", "assessment": "halfway through", "progress_pct": 50}"#,
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
    let detail = outcomes[0].detail.to_lowercase();
    assert!(
        detail.contains("assess_only") || detail.contains("assess only"),
        "outcome detail must mention assess_only branch, got: {}",
        outcomes[0].detail
    );
    assert!(
        outcomes[0].detail.contains("halfway through") || outcomes[0].detail.contains("50"),
        "outcome detail must include the assessment text or progress, got: {}",
        outcomes[0].detail
    );
}

#[test]
fn dispatch_assess_only_updates_progress_from_json() {
    // A clean assess_only JSON with progress_pct should update goal progress
    // — this is the structured replacement for the legacy "PROGRESS:" line.
    let (session, _) = MockSession::new_ok(
        r#"{"action": "assess_only", "assessment": "good", "progress_pct": 75}"#,
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
        GoalProgress::InProgress { percent: 75 },
        "assess_only progress_pct must propagate to goal status"
    );
}

#[test]
fn dispatch_records_parse_failure_fallback_outcome_detail() {
    // LLM returns prose with no JSON — parser returns None and the
    // dispatcher should fall back to legacy assessment, but the outcome
    // detail must indicate the fallback path was taken.
    let (session, _) = MockSession::new_ok(
        "I have no idea what to do. PROGRESS: 30",
        vec![],
    );
    let mut bridges = bridges_with_session(session);
    let board = board_with_goal("g1", GoalProgress::InProgress { percent: 25 }, None);
    let mut state = OodaState::new(board);
    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g1".into()),
        description: "advance".into(),
    };
    let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

    let detail = outcomes[0].detail.to_lowercase();
    assert!(
        detail.contains("parse fail")
            || detail.contains("fell back")
            || detail.contains("fallback")
            || detail.contains("legacy"),
        "outcome detail must indicate parse fallback was used, got: {}",
        outcomes[0].detail
    );
}

#[test]
fn dispatch_outcomes_are_never_empty() {
    // Verify every branch produces a non-empty outcome detail string.
    for response in [
        r#"{"action": "noop", "reason": "nothing"}"#,
        r#"{"action": "assess_only", "assessment": "x", "progress_pct": 10}"#,
        "totally unparseable prose",
    ] {
        let (session, _) = MockSession::new_ok(response, vec![]);
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::InProgress { percent: 5 }, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert!(
            !outcomes[0].detail.trim().is_empty(),
            "outcome detail must never be empty for response: {response:?}"
        );
    }
}

#[test]
fn dispatch_spawn_engineer_outcome_mentions_branch() {
    // For spawn_engineer we cannot exercise real spawn_subordinate in unit
    // tests (it would actually fork the current_exe). But we can still
    // assert the outcome detail mentions the spawn_engineer branch — even
    // if the spawn itself fails, the detail should reflect the attempt.
    //
    // This test uses a deeply-nested depth env var so any future depth gate
    // can short-circuit, but per the design spec the dispatcher attempts
    // spawn_subordinate and reports the result either way.
    //
    // SAFETY: env mutation is process-global; isolated in this test only.
    // SAFETY: env mutation is process-global; isolated in this test only.
    // SAFETY: env mutation is process-global; isolated in this test only.
    unsafe {
        std::env::set_var("SIMARD_SUBORDINATE_DEPTH", "9999");
        std::env::set_var("SIMARD_MAX_SUBORDINATE_DEPTH", "1");
    }

    let (session, _) = MockSession::new_ok(
        r#"{"action": "spawn_engineer", "task": "fix issue 929"}"#,
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
    let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

    let detail = outcomes[0].detail.to_lowercase();
    assert!(
        detail.contains("spawn_engineer") || detail.contains("spawn engineer") || detail.contains("subordinate"),
        "spawn_engineer outcome detail must mention the branch, got: {}",
        outcomes[0].detail
    );

    // SAFETY: cleanup of process-global env after test.
    unsafe {
        std::env::remove_var("SIMARD_SUBORDINATE_DEPTH");
        std::env::remove_var("SIMARD_MAX_SUBORDINATE_DEPTH");
    }
}

// ===== Issue #929: prompt asset contract tests =====

#[test]
fn prompt_asset_instructs_json_output() {
    const ASSET: &str =
        include_str!("../../prompt_assets/simard/goal_session_objective.md");
    let lower = ASSET.to_lowercase();
    assert!(
        lower.contains("json"),
        "goal_session_objective.md must instruct JSON output (issue #929)"
    );
}

#[test]
fn prompt_asset_documents_three_action_variants() {
    const ASSET: &str =
        include_str!("../../prompt_assets/simard/goal_session_objective.md");
    assert!(
        ASSET.contains("spawn_engineer"),
        "prompt asset must document spawn_engineer action"
    );
    assert!(
        ASSET.contains("noop"),
        "prompt asset must document noop action"
    );
    assert!(
        ASSET.contains("assess_only"),
        "prompt asset must document assess_only action"
    );
}
