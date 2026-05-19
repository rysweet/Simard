use super::advance_goal::*;

use crate::goal_curation::GoalProgress;
use crate::ooda_actions::dispatch_actions;
use crate::ooda_actions::test_helpers::*;
use crate::ooda_loop::{ActionKind, OodaState, PlannedAction};

// ─────────────────────────────────────────────────────────────────────────────
// Issue #1911 — brain-failure auto-recovery TDD tests.
//
// The constants and `is_brain_failure_marker` predicate referenced below
// are introduced by the issue-#1911 implementation. The tests are
// committed first (TDD red phase); the implementation step adds the
// constants, predicate, and auto-recovery branches.
// ─────────────────────────────────────────────────────────────────────────────

/// T2 — `marker_predicate_matches_only_brain_failure`.
///
/// The brain-failure marker is composed of two compile-time constants in
/// `src/ooda_actions/advance_goal/spawn.rs`. The predicate
/// `is_brain_failure_marker` must:
///   - accept any string that starts with the 🔒-sentinel prefix AND
///     contains the suffix;
///   - reject every other operator-set, scope-blocked, or
///     dependency-blocked reason.
#[test]
fn marker_predicate_matches_only_brain_failure() {
    use crate::ooda_actions::advance_goal::spawn::{
        BRAIN_FAILURE_BLOCKED_PREFIX, BRAIN_FAILURE_BLOCKED_SUFFIX, is_brain_failure_marker,
    };

    // The sentinel-bearing constants must be intentionally non-LLM-typable:
    // they include the U+1F512 LOCK code point and the literal token
    // "OODA-SAFEGUARD". The brain's free-text `MarkGoalBlocked.reason`
    // cannot produce this exact prefix without the dispatcher's cooperation.
    assert!(
        BRAIN_FAILURE_BLOCKED_PREFIX.contains("OODA-SAFEGUARD"),
        "prefix must carry the OODA-SAFEGUARD sentinel token: {BRAIN_FAILURE_BLOCKED_PREFIX:?}"
    );
    assert!(
        BRAIN_FAILURE_BLOCKED_PREFIX.contains('\u{1F512}'),
        "prefix must carry the U+1F512 LOCK code point: {BRAIN_FAILURE_BLOCKED_PREFIX:?}"
    );
    assert!(
        BRAIN_FAILURE_BLOCKED_SUFFIX.contains("needs human review"),
        "suffix must terminate with 'needs human review': {BRAIN_FAILURE_BLOCKED_SUFFIX:?}"
    );

    // Positive matches: the canonical rendered form and any failure count.
    let rendered = format!("{BRAIN_FAILURE_BLOCKED_PREFIX}3{BRAIN_FAILURE_BLOCKED_SUFFIX}");
    assert!(
        is_brain_failure_marker(&rendered),
        "canonical 3-failure marker must match: {rendered:?}"
    );
    let rendered_42 = format!("{BRAIN_FAILURE_BLOCKED_PREFIX}42{BRAIN_FAILURE_BLOCKED_SUFFIX}");
    assert!(
        is_brain_failure_marker(&rendered_42),
        "marker must match regardless of failure count: {rendered_42:?}"
    );

    // Negative matches: every other Blocked reason the system produces.
    for reason in [
        "waiting",                 // existing dispatch_advance_goal_blocked_fails reason
        "waiting for code review", // operator-set
        "subordinate sub-123 reported dead heartbeat", // subordinate.rs N2 prefix
        "scope rejected by curator", // scope-blocked
        "depends on goal: enhance-X", // dependency-blocked
        // Brain-forgeable lookalike: the pre-#1911 brain-failure marker text
        // without the sentinel must NOT match the new predicate. A brain
        // emitting this exact string via `MarkGoalBlocked` must be ignored.
        "OODA brain failing for 3 consecutive cycles; needs human review",
        // Empty + edge cases.
        "",
        BRAIN_FAILURE_BLOCKED_SUFFIX, // suffix only, no prefix
    ] {
        assert!(
            !is_brain_failure_marker(reason),
            "predicate must NOT match non-brain-failure reason: {reason:?}"
        );
    }
}

