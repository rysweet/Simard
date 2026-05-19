//! Failing TDD tests (issues
//! [#1923](https://github.com/rysweet/Simard/issues/1923) /
//! [#1925](https://github.com/rysweet/Simard/issues/1925)) for
//! [`super::save_goal_board_with_removals`].
//!
//! Contract under test (see
//! `docs/reference/goal-board-api.md#save_goal_board_with_removals`):
//!
//! - Empty `force_remove_ids` slice → exactly equivalent to
//!   `save_goal_board(board, bridge)`.
//! - Ids present on the merged board (active or backlog) are removed.
//! - Ids absent from the merged board are silent no-ops (no error) —
//!   preserves the idempotency the CLI surface advertises.
//! - Duplicate ids in the slice are treated as one removal.
//! - **PR #1926 regression** — an id that is on the persisted snapshot
//!   but NOT on the in-flight board is still removed, because the
//!   filter runs after the merge. Plain `save_goal_board` would
//!   "resurrect" it from the persisted side; the removal variant must
//!   defeat that resurrection.
//! - Unrelated persisted goals are preserved (defends the merge-on-write
//!   guarantee from [#1915](https://github.com/rysweet/Simard/issues/1915)).
//!
//! Tests fail until the implementation step replaces the
//! `unimplemented!` body in `src/goal_curation/operations.rs`.

use serial_test::serial;
use tempfile::TempDir;

use crate::goal_curation::{
    ActiveGoal, BacklogItem, GoalBoard, GoalProgress, add_active_goal, add_backlog_item,
    load_goal_board, save_goal_board, save_goal_board_with_removals,
};
use crate::memory_ipc::launch_writer_bridge;
use crate::state_root::STATE_ROOT_ENV;

/// Allocate a TempDir state root and set `SIMARD_STATE_ROOT` for the
/// duration of the test. `TempDir` returned so the caller can keep it
/// alive — once it drops, the cognitive-memory DB is reaped.
fn isolated_state_root() -> (TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    unsafe {
        std::env::set_var(STATE_ROOT_ENV, &root);
    }
    (tmp, root)
}

fn active_goal(id: &str, priority: u32) -> ActiveGoal {
    ActiveGoal {
        id: id.to_string(),
        description: format!("{id} description"),
        priority,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    }
}

fn backlog_item(id: &str, score: f64) -> BacklogItem {
    BacklogItem {
        id: id.to_string(),
        description: format!("{id} backlog description"),
        score,
        source: "tdd-1923".to_string(),
    }
}

/// Persist `board` through `save_goal_board` and re-read via a freshly
/// opened bridge so each step exercises the same persistence layer as
/// production callers.
fn persist(board: &GoalBoard, root: &std::path::Path) {
    let bridge = launch_writer_bridge(root).expect("writer bridge");
    save_goal_board(board, bridge.ops()).expect("save_goal_board");
}

fn reload(root: &std::path::Path) -> GoalBoard {
    let bridge = launch_writer_bridge(root).expect("reader bridge");
    load_goal_board(bridge.ops()).expect("load_goal_board")
}

// ─── empty-removals equivalence ────────────────────────────────────────────

#[test]
#[serial(cognitive_memory)]
fn empty_removals_is_equivalent_to_save_goal_board() {
    let (_tmp, root) = isolated_state_root();

    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("alpha", 1)).unwrap();
    add_active_goal(&mut board, active_goal("beta", 2)).unwrap();
    add_backlog_item(&mut board, backlog_item("zeta", 0.7)).unwrap();

    let bridge = launch_writer_bridge(&root).expect("writer bridge");
    save_goal_board_with_removals(&board, &[], bridge.ops())
        .expect("save_goal_board_with_removals(&[]) must succeed");

    let reloaded = reload(&root);
    assert_eq!(reloaded.active.len(), 2);
    assert_eq!(reloaded.backlog.len(), 1);
    assert!(reloaded.active.iter().any(|g| g.id == "alpha"));
    assert!(reloaded.active.iter().any(|g| g.id == "beta"));
    assert!(reloaded.backlog.iter().any(|b| b.id == "zeta"));
}

// ─── id-on-merged-board removal ────────────────────────────────────────────

