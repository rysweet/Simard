//! Integration tests for the `simard goal` subcommand introduced in the
//! issue-#1911 fix. Exercises `goal list`, `goal unblock <id>`, and
//! `goal unblock-all` against a temporary `SIMARD_STATE_ROOT` so the
//! tests are hermetic and never touch the operator's live `~/.simard`.
//!
//! These are the canonical TDD-first tests for the CLI surface. The
//! production implementation lives in `src/operator_cli/goal.rs` (created
//! by the issue-#1911 implementation step). Tests written here drive the
//! shape of that module.
//!
//! Isolation: every test uses `#[serial_test::serial(cognitive_memory)]`
//! and a `tempfile::TempDir` overridden via `SIMARD_STATE_ROOT`, matching
//! the established pattern in `src/goals/cognitive_memory_store.rs:223`
//! and `src/memory_ipc/tests_launcher.rs`.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, add_active_goal, save_goal_board};
use crate::memory_ipc::launch_writer_bridge;
use crate::ooda_actions::advance_goal::spawn::{
    BRAIN_FAILURE_BLOCKED_PREFIX, BRAIN_FAILURE_BLOCKED_SUFFIX,
};
use crate::operator_cli::dispatch_operator_cli;

// ─── helpers ────────────────────────────────────────────────────────────────

/// Allocate an isolated state root for a single test. Returned `TempDir`
/// must be kept alive for the duration of the test.
fn isolated_state_root() -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let root = tmp.path().to_path_buf();
    // Set BEFORE launching any bridge so the writer + reader land in the
    // same isolated directory.
    // SAFETY: tests are serialised via `#[serial_test::serial(cognitive_memory)]`,
    // so concurrent env mutation is excluded by the harness.
    unsafe {
        std::env::set_var("SIMARD_STATE_ROOT", &root);
    }
    (tmp, root)
}

/// Seed a goal board into cognitive memory at the given state root.
/// Mirrors what `simard ooda` would have persisted before being shut down.
fn seed_board(root: &Path, goals: Vec<ActiveGoal>) {
    let mut board = GoalBoard::new();
    for goal in goals {
        add_active_goal(&mut board, goal).expect("add goal under MAX_ACTIVE_GOALS");
    }
    let writer = launch_writer_bridge(root).expect("writer bridge");
    save_goal_board(&board, writer.ops()).expect("save board");
}

fn marker_reason(consecutive: u32) -> String {
    format!("{BRAIN_FAILURE_BLOCKED_PREFIX}{consecutive}{BRAIN_FAILURE_BLOCKED_SUFFIX}")
}

fn active_goal(id: &str, status: GoalProgress) -> ActiveGoal {
    ActiveGoal {
        id: id.to_string(),
        description: format!("Goal {id}"),
        priority: 1,
        status,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    }
}

/// Re-read the persisted goal board from cognitive memory at `root`.
fn load_board(root: &Path) -> GoalBoard {
    let writer = launch_writer_bridge(root).expect("writer bridge");
    crate::goal_curation::load_goal_board(writer.ops()).expect("load board")
}