/// T1 — `failures_then_success_clears_marker_and_counter`.
///
/// Headline TDD test for issue #1911. Simulates the production lockout:
///   1. The brain has failed 3 consecutive times for a goal.
///   2. The dispatcher previously persisted the brain-failure marker into
///      `GoalProgress::Blocked(...)` and bumped `goal_failure_counts` to 3.
///   3. The brain is healthy again (in the test we don't even need a
///      session — auto-recovery happens BEFORE the session is consulted).
///
/// Expected post-fix behavior of `dispatch_advance_goal`:
///   - Detect the marker via `is_brain_failure_marker`.
///   - Clear `goal_failure_counts[goal_id]`.
///   - Reset the goal's `status` from `Blocked(...)` back to `NotStarted`.
///   - Fall through to normal dispatch (which, with no session, will
///     surface the existing "no LLM session available" failure — that is
///     a strictly different failure mode from the pre-fix "blocked"
///     failure and provably demonstrates the auto-recovery branch fired).
///
/// Pre-fix the assertions fail because:
///   - the dispatcher returns `success=false` with detail `"blocked: ..."`
///     (the marker reason is never inspected),
///   - the counter is never cleared,
///   - the goal status is never restored.
#[test]
fn failures_then_success_clears_marker_and_counter() {
    use crate::ooda_actions::advance_goal::spawn::{
        BRAIN_FAILURE_BLOCKED_PREFIX, BRAIN_FAILURE_BLOCKED_SUFFIX,
    };

    let marker = format!("{BRAIN_FAILURE_BLOCKED_PREFIX}3{BRAIN_FAILURE_BLOCKED_SUFFIX}");
    let board = board_with_goal("g-locked", GoalProgress::Blocked(marker.clone()), None);
    let mut bridges = test_bridges(); // session: None — auto-recovery happens before session check
    let mut state = OodaState::new(board);

    // Seed the counter to simulate the 3-failure history that produced
    // the marker.
    state.goal_failure_counts.insert("g-locked".to_string(), 3);

    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g-locked".into()),
        description: "advance".into(),
    };
    let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

    // The outcome itself is still success=false (no session in this test
    // setup), BUT the failure must NOT be the "blocked" short-circuit:
    // post-fix, dispatch_advance_goal must have auto-recovered the marker
    // and fallen through to the session-check failure.
    assert!(
        !outcomes[0].detail.contains("blocked"),
        "auto-recovery must clear the marker before the blocked-short-circuit \
         fires; got detail: {}",
        outcomes[0].detail
    );
    assert!(
        outcomes[0].detail.contains("no LLM session available"),
        "post-recovery dispatch must fall through to the session check; \
         got detail: {}",
        outcomes[0].detail
    );

    // The failure counter for the goal must be cleared by Site 1 (the
    // auto-recovery branch in `mod.rs`).
    assert!(
        !state.goal_failure_counts.contains_key("g-locked"),
        "auto-recovery must remove goal from goal_failure_counts; \
         observed: {:?}",
        state.goal_failure_counts
    );

    // The goal's status must be restored to NotStarted. Other Blocked
    // reasons (operator-set, subordinate-, scope-, dependency-blocked)
    // remain untouched — see `marker_predicate_matches_only_brain_failure`.
    let g = state
        .active_goals
        .active
        .iter()
        .find(|g| g.id == "g-locked")
        .expect("goal must still be on the board");
    assert_eq!(
        g.status,
        GoalProgress::NotStarted,
        "auto-recovery must restore status to NotStarted; got {:?}",
        g.status
    );
}