#[test]
#[serial(cognitive_memory)]
fn removes_active_goal_present_on_in_flight_board() {
    let (_tmp, root) = isolated_state_root();

    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("keeper", 1)).unwrap();
    add_active_goal(&mut board, active_goal("doomed", 2)).unwrap();

    let bridge = launch_writer_bridge(&root).expect("writer bridge");
    save_goal_board_with_removals(&board, &["doomed".to_string()], bridge.ops())
        .expect("save_goal_board_with_removals");

    let reloaded = reload(&root);
    assert!(
        reloaded.active.iter().any(|g| g.id == "keeper"),
        "unrelated goal must survive removal"
    );
    assert!(
        !reloaded.active.iter().any(|g| g.id == "doomed"),
        "removed goal id 'doomed' must not survive: {:?}",
        reloaded.active.iter().map(|g| &g.id).collect::<Vec<_>>(),
    );
}

#[test]
#[serial(cognitive_memory)]
fn removes_backlog_item_in_addition_to_active() {
    let (_tmp, root) = isolated_state_root();

    let mut board = GoalBoard::new();
    add_backlog_item(&mut board, backlog_item("backlog-doomed", 0.5)).unwrap();
    add_backlog_item(&mut board, backlog_item("backlog-keeper", 0.6)).unwrap();
    add_active_goal(&mut board, active_goal("active-doomed", 1)).unwrap();
    add_active_goal(&mut board, active_goal("active-keeper", 2)).unwrap();

    let removals = vec!["active-doomed".to_string(), "backlog-doomed".to_string()];
    let bridge = launch_writer_bridge(&root).expect("writer bridge");
    save_goal_board_with_removals(&board, &removals, bridge.ops())
        .expect("save_goal_board_with_removals");

    let reloaded = reload(&root);
    assert!(reloaded.active.iter().any(|g| g.id == "active-keeper"));
    assert!(reloaded.backlog.iter().any(|b| b.id == "backlog-keeper"));
    assert!(
        !reloaded.active.iter().any(|g| g.id == "active-doomed"),
        "active 'active-doomed' must be removed"
    );
    assert!(
        !reloaded.backlog.iter().any(|b| b.id == "backlog-doomed"),
        "backlog 'backlog-doomed' must be removed"
    );
}

// ─── PR #1926 regression: post-merge filter ───────────────────────────────

#[test]
#[serial(cognitive_memory)]
fn removal_defeats_pr_1926_resurrection_from_persisted_snapshot() {
    // This is the core #1923/#1925 property and the reason
    // `save_goal_board_with_removals` exists.
    //
    // Setup: persist board {keeper, doomed} via save_goal_board. Then
    // construct an in-flight board that contains ONLY {keeper} (i.e.
    // the caller "deleted" doomed by omitting it) and persist it via
    // the new variant with force_remove_ids=["doomed"].
    //
    // Under plain save_goal_board, the merge would union the persisted
    // 'doomed' back into the in-flight {keeper}-only board, silently
    // resurrecting it — exactly the PR #1926 failure mode. The new
    // variant's post-merge filter must drop it.
    let (_tmp, root) = isolated_state_root();

    let mut seed = GoalBoard::new();
    add_active_goal(&mut seed, active_goal("keeper", 1)).unwrap();
    add_active_goal(&mut seed, active_goal("doomed", 2)).unwrap();
    persist(&seed, &root);

    // Sanity check: plain save_goal_board would resurrect 'doomed'.
    let mut in_flight = GoalBoard::new();
    add_active_goal(&mut in_flight, active_goal("keeper", 1)).unwrap();
    {
        let bridge = launch_writer_bridge(&root).expect("writer bridge");
        // Plain save: this is what PR #1926 tried and lost to merge.
        save_goal_board(&in_flight, bridge.ops()).expect("plain save");
    }
    let after_plain = reload(&root);
    assert!(
        after_plain.active.iter().any(|g| g.id == "doomed"),
        "sanity: plain save_goal_board MUST resurrect 'doomed' from \
         the persisted snapshot (this is the PR #1926 failure mode); \
         the regression test is meaningless otherwise. Got: {:?}",
        after_plain.active.iter().map(|g| &g.id).collect::<Vec<_>>(),
    );

    // Now the new variant: ask explicitly for removal.
    let bridge = launch_writer_bridge(&root).expect("writer bridge");
    save_goal_board_with_removals(&in_flight, &["doomed".to_string()], bridge.ops())
        .expect("save_goal_board_with_removals");

    let after_removal = reload(&root);
    assert!(
        after_removal.active.iter().any(|g| g.id == "keeper"),
        "unrelated 'keeper' must survive removal"
    );
    assert!(
        !after_removal.active.iter().any(|g| g.id == "doomed"),
        "force_remove_ids must defeat the merge-on-write resurrection \
         of 'doomed' from the persisted snapshot; got: {:?}",
        after_removal
            .active
            .iter()
            .map(|g| &g.id)
            .collect::<Vec<_>>(),
    );
}

