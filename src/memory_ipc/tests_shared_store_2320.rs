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

use super::{clear_tier2_store_cache, launch_writer_bridge, open_reader_bridge};
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
            repo: None,
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
        repo: None,
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

/// Tier-2 bridges wrap the shared store in `SharedMemory`. That wrapper must
/// forward *every* `CognitiveMemoryOps` method to the inner library handle —
/// in particular the episodic-recall / distillation methods whose trait
/// defaults are empty no-ops. If a method is not forwarded, a tier-2 caller
/// silently reads empty episodes even though the store holds them.
#[test]
#[serial_test::serial(cognitive_memory)]
fn tier2_bridge_forwards_episodic_recall_through_sharedmemory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();

    // Store an episode through a writer bridge.
    let writer = launch_writer_bridge(root).expect("writer bridge");
    let node_id = writer
        .ops()
        .store_episode("engineer fixed the durable-recall race", "test", None)
        .expect("store_episode through tier-2 writer");
    assert!(!node_id.is_empty(), "store_episode must return a node id");

    // A fresh reader bridge must recall it via the keyword search and list it as
    // undistilled — both of which return `vec![]` if `SharedMemory` falls back
    // to the trait defaults instead of forwarding to the library backend.
    let reader = open_reader_bridge(root).expect("reader bridge");
    let by_kw = reader
        .ops()
        .search_episodes_by_keywords(&["durable-recall".to_string()], 10)
        .expect("search_episodes_by_keywords");
    assert!(
        by_kw.iter().any(|e| e.node_id == node_id),
        "SharedMemory must forward search_episodes_by_keywords to the library \
         backend; got {} episodes (a no-op default returns none)",
        by_kw.len(),
    );

    let undistilled = reader
        .ops()
        .list_undistilled_episodes(10)
        .expect("list_undistilled_episodes");
    assert!(
        undistilled.iter().any(|e| e.node_id == node_id),
        "SharedMemory must forward list_undistilled_episodes to the library backend",
    );

    // mark_episode_distilled must also reach the backend: after marking, the
    // episode drops out of the undistilled list.
    reader
        .ops()
        .mark_episode_distilled(&node_id)
        .expect("mark_episode_distilled");
    let after = open_reader_bridge(root)
        .expect("reader bridge")
        .ops()
        .list_undistilled_episodes(10)
        .expect("list_undistilled_episodes after marking");
    assert!(
        !after.iter().any(|e| e.node_id == node_id),
        "mark_episode_distilled must be forwarded; the episode should no longer be undistilled",
    );
}

/// Issue #2331: `SharedMemory` must also forward `graph_stats`. Its trait
/// default returns an all-zero [`GraphStats`], so a tier-0/2 reader that wraps
/// the live library handle would otherwise report zero edges even when the
/// store holds real provenance — making `simard memory stats` blind to the
/// connections whenever a reader resolves through the shared in-process store.
#[test]
#[serial_test::serial(cognitive_memory)]
fn tier2_bridge_forwards_graph_stats_through_sharedmemory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();

    // Seed a DERIVES_FROM edge: a fact distilled from an episode.
    let writer = launch_writer_bridge(root).expect("writer bridge");
    let episode = writer
        .ops()
        .store_episode("ran cargo test; 0 failures", "test", None)
        .expect("store_episode through tier-2 writer");
    writer
        .ops()
        .store_fact_with_provenance(
            "lesson",
            "tests must stay green",
            0.9,
            "distill:cycle",
            None,
            None,
            std::slice::from_ref(&episode),
        )
        .expect("store_fact_with_provenance through tier-2 writer");

    // A reader resolving through the shared store wraps it in `SharedMemory`;
    // `graph_stats` must reach the library backend, not the zeroed default.
    let reader = open_reader_bridge(root).expect("reader bridge");
    let stats = reader
        .ops()
        .graph_stats()
        .expect("graph_stats via SharedMemory");
    assert!(
        stats.derives_from_edges >= 1,
        "SharedMemory must forward graph_stats to the library backend; got \
         derives_from_edges={} (a zeroed default means no forwarding)",
        stats.derives_from_edges,
    );
    assert!(
        stats.facts_with_provenance >= 1,
        "forwarded graph_stats must reflect the seeded provenance coverage; got \
         facts_with_provenance={}",
        stats.facts_with_provenance,
    );
}

/// After [`clear_tier2_store_cache`] drops the shared handle (checkpointing via
/// `Database::drop`), a fresh tier-2 open must cold-recall the persisted board
/// from disk. Guards the lifetime argument: evicting a cached handle does not
/// lose data, and the next open rebuilds it correctly.
#[test]
#[serial_test::serial(cognitive_memory)]
fn cold_reopen_after_cache_clear_recalls_persisted_board() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();

    write_board(root, &GoalBoard::new());
    write_board(root, &seeded_board());

    // Drop every cached handle — forces the next open to read from disk.
    clear_tier2_store_cache();

    let board = read_board(root);
    assert_eq!(
        board.active.len(),
        3,
        "after clearing the tier-2 cache, a cold reopen must recall the 3 \
         persisted goals from disk",
    );
}
