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
use crate::memory_ipc::launch_writer_client;
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
    let writer = launch_writer_client(root).expect("writer bridge");
    save_goal_board(&board, writer.ops()).expect("save board");
}

fn marker_reason(consecutive: u32) -> String {
    format!("{BRAIN_FAILURE_BLOCKED_PREFIX}{consecutive}{BRAIN_FAILURE_BLOCKED_SUFFIX}")
}

fn active_goal(id: &str, status: GoalProgress) -> ActiveGoal {
    ActiveGoal {
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: id.to_string(),
        description: format!("Goal {id}"),
        priority: 1,
        status,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    }
}

/// Re-read the persisted goal board from cognitive memory at `root`.
fn load_board(root: &Path) -> GoalBoard {
    let writer = launch_writer_client(root).expect("writer bridge");
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

// ─── Issue #1: steerability — operator edits stick, survive cycles/restarts ──

/// An operator `goal add` is reflected by the very next read of the
/// authoritative store (read-your-writes) and survives a daemon restart.
#[test]
#[serial_test::serial(cognitive_memory)]
fn operator_add_reflected_immediately_and_survives_restart() {
    let (_tmp, root) = isolated_state_root();

    dispatch_operator_cli(vec![
        "goal".to_string(),
        "add".to_string(),
        "1".to_string(),
        "steer".to_string(),
        "the".to_string(),
        "board".to_string(),
    ])
    .expect("`goal add` must succeed");

    let id = crate::goals::goal_slug("steer the board");

    // Reflected immediately in the authoritative store.
    let persistent = crate::goal_board_store::load(&root);
    assert!(
        persistent.board.active.iter().any(|g| g.id == id),
        "operator add must be reflected by the next authoritative read"
    );

    // Survives a daemon restart (a fresh load from goal_board.json).
    let after_restart = crate::goal_board_store::load(&root);
    assert!(
        after_restart.board.active.iter().any(|g| g.id == id),
        "operator add must survive a restart"
    );
}

/// An operator `goal remove` survives a full daemon cycle: even though the
/// daemon's in-flight board still carries the removed goal (it loaded before the
/// remove), the tombstone-aware reconcile at cycle-commit drops it — the
/// production clobber bug is closed. It also survives a subsequent restart.
#[test]
#[serial_test::serial(cognitive_memory)]
fn operator_remove_via_cli_survives_daemon_cycle_and_restart() {
    let (_tmp, root) = isolated_state_root();
    seed_board(
        &root,
        vec![
            active_goal("keep-me", GoalProgress::NotStarted),
            active_goal("remove-me", GoalProgress::NotStarted),
        ],
    );

    // Daemon adopts the board; its in-flight copy holds BOTH goals.
    let bridge = launch_writer_client(&root).expect("writer bridge");
    let daemon_in_flight = crate::goal_board_store::load_or_migrate(&root, bridge.ops())
        .expect("migrate")
        .board;
    assert_eq!(daemon_in_flight.active.len(), 2);

    // Operator removes one goal via the CLI (writes a tombstone).
    dispatch_operator_cli(vec![
        "goal".to_string(),
        "remove".to_string(),
        "remove-me".to_string(),
    ])
    .expect("`goal remove` must succeed");

    // Daemon commits its now-stale in-flight board at cycle end.
    let committed = crate::goal_board_store::commit_cycle(
        &root,
        &daemon_in_flight,
        &crate::goal_curation::NoProgressTracker::new(),
        1,
        &[],
    )
    .expect("cycle commit");

    let ids: std::collections::HashSet<&str> =
        committed.active.iter().map(|g| g.id.as_str()).collect();
    assert!(ids.contains("keep-me"));
    assert!(
        !ids.contains("remove-me"),
        "operator remove must survive a full daemon cycle (anti-clobber via tombstone reconcile)"
    );

    // Survives a restart.
    let after_restart = crate::goal_board_store::load(&root);
    assert!(
        !after_restart
            .board
            .active
            .iter()
            .any(|g| g.id == "remove-me"),
        "operator remove must survive a restart"
    );
}

/// `goal complete` tombstones a goal so a later recall/curation pass that tries
/// to re-introduce it (the ladybug re-seeding failure mode) cannot resurrect it.
#[test]
#[serial_test::serial(cognitive_memory)]
fn operator_complete_tombstones_and_blocks_reseed() {
    let (_tmp, root) = isolated_state_root();
    seed_board(
        &root,
        vec![active_goal("ladybug-hardening", GoalProgress::NotStarted)],
    );

    dispatch_operator_cli(vec![
        "goal".to_string(),
        "complete".to_string(),
        "ladybug-hardening".to_string(),
    ])
    .expect("`goal complete` must succeed");

    // Tombstone recorded and the goal is off the board.
    assert!(crate::ooda_loop::load_tombstones(&root).contains("ladybug-hardening"));

    // A recall pass proposing the completed goal again is filtered out.
    let recalled = GoalBoard {
        active: vec![active_goal("ladybug-hardening", GoalProgress::NotStarted)],
        backlog: Vec::new(),
    };
    let tombstones = crate::ooda_loop::load_tombstones(&root);
    let filtered = crate::goal_board_store::filter_tombstoned(recalled, &tombstones);
    assert!(
        filtered.active.is_empty(),
        "a completed+tombstoned goal must never be re-seeded from recalled memory"
    );
}

/// `goal reprioritize <id> <p>` (the required alias of `set-priority`) changes an
/// existing goal's priority in the authoritative store, is reflected by the very
/// next read (read-your-writes), and survives a daemon restart — an operator
/// steer that must STICK and not be clobbered. An unknown id is a hard error.
#[test]
#[serial_test::serial(cognitive_memory)]
fn operator_reprioritize_via_cli_sticks_and_survives_restart() {
    let (_tmp, root) = isolated_state_root();

    // Seed a goal at priority 5 so the change to p2 is observable.
    let seeded = {
        let mut g = active_goal("steer-me", GoalProgress::NotStarted);
        g.priority = 5;
        g
    };
    seed_board(&root, vec![seeded]);

    dispatch_operator_cli(vec![
        "goal".to_string(),
        "reprioritize".to_string(),
        "steer-me".to_string(),
        "2".to_string(),
    ])
    .expect("`goal reprioritize` must succeed");

    // Reflected immediately in the authoritative store (read-your-writes).
    let after = crate::goal_board_store::load(&root).board;
    let g = after
        .active
        .iter()
        .find(|g| g.id == "steer-me")
        .expect("goal must survive reprioritize");
    assert_eq!(
        g.priority, 2,
        "reprioritize must change the persisted priority immediately"
    );

    // Survives a daemon restart (a fresh load of goal_board.json).
    let after_restart = crate::goal_board_store::load(&root).board;
    assert_eq!(
        after_restart
            .active
            .iter()
            .find(|g| g.id == "steer-me")
            .expect("goal must survive restart")
            .priority,
        2,
        "reprioritize must survive a restart"
    );

    // Reprioritizing an unknown id is a hard error (non-zero exit) and never
    // silently succeeds.
    let err = dispatch_operator_cli(vec![
        "goal".to_string(),
        "reprioritize".to_string(),
        "no-such-goal".to_string(),
        "3".to_string(),
    ]);
    assert!(
        err.is_err(),
        "reprioritize of an unknown goal id must return a non-zero exit"
    );
}

// ─── standing / perpetual goals (issue #2580) ───────────────────────────────

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_add_standing_marks_goal_perpetual() {
    let (_tmp, root) = isolated_state_root();

    let result = dispatch_operator_cli(vec![
        "goal".to_string(),
        "add".to_string(),
        "5".to_string(),
        "--standing".to_string(),
        "continuously research and improve cognition".to_string(),
    ]);
    assert!(
        result.is_ok(),
        "`goal add --standing` must exit 0; got: {:?}",
        result.err().map(|e| e.to_string())
    );

    let board = load_board(&root);
    let g = board
        .active
        .iter()
        .find(|g| g.is_perpetual())
        .expect("a standing goal must be on the board after --standing add");
    assert!(
        g.description.contains("continuously research"),
        "operator description must be preserved: {}",
        g.description
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_complete_reopens_standing_goal_without_tombstone() {
    let (_tmp, root) = isolated_state_root();
    // A standing goal that a done-claim drove to Completed.
    let mut standing = active_goal("research-loop", GoalProgress::Completed);
    standing.description = "Research cognition. STANDING PERPETUAL goal.".to_string();
    standing.assigned_to = Some("engineer-x".to_string());
    seed_board(&root, vec![standing]);

    let result = dispatch_operator_cli(vec![
        "goal".to_string(),
        "complete".to_string(),
        "research-loop".to_string(),
    ]);
    assert!(
        result.is_ok(),
        "`goal complete` on a standing goal must exit 0; got: {:?}",
        result.err().map(|e| e.to_string())
    );

    let board = load_board(&root);
    let g = board
        .active
        .iter()
        .find(|g| g.id == "research-loop")
        .expect("a standing goal must NOT be removed by `goal complete`");
    assert_eq!(
        g.status,
        GoalProgress::NotStarted,
        "standing goal must be reopened (rolled to a fresh cycle)"
    );
    assert!(
        g.assigned_to.is_none(),
        "assignment must be cleared on reopen"
    );
    // Never tombstoned: a fresh add of the same id must succeed / not be filtered.
    let tombstones = crate::ooda_loop::load_tombstones(&root);
    assert!(
        !tombstones.contains("research-loop"),
        "a standing goal must never be tombstoned by `goal complete`"
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_complete_still_removes_and_tombstones_normal_goal() {
    // Regression guard: a normal goal completes as before.
    let (_tmp, root) = isolated_state_root();
    seed_board(
        &root,
        vec![active_goal("one-shot", GoalProgress::Completed)],
    );

    let result = dispatch_operator_cli(vec![
        "goal".to_string(),
        "complete".to_string(),
        "one-shot".to_string(),
    ]);
    assert!(
        result.is_ok(),
        "`goal complete` on a normal goal must exit 0"
    );

    let board = load_board(&root);
    assert!(
        board.active.iter().all(|g| g.id != "one-shot"),
        "a normal goal must be removed by `goal complete`"
    );
    let tombstones = crate::ooda_loop::load_tombstones(&root);
    assert!(
        tombstones.contains("one-shot"),
        "a normal completed goal must be tombstoned"
    );
}

// ─── #2743 — `simard goal label <id> add|remove|list` round-trips ───────────

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_label_add_remove_round_trips_on_persisted_board() {
    let (_tmp, root) = isolated_state_root();
    seed_board(&root, vec![active_goal("tag-me", GoalProgress::NotStarted)]);

    // add lands and persists.
    let r = dispatch_operator_cli(vec![
        "goal".to_string(),
        "label".to_string(),
        "tag-me".to_string(),
        "add".to_string(),
        "area:dashboard".to_string(),
    ]);
    assert!(
        r.is_ok(),
        "label add must exit 0: {:?}",
        r.err().map(|e| e.to_string())
    );
    let board = load_board(&root);
    let goal = board.active.iter().find(|g| g.id == "tag-me").unwrap();
    assert_eq!(goal.labels, vec!["area:dashboard"], "tag persisted");

    // add is idempotent (still exit 0, no duplicate).
    let r = dispatch_operator_cli(vec![
        "goal".to_string(),
        "label".to_string(),
        "tag-me".to_string(),
        "add".to_string(),
        "area:dashboard".to_string(),
    ]);
    assert!(r.is_ok(), "idempotent re-add must exit 0");
    let board = load_board(&root);
    assert_eq!(
        board
            .active
            .iter()
            .find(|g| g.id == "tag-me")
            .unwrap()
            .labels,
        vec!["area:dashboard"],
        "re-adding an existing tag does not duplicate it",
    );

    // remove lands.
    let r = dispatch_operator_cli(vec![
        "goal".to_string(),
        "label".to_string(),
        "tag-me".to_string(),
        "remove".to_string(),
        "area:dashboard".to_string(),
    ]);
    assert!(r.is_ok(), "label remove must exit 0");
    let board = load_board(&root);
    assert!(
        board
            .active
            .iter()
            .find(|g| g.id == "tag-me")
            .unwrap()
            .labels
            .is_empty(),
        "tag removed",
    );

    // remove of an absent tag is a no-op that still exits 0.
    let r = dispatch_operator_cli(vec![
        "goal".to_string(),
        "label".to_string(),
        "tag-me".to_string(),
        "remove".to_string(),
        "area:dashboard".to_string(),
    ]);
    assert!(r.is_ok(), "removing an absent tag is a no-op that exits 0");
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_label_add_rejects_empty_tag_and_unknown_goal() {
    let (_tmp, root) = isolated_state_root();
    seed_board(
        &root,
        vec![active_goal("present", GoalProgress::NotStarted)],
    );

    // Whitespace-only tag is rejected (non-zero exit).
    let r = dispatch_operator_cli(vec![
        "goal".to_string(),
        "label".to_string(),
        "present".to_string(),
        "add".to_string(),
        "   ".to_string(),
    ]);
    assert!(r.is_err(), "an empty-after-trim tag must be rejected");

    // Unknown goal id is a non-zero exit.
    let r = dispatch_operator_cli(vec![
        "goal".to_string(),
        "label".to_string(),
        "ghost".to_string(),
        "add".to_string(),
        "area:x".to_string(),
    ]);
    assert!(r.is_err(), "labelling an unknown goal must exit non-zero");
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn simard_goal_list_with_tag_filter_exits_zero() {
    let (_tmp, root) = isolated_state_root();
    let mut g = active_goal("filtered", GoalProgress::NotStarted);
    g.labels = vec!["source:creative-ideas".to_string()];
    seed_board(
        &root,
        vec![g, active_goal("other", GoalProgress::NotStarted)],
    );

    let r = dispatch_operator_cli(vec![
        "goal".to_string(),
        "list".to_string(),
        "--tag".to_string(),
        "source:creative-ideas".to_string(),
    ]);
    assert!(
        r.is_ok(),
        "goal list --tag must exit 0: {:?}",
        r.err().map(|e| e.to_string())
    );
}
