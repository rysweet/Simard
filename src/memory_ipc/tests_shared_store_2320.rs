//! Regression tests for the read-after-write durability race that made
//! `operator_commands_dashboard::tests_goals_crud::full_goal_lifecycle_crud`
//! flaky after the cognitive-memory de-fork (issues
//! [#2320](https://github.com/rysweet/Simard/issues/2320) /
//! [#2316](https://github.com/rysweet/Simard/issues/2316)).
//!
//! Root cause: every dashboard goal write/read opened a *fresh*
//! `LibraryCognitiveMemory` via the tier-2 launcher ladder, so a sequence of
//! open→write→drop→reopen→read cycles against one `state_root` reopened the
//! lbug `Database` repeatedly. That reopen intermittently returned fact rows
//! whose per-fact `_simard_seq` ordering metadata had not been folded back in,
//! collapsing the "max node_id == newest snapshot" invariant the goal-board
//! read depends on — surfacing as an empty / stale board.
//!
//! Fix: [`launch_writer_bridge`] and [`open_reader_bridge`] now share one
//! cached store handle per canonical `state_root`, so a write is immediately
//! visible to the next read with no reopen. These tests drive the exact
//! open→write→reopen→read pattern in a tight loop; before the fix they failed
//! within a handful of iterations.

use std::path::Path;

use super::{launch_writer_bridge, open_reader_bridge};
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, load_goal_board, save_goal_board};

/// Three seeded goals with non-placeholder descriptions (so the board-integrity
/// guard in `save_goal_board` accepts them).
fn seeded_board() -> GoalBoard {
    let descs = [
        "Continuously improve own capabilities through gym scenarios and self-evaluation",
        "Expand knowledge base through meetings, research, and cognitive memory consolidation",
        "Maintain system health: budget compliance and resource usage within thresholds",
    ];
    let mut board = GoalBoard::new();
    for (i, d) in descs.iter().enumerate() {
        board.active.push(ActiveGoal {
            id: format!("seed-goal-{i}"),
            description: (*d).to_string(),
            priority: (i + 1) as u32,
            status: GoalProgress::InProgress { percent: 0 },
            assigned_to: Some("simard".to_string()),
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        });
    }
    board
}

/// Read the goal board through a freshly-opened reader bridge — the exact path
/// `dashboard_goal_board_snapshot` takes.
fn read_board(root: &Path) -> GoalBoard {
    let reader = open_reader_bridge(root).expect("reader bridge");
    load_goal_board(reader.ops()).expect("load_goal_board")
}

/// Write the goal board through a freshly-opened writer bridge — the exact path
/// `dashboard_save_goal_board` takes.
fn write_board(root: &Path, board: &GoalBoard) {
    let writer = launch_writer_bridge(root).expect("writer bridge");
    save_goal_board(board, writer.ops()).expect("save_goal_board");
}

/// A write through one tier-2 bridge must be visible to the *next* freshly
/// opened reader bridge on the same `state_root`, every iteration — no flaky
/// empty/stale reads (#2320). Before the shared-store fix this failed within a
/// few iterations with an empty board.
#[test]
#[serial_test::serial(cognitive_memory)]
fn read_after_write_is_stable_across_reopen_cycles() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();

    // Initialise with an empty board, then seed three active goals — mirroring
    // `init_empty_board` + `seed_goals`.
    write_board(root, &GoalBoard::new());
    write_board(root, &seeded_board());

    // Many fresh-bridge read/rewrite cycles. The board must keep exactly the
    // three seeded goals every time. Each iteration opens a new writer and a new
    // reader (the reopen pattern that used to race).
    for i in 0..40 {
        let board = read_board(root);
        assert_eq!(
            board.active.len(),
            3,
            "iteration {i}: read-after-write must return the 3 seeded goals, \
             not an empty/stale board (#2320)",
        );
        // Re-persist the board we just read, exactly like a no-op dashboard
        // mutation would, to keep the open→write→reopen→read churn going.
        write_board(root, &board);
    }
}

/// Two tier-2 bridges opened for the same `state_root` must observe each
/// other's writes immediately, because they share one cached store handle.
#[test]
#[serial_test::serial(cognitive_memory)]
fn tier2_writer_and_reader_share_one_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();

    write_board(root, &GoalBoard::new());
    write_board(root, &seeded_board());

    // Open a reader and hold it, then perform a *separate* writer-bridge
    // mutation, then read again through the still-open reader. The mutation
    // must be visible without reopening the reader — proving the shared handle.
    let reader = open_reader_bridge(root).expect("reader bridge");
    assert_eq!(load_goal_board(reader.ops()).expect("load").active.len(), 3);

    // Add a fourth active goal through a fresh writer bridge.
    let mut board = seeded_board();
    board.active.push(ActiveGoal {
        id: "fourth-goal".to_string(),
        description: "Establish hive-mind sync with remote Simard instances".to_string(),
        priority: 4,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    });
    write_board(root, &board);

    // The already-open reader sees the new goal through the shared store.
    let after = load_goal_board(reader.ops()).expect("load after write");
    assert_eq!(
        after.active.len(),
        4,
        "a write through a sibling tier-2 bridge must be immediately visible to \
         an already-open reader bridge on the same state_root (shared store, #2320)",
    );
}