// ─── idempotency: unknown ids silently skip ───────────────────────────────

#[test]
#[serial(cognitive_memory)]
fn unknown_removal_ids_are_silent_no_ops() {
    let (_tmp, root) = isolated_state_root();

    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("alpha", 1)).unwrap();
    add_active_goal(&mut board, active_goal("beta", 2)).unwrap();

    let removals = vec!["never-existed".to_string(), "alpha".to_string()];
    let bridge = launch_writer_bridge(&root).expect("writer bridge");
    let result = save_goal_board_with_removals(&board, &removals, bridge.ops());
    assert!(
        result.is_ok(),
        "unknown ids in force_remove_ids must be silent no-ops, not errors; \
         got: {:?}",
        result.err().map(|e| e.to_string()),
    );

    let reloaded = reload(&root);
    assert!(
        !reloaded.active.iter().any(|g| g.id == "alpha"),
        "the known id 'alpha' must still be removed even when an unknown id \
         shares the slice"
    );
    assert!(reloaded.active.iter().any(|g| g.id == "beta"));
}

#[test]
#[serial(cognitive_memory)]
fn empty_board_with_removals_is_idempotent() {
    let (_tmp, root) = isolated_state_root();

    let board = GoalBoard::new();
    let removals = vec!["does-not-exist".to_string()];

    let bridge = launch_writer_bridge(&root).expect("writer bridge");
    let r1 = save_goal_board_with_removals(&board, &removals, bridge.ops());
    let r2 = save_goal_board_with_removals(&board, &removals, bridge.ops());
    assert!(r1.is_ok() && r2.is_ok(), "both calls must succeed");
    let reloaded = reload(&root);
    assert!(reloaded.active.is_empty());
    assert!(reloaded.backlog.is_empty());
}

// ─── deduplication of repeated ids ─────────────────────────────────────────

#[test]
#[serial(cognitive_memory)]
fn duplicate_ids_in_slice_are_treated_as_one_removal() {
    let (_tmp, root) = isolated_state_root();

    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("dup", 1)).unwrap();
    add_active_goal(&mut board, active_goal("other", 2)).unwrap();

    let removals = vec!["dup".to_string(), "dup".to_string(), "dup".to_string()];
    let bridge = launch_writer_bridge(&root).expect("writer bridge");
    let result = save_goal_board_with_removals(&board, &removals, bridge.ops());
    assert!(
        result.is_ok(),
        "repeated ids must not produce a duplicate-key error; got: {:?}",
        result.err().map(|e| e.to_string()),
    );

    let reloaded = reload(&root);
    assert!(!reloaded.active.iter().any(|g| g.id == "dup"));
    assert!(reloaded.active.iter().any(|g| g.id == "other"));
}

// ─── concurrent unrelated writers preserved ───────────────────────────────

#[test]
#[serial(cognitive_memory)]
fn concurrent_unrelated_persisted_goal_survives_removal_of_other_id() {
    // Simulates: writer X persists {alpha}. Operator runs
    // save_goal_board_with_removals(&{}, &["doomed"]). Result must
    // retain alpha — only 'doomed' is removed, and 'doomed' wasn't on
    // either side.
    let (_tmp, root) = isolated_state_root();

    let mut writer_x = GoalBoard::new();
    add_active_goal(&mut writer_x, active_goal("alpha", 1)).unwrap();
    persist(&writer_x, &root);

    let operator_in_flight = GoalBoard::new();
    let bridge = launch_writer_bridge(&root).expect("writer bridge");
    save_goal_board_with_removals(&operator_in_flight, &["doomed".to_string()], bridge.ops())
        .expect("save_goal_board_with_removals");

    let reloaded = reload(&root);
    assert!(
        reloaded.active.iter().any(|g| g.id == "alpha"),
        "writer X's 'alpha' must survive an unrelated removal — the merge \
         must run before the filter so unrelated writers are not dropped"
    );
}
