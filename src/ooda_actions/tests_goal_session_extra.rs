use crate::goal_curation::GoalProgress;
use crate::ooda_actions::dispatch_actions;
use crate::ooda_actions::test_helpers::*;
use crate::ooda_loop::{ActionKind, OodaState, PlannedAction};

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
fn dispatch_records_parse_failure_outcome_detail() {
    // LLM returns prose with no JSON — parser returns None and the
    // dispatcher MUST fail loudly (no silent fallback). The outcome detail
    // must explain the parse failure so operators can diagnose.
    let (session, _) = MockSession::new_ok("I have no idea what to do. PROGRESS: 30", vec![]);
    let mut bridges = bridges_with_session(session);
    let board = board_with_goal("g1", GoalProgress::InProgress { percent: 25 }, None);
    let mut state = OodaState::new(board);
    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g1".into()),
        description: "advance".into(),
    };
    let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

    assert!(
        !outcomes[0].success,
        "non-JSON LLM response must produce a failed outcome, got success"
    );
    let detail = outcomes[0].detail.to_lowercase();
    assert!(
        detail.contains("parse failed"),
        "outcome detail must explain the parse failure, got: {}",
        outcomes[0].detail
    );
    // Progress MUST stay at 25% (no silent mutation).
    assert_eq!(
        state.active_goals.active[0].status,
        GoalProgress::InProgress { percent: 25 },
        "progress must be preserved when LLM emits invalid action"
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
        detail.contains("spawn_engineer")
            || detail.contains("spawn engineer")
            || detail.contains("subordinate"),
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
    const ASSET: &str = include_str!("../../prompt_assets/simard/goal_session_objective.md");
    let lower = ASSET.to_lowercase();
    assert!(
        lower.contains("json"),
        "goal_session_objective.md must instruct JSON output (issue #929)"
    );
}

#[test]
fn prompt_asset_documents_all_action_variants() {
    const ASSET: &str = include_str!("../../prompt_assets/simard/goal_session_objective.md");
    for variant in [
        "spawn_engineer",
        "noop",
        "assess_only",
        "gh_issue_create",
        "gh_issue_comment",
        "gh_issue_close",
        "gh_pr_comment",
    ] {
        assert!(
            ASSET.contains(variant),
            "prompt asset must document {variant} action"
        );
    }
}
