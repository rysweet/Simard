use super::advance_goal::*;

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
    let outcome = validate_subordinate_completion(&action, &mut state, "g1", "sub-ok", &progress);
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
    let outcome =
        validate_subordinate_completion(&action, &mut state, "g1", "sub-empty", &progress);
    assert!(
        !outcome.success,
        "should fail when no artifacts: {}",
        outcome.detail
    );
    assert!(outcome.detail.contains("0 commits"));
    assert!(outcome.detail.contains("0 PRs"));
}

#[cfg(test)]
mod inflight_tests {
    use super::find_live_engineer_for_goal;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn find_live_engineer_returns_none_when_root_missing() {
        let tmp = tempdir().unwrap();
        // No engineer-worktrees subdir exists.
        let result = find_live_engineer_for_goal(tmp.path(), "any-goal");
        assert!(result.is_none());
    }

    #[test]
    fn find_live_engineer_returns_none_when_no_matching_dir() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join(crate::engineer_worktree::WORKTREES_SUBDIR);
        fs::create_dir_all(&root).unwrap();
        let other = root.join("different-goal-1234-abc");
        fs::create_dir_all(&other).unwrap();
        fs::write(
            other.join(crate::engineer_worktree::ENGINEER_CLAIM_FILE),
            format!("{}\n", std::process::id()),
        )
        .unwrap();
        let result = find_live_engineer_for_goal(tmp.path(), "wanted-goal");
        assert!(result.is_none());
    }

    #[test]
    fn find_live_engineer_returns_path_when_claim_alive() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join(crate::engineer_worktree::WORKTREES_SUBDIR);
        fs::create_dir_all(&root).unwrap();
        let live = root.join("my-goal-1777000000-deadbe");
        fs::create_dir_all(&live).unwrap();
        fs::write(
            live.join(crate::engineer_worktree::ENGINEER_CLAIM_FILE),
            format!("{}\n", std::process::id()),
        )
        .unwrap();
        let result = find_live_engineer_for_goal(tmp.path(), "my-goal");
        assert_eq!(result, Some(live));
    }

    #[test]
    fn find_live_engineer_returns_none_when_claim_dead() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join(crate::engineer_worktree::WORKTREES_SUBDIR);
        fs::create_dir_all(&root).unwrap();
        let dead = root.join("ghost-goal-0-cafe");
        fs::create_dir_all(&dead).unwrap();
        fs::write(
            dead.join(crate::engineer_worktree::ENGINEER_CLAIM_FILE),
            "2147483646\n",
        )
        .unwrap();
        let result = find_live_engineer_for_goal(tmp.path(), "ghost-goal");
        assert!(result.is_none());
    }

    #[test]
    fn find_live_engineer_returns_none_when_claim_file_missing() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join(crate::engineer_worktree::WORKTREES_SUBDIR);
        fs::create_dir_all(&root).unwrap();
        let no_claim = root.join("orphan-goal-1-aa");
        fs::create_dir_all(&no_claim).unwrap();
        // No sentinel file written — pre-#1213 worktrees, or partial allocate.
        let result = find_live_engineer_for_goal(tmp.path(), "orphan-goal");
        assert!(result.is_none());
    }

    #[test]
    fn find_live_engineer_only_matches_exact_goal_prefix() {
        // Goals "foo" and "foo-extended" must not collide. The match
        // prefix is `<goal_id>-`, so "foo-1234-abc" matches goal "foo"
        // but NOT goal "foo-extended" (which would need "foo-extended-...").
        let tmp = tempdir().unwrap();
        let root = tmp.path().join(crate::engineer_worktree::WORKTREES_SUBDIR);
        fs::create_dir_all(&root).unwrap();
        let foo_wt = root.join("foo-1234-abc");
        fs::create_dir_all(&foo_wt).unwrap();
        fs::write(
            foo_wt.join(crate::engineer_worktree::ENGINEER_CLAIM_FILE),
            format!("{}\n", std::process::id()),
        )
        .unwrap();

        // Goal "foo" matches.
        assert_eq!(find_live_engineer_for_goal(tmp.path(), "foo"), Some(foo_wt));
        // Goal "foo-extended" does NOT match.
        assert!(find_live_engineer_for_goal(tmp.path(), "foo-extended").is_none());
        // Goal "fo" does NOT match (prefix is "fo-" which doesn't match "foo-1234").
        assert!(find_live_engineer_for_goal(tmp.path(), "fo").is_none());
    }
}