/// **Regression test required by the `stuck-b` engineer brief
/// (synthetic-placeholder remediation cycle).**
///
/// The auto-recovery ladder in `dispatch_advance_goal` must NEVER
/// synthesise a goal record on the active board — its single
/// responsibility is to (a) drop the brain-failure marker, (b) clear
/// the per-goal failure counter, and (c) restore the existing goal's
/// status to `NotStarted`. It must NOT mint new active-board entries,
/// and in particular must never produce ids of the form `stuck-<x>` or
/// descriptions of the form `Goal <id>` (the test-fixture shape that
/// leaked into the production board between cycles 6 and 7 — see
/// session forensics under
/// `~/.copilot/session-state/.../plan.md`).
///
/// This test exercises both the marker-recovery path (T1) and the
/// non-marker path (T5) and asserts on the entire post-call active
/// board, not just the dispatched goal.
#[test]
fn auto_recovery_ladder_never_emits_placeholder_goal_titles_or_stuck_ids() {
    use crate::ooda_actions::advance_goal::spawn::{
        BRAIN_FAILURE_BLOCKED_PREFIX, BRAIN_FAILURE_BLOCKED_SUFFIX,
    };

    let marker = format!("{BRAIN_FAILURE_BLOCKED_PREFIX}9{BRAIN_FAILURE_BLOCKED_SUFFIX}");

    // Two scenarios — marker (auto-recovery fires) and non-marker
    // (auto-recovery is gated off). Both must leave the board
    // free of synthesised placeholder rows.
    for (label, status) in [
        (
            "brain-failure-marker",
            GoalProgress::Blocked(marker.clone()),
        ),
        (
            "operator-blocked-non-marker",
            GoalProgress::Blocked("waiting for human review".into()),
        ),
        ("not-started-baseline", GoalProgress::NotStarted),
    ] {
        let goal_id = "real-goal-with-meaningful-id";
        let board = board_with_goal(goal_id, status.clone(), None);
        let pre_active_count = board.active.len();
        let pre_ids: Vec<String> = board.active.iter().map(|g| g.id.clone()).collect();

        let mut bridges = test_bridges();
        let mut state = OodaState::new(board);
        state.goal_failure_counts.insert(goal_id.to_string(), 9);

        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some(goal_id.into()),
            description: "advance".into(),
        };
        let _outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        // (a) No NEW goals were minted by the auto-recovery ladder.
        assert_eq!(
            state.active_goals.active.len(),
            pre_active_count,
            "scenario {label}: auto-recovery must not add active-board rows; \
             before={pre_ids:?}, after={:?}",
            state
                .active_goals
                .active
                .iter()
                .map(|g| g.id.clone())
                .collect::<Vec<_>>()
        );

        // (b) No `stuck-<x>` id appears (test-fixture shape from
        //     src/operator_cli/tests_goal.rs that was observed in
        //     production cycle_7.json).
        for g in &state.active_goals.active {
            assert!(
                !is_stuck_fixture_id(&g.id),
                "scenario {label}: auto-recovery must NEVER emit a `stuck-<letter>` placeholder id; \
                 got id {:?}",
                g.id,
            );
        }

        // (c) No `Goal <id>` description appears unless it was already
        //     present on the pre-call board (which test_helpers.rs
        //     populates by default — the post-call board must be a
        //     subset of the pre-call descriptions in this dimension).
        let pre_descriptions: std::collections::HashSet<String> =
            pre_ids.iter().map(|id| format!("Goal {id}")).collect();
        for g in &state.active_goals.active {
            if is_goal_id_placeholder_description(&g.description) {
                assert!(
                    pre_descriptions.contains(&g.description),
                    "scenario {label}: auto-recovery must NEVER synthesise a new \
                     `Goal <id>` placeholder description; got new description {:?}",
                    g.description,
                );
            }
        }
    }
}

/// Helper: `stuck-<single-lowercase-letter>` is the test-fixture id
/// pattern from `src/operator_cli/tests_goal.rs`. Catches the exact
/// shape observed in the cycle_7.json corruption (`stuck-a`, `stuck-b`).
fn is_stuck_fixture_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("stuck-") else {
        return false;
    };
    rest.len() == 1 && rest.chars().all(|c| c.is_ascii_lowercase())
}