// ─── T7 — `simard goal list` schema and empty-board rendering ───────────────

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_list_succeeds_on_empty_board() {
    let (_tmp, _root) = isolated_state_root();
    let result = dispatch_operator_cli(vec!["goal".to_string(), "list".to_string()]);
    assert!(
        result.is_ok(),
        "`simard goal list` against an empty state root must exit 0; \
         got: {:?}",
        result.err().map(|e| e.to_string())
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_list_succeeds_with_active_goals_present() {
    let (_tmp, root) = isolated_state_root();
    seed_board(
        &root,
        vec![
            active_goal("alpha", GoalProgress::NotStarted),
            active_goal("beta-1", GoalProgress::Blocked(marker_reason(3))),
            active_goal("gamma", GoalProgress::InProgress { percent: 42 }),
        ],
    );

    let result = dispatch_operator_cli(vec!["goal".to_string(), "list".to_string()]);
    assert!(
        result.is_ok(),
        "`simard goal list` with an active board must exit 0; got: {:?}",
        result.err().map(|e| e.to_string())
    );
}

// ─── single-id `simard goal unblock <id>` — unconditional override ──────────

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_unblock_clears_marker_blocked_goal() {
    let (_tmp, root) = isolated_state_root();
    seed_board(
        &root,
        vec![active_goal(
            "stuck-goal",
            GoalProgress::Blocked(marker_reason(3)),
        )],
    );

    let result = dispatch_operator_cli(vec![
        "goal".to_string(),
        "unblock".to_string(),
        "stuck-goal".to_string(),
    ]);
    assert!(
        result.is_ok(),
        "`simard goal unblock stuck-goal` must exit 0; got: {:?}",
        result.err().map(|e| e.to_string())
    );

    let board = load_board(&root);
    let g = board
        .active
        .iter()
        .find(|g| g.id == "stuck-goal")
        .expect("goal must survive unblock");
    assert_eq!(
        g.status,
        GoalProgress::NotStarted,
        "single-id unblock must restore status to NotStarted; got {:?}",
        g.status
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_unblock_clears_any_blocked_reason_unconditionally() {
    // A1/A4 in the design spec: single-id `unblock` is the operator
    // escape hatch — it clears `Blocked` regardless of the reason text.
    // `unblock-all` is the narrowly scoped bulk-clear (marker only).
    let (_tmp, root) = isolated_state_root();
    seed_board(
        &root,
        vec![active_goal(
            "operator-blocked",
            GoalProgress::Blocked("waiting on human review".into()),
        )],
    );

    let result = dispatch_operator_cli(vec![
        "goal".to_string(),
        "unblock".to_string(),
        "operator-blocked".to_string(),
    ]);
    assert!(
        result.is_ok(),
        "single-id unblock must override even non-marker Blocked reasons; \
         got: {:?}",
        result.err().map(|e| e.to_string())
    );

    let board = load_board(&root);
    let g = board
        .active
        .iter()
        .find(|g| g.id == "operator-blocked")
        .expect("goal must survive unblock");
    assert_eq!(g.status, GoalProgress::NotStarted);
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_unblock_unknown_id_returns_error() {
    let (_tmp, _root) = isolated_state_root();
    let result = dispatch_operator_cli(vec![
        "goal".to_string(),
        "unblock".to_string(),
        "no-such-goal".to_string(),
    ]);
    assert!(
        result.is_err(),
        "unblock of unknown goal id must return a non-zero exit"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("no-such-goal"),
        "error must name the unknown goal id; got: {msg}"
    );
}

// ─── bulk `simard goal unblock-all` — scoped to brain-failure marker ────────

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_unblock_all_clears_only_marker_blocked_goals() {
    // Mixed board: 2 marker-blocked, 1 operator-blocked, 1 in-progress.
    // `unblock-all` must clear the 2 marker-blocked goals back to
    // NotStarted and leave the other two untouched.
    let (_tmp, root) = isolated_state_root();
    seed_board(
        &root,
        vec![
            active_goal("stuck-a", GoalProgress::Blocked(marker_reason(3))),
            active_goal("stuck-b", GoalProgress::Blocked(marker_reason(7))),
            active_goal(
                "operator-blocked",
                GoalProgress::Blocked("waiting on human review".into()),
            ),
            active_goal("working", GoalProgress::InProgress { percent: 50 }),
        ],
    );

    let result = dispatch_operator_cli(vec!["goal".to_string(), "unblock-all".to_string()]);
    assert!(
        result.is_ok(),
        "`simard goal unblock-all` must exit 0; got: {:?}",
        result.err().map(|e| e.to_string())
    );

    let board = load_board(&root);

    // Marker-blocked goals were cleared.
    for id in ["stuck-a", "stuck-b"] {
        let g = board
            .active
            .iter()
            .find(|g| g.id == id)
            .unwrap_or_else(|| panic!("goal {id} must survive unblock-all"));
        assert_eq!(
            g.status,
            GoalProgress::NotStarted,
            "marker-blocked goal {id} must be NotStarted after unblock-all; \
             got {:?}",
            g.status
        );
    }

    // Operator-set Blocked must be untouched.
    let op = board
        .active
        .iter()
        .find(|g| g.id == "operator-blocked")
        .expect("operator-blocked goal must survive");
    assert!(
        matches!(&op.status, GoalProgress::Blocked(r) if r == "waiting on human review"),
        "unblock-all must NOT clear non-marker Blocked goals; got {:?}",
        op.status
    );

    // InProgress must remain InProgress.
    let working = board
        .active
        .iter()
        .find(|g| g.id == "working")
        .expect("working goal must survive");
    assert_eq!(working.status, GoalProgress::InProgress { percent: 50 });
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_unblock_all_on_empty_board_succeeds_as_noop() {
    // Operator runbook safety: unblock-all is idempotent and never errors
    // on an empty board.
    let (_tmp, _root) = isolated_state_root();
    let result = dispatch_operator_cli(vec!["goal".to_string(), "unblock-all".to_string()]);
    assert!(
        result.is_ok(),
        "unblock-all must be a no-op on empty board; got: {:?}",
        result.err().map(|e| e.to_string())
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_unblock_all_does_not_touch_completed_or_in_progress_goals() {
    let (_tmp, root) = isolated_state_root();
    seed_board(
        &root,
        vec![
            active_goal("done-1", GoalProgress::Completed),
            active_goal("running", GoalProgress::InProgress { percent: 88 }),
            active_goal("pending", GoalProgress::NotStarted),
        ],
    );

    let result = dispatch_operator_cli(vec!["goal".to_string(), "unblock-all".to_string()]);
    assert!(result.is_ok());

    let board = load_board(&root);
    let by_id = |id: &str| {
        board
            .active
            .iter()
            .find(|g| g.id == id)
            .unwrap_or_else(|| panic!("goal {id} missing"))
            .clone()
    };
    assert_eq!(by_id("done-1").status, GoalProgress::Completed);
    assert_eq!(
        by_id("running").status,
        GoalProgress::InProgress { percent: 88 }
    );
    assert_eq!(by_id("pending").status, GoalProgress::NotStarted);
}

// ─── `simard goal delete <id>` — operator escape hatch for fabricated/corrupt goals ──
//
// Motivating incident: synthetic placeholder goals (e.g. `stuck-b` with
// description "Goal stuck-b") leaked into the production OODA board via
// the cycle-5→6 corruption pathway tracked in issue #1915. Operators
// need a CLI to remove these without raw `cognitive_memory.ladybug`
// mutation. Tests below cover the three required behaviours:
//   1. delete-success removes the named goal and preserves the rest
//   2. unknown-id is a hard error (no silent no-op)
//   3. backlog items are not touched

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_delete_removes_named_goal_and_preserves_siblings() {
    let (_tmp, root) = isolated_state_root();
    seed_board(
        &root,
        vec![
            active_goal("alpha-1", GoalProgress::NotStarted),
            active_goal("beta-2", GoalProgress::InProgress { percent: 25 }),
            active_goal("gamma-3", GoalProgress::Completed),
        ],
    );

    let result = dispatch_operator_cli(vec![
        "goal".to_string(),
        "delete".to_string(),
        "beta-2".to_string(),
    ]);
    assert!(
        result.is_ok(),
        "`simard goal delete beta-2` must exit 0; got: {:?}",
        result.err().map(|e| e.to_string())
    );

    let board = load_board(&root);
    assert!(
        board.active.iter().all(|g| g.id != "beta-2"),
        "deleted goal must be absent from the board; got ids: {:?}",
        board.active.iter().map(|g| &g.id).collect::<Vec<_>>()
    );
    // Siblings must survive untouched.
    let surviving_ids: Vec<&str> = board.active.iter().map(|g| g.id.as_str()).collect();
    assert!(
        surviving_ids.contains(&"alpha-1") && surviving_ids.contains(&"gamma-3"),
        "sibling goals must survive delete; got: {surviving_ids:?}"
    );
    assert_eq!(
        board.active.len(),
        2,
        "exactly one goal must be removed; got {} active goal(s)",
        board.active.len()
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_delete_unknown_id_returns_error_without_mutating_board() {
    let (_tmp, root) = isolated_state_root();
    seed_board(
        &root,
        vec![active_goal("alpha-1", GoalProgress::NotStarted)],
    );

    let result = dispatch_operator_cli(vec![
        "goal".to_string(),
        "delete".to_string(),
        "no-such-goal".to_string(),
    ]);
    assert!(
        result.is_err(),
        "delete of unknown goal id must return a non-zero exit"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("no-such-goal"),
        "error must name the unknown goal id; got: {msg}"
    );

    // Board must be untouched.
    let board = load_board(&root);
    assert_eq!(board.active.len(), 1, "board must be unchanged on error");
    assert_eq!(board.active[0].id, "alpha-1");
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_delete_clears_brain_failure_marker_goals_via_id() {
    // Regression scenario from the `stuck-b` engineer brief: a goal stuck
    // on the brain-failure safeguard marker (or any other Blocked reason)
    // can be deleted by id when the operator confirms it is fabricated.
    use crate::ooda_actions::advance_goal::spawn::{
        BRAIN_FAILURE_BLOCKED_PREFIX, BRAIN_FAILURE_BLOCKED_SUFFIX,
    };
    let marker = format!("{BRAIN_FAILURE_BLOCKED_PREFIX}5{BRAIN_FAILURE_BLOCKED_SUFFIX}");
    let (_tmp, root) = isolated_state_root();
    seed_board(
        &root,
        vec![
            active_goal("legit-goal", GoalProgress::NotStarted),
            active_goal("fabricated", GoalProgress::Blocked(marker)),
        ],
    );

    let result = dispatch_operator_cli(vec![
        "goal".to_string(),
        "delete".to_string(),
        "fabricated".to_string(),
    ]);
    assert!(
        result.is_ok(),
        "delete must remove a Blocked goal regardless of reason: {:?}",
        result.err().map(|e| e.to_string())
    );

    let board = load_board(&root);
    assert_eq!(board.active.len(), 1);
    assert_eq!(board.active[0].id, "legit-goal");
}