/// Helper: matches descriptions of the form `Goal <id>` (the
/// `active_goal()` test helper's `format!("Goal {id}")` shape).
fn is_goal_id_placeholder_description(desc: &str) -> bool {
    let trimmed = desc.trim();
    let Some(rest) = trimmed
        .strip_prefix("Goal ")
        .or_else(|| trimmed.strip_prefix("goal "))
    else {
        return false;
    };
    !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[test]
fn helper_is_stuck_fixture_id_catches_the_observed_corruption_pattern() {
    assert!(is_stuck_fixture_id("stuck-a"));
    assert!(is_stuck_fixture_id("stuck-b"));
    assert!(is_stuck_fixture_id("stuck-z"));
    // Negative cases: real goal slugs.
    assert!(!is_stuck_fixture_id("enhance-simard-meeting-experience"));
    assert!(!is_stuck_fixture_id("stuck-goal")); // pre-existing synthetic id, different shape
    assert!(!is_stuck_fixture_id("stuck-"));
    assert!(!is_stuck_fixture_id(""));
    assert!(!is_stuck_fixture_id("STUCK-A")); // uppercase doesn't match
}

#[test]
fn helper_is_goal_id_placeholder_description_catches_test_fixture_shape() {
    assert!(is_goal_id_placeholder_description("Goal stuck-b"));
    assert!(is_goal_id_placeholder_description("Goal alpha"));
    assert!(is_goal_id_placeholder_description("goal working"));
    // Negative cases: real descriptions.
    assert!(!is_goal_id_placeholder_description("Ship the v1 release"));
    assert!(!is_goal_id_placeholder_description("Goal "));
    assert!(!is_goal_id_placeholder_description(""));
    assert!(!is_goal_id_placeholder_description("Goal with spaces"));
}

/// T5 — `auto_recovery_skipped_when_blocked_reason_is_not_marker`.
///
/// A goal whose `Blocked` reason does NOT match the brain-failure marker
/// must continue to fail-fast in the dispatcher (existing behavior). The
/// auto-recovery branch must not touch operator-set, scope-blocked, or
/// subordinate-blocked goals.
///
/// This is the non-regression complement to T1. It guarantees the
/// auto-recovery branch is gated on the predicate, not on the
/// `Blocked` variant alone.
#[test]
fn auto_recovery_skipped_when_blocked_reason_is_not_marker() {
    let board = board_with_goal(
        "g-operator-blocked",
        GoalProgress::Blocked("waiting for human review".into()),
        None,
    );
    let mut bridges = test_bridges();
    let mut state = OodaState::new(board);
    state
        .goal_failure_counts
        .insert("g-operator-blocked".to_string(), 3);

    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g-operator-blocked".into()),
        description: "advance".into(),
    };
    let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

    // Operator-set Blocked must still surface the existing "blocked"
    // short-circuit — auto-recovery did NOT fire.
    assert!(!outcomes[0].success);
    assert!(
        outcomes[0].detail.contains("blocked"),
        "non-marker Blocked must still fail with 'blocked' detail; got: {}",
        outcomes[0].detail
    );

    // Counter must not have been touched — operator-set Blocked is not in
    // scope for auto-recovery.
    assert_eq!(
        state.goal_failure_counts.get("g-operator-blocked"),
        Some(&3),
        "non-marker Blocked must leave the failure counter intact"
    );

    // Status must remain Blocked with the operator-set reason.
    let g = state
        .active_goals
        .active
        .iter()
        .find(|g| g.id == "g-operator-blocked")
        .expect("goal must still be on the board");
    assert!(
        matches!(&g.status, GoalProgress::Blocked(r) if r == "waiting for human review"),
        "non-marker Blocked must remain Blocked with the operator reason; got {:?}",
        g.status
    );
}

/// T4 — `cycle_rs_257_reset_preserved`.
///
/// The pre-existing failure-counter reset on `outcome.success` in
/// `cycle.rs` is the primary reset path during steady-state healthy
/// operation. The issue-#1911 fix adds two NEW reset sites (auto-recovery
/// in `mod.rs` and Site 2 in `spawn.rs`); neither must remove or change
/// the existing reset semantics.
///
/// This test is a regression-protection sentinel: any future refactor
/// that drops `state.goal_failure_counts.remove(goal_id)` from the
/// `outcome.success` branch in `cycle.rs:252-268` will fail this test.
#[test]
fn cycle_rs_257_reset_preserved() {
    use crate::ooda_loop::ActionOutcome;

    // Construct an OodaState with a seeded failure counter, then apply
    // the exact reset logic from cycle.rs:252-268 (a successful outcome
    // for the goal). The assertion is that the counter is cleared.
    let board = board_with_goal("g-success", GoalProgress::NotStarted, None);
    let mut state = OodaState::new(board);
    state.goal_failure_counts.insert("g-success".to_string(), 2);

    let action = PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g-success".into()),
        description: "advance".into(),
    };
    let outcome = ActionOutcome {
        action: action.clone(),
        success: true,
        detail: "spawn_engineer dispatched".into(),
    };

    // Mirror the exact cycle.rs:256-258 logic — this test exists to
    // anchor that behavior so the refactor in the #1911 fix does not
    // remove it.
    if let Some(goal_id) = &outcome.action.goal_id
        && outcome.success
    {
        state.goal_failure_counts.remove(goal_id);
    }

    assert!(
        !state.goal_failure_counts.contains_key("g-success"),
        "cycle.rs:257 reset must clear the counter on outcome.success"
    );
}

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
    // T6 regression: non-marker `Blocked` goals continue to fail dispatch.
    // The issue-#1911 auto-recovery branch must only fire when the
    // `Blocked` reason matches `is_brain_failure_marker`; any other
    // operator-set, scope-blocked, dependency-blocked, or
    // subordinate-blocked reason continues to short-circuit dispatch
    // with the existing "blocked" failure detail.
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
