//! TDD test: repeated goal-board snapshot saves SUPERSEDE rather than
//! accumulate (issue #2329, SimPR4).
//!
//! `save_goal_board` routes the `goal-board:snapshot` write through the
//! library's CallerKey dedup. Saving the board repeatedly (each save changing
//! it) must leave exactly **one live** snapshot fact — the prior revisions are
//! superseded, not piled up — while the latest content remains readable.

use serial_test::serial;
use tempfile::TempDir;

use crate::cognitive_memory::{CognitiveMemoryOps, RecallWeightSet};
use crate::goal_curation::{
    ActiveGoal, GoalBoard, GoalProgress, add_active_goal, load_goal_board, save_goal_board,
};
use crate::memory_ipc::launch_writer_bridge;
use crate::state_root::STATE_ROOT_ENV;

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
        parent_goal_id: None,
        repo: None,
        id: id.to_string(),
        description: format!("{id} description"),
        priority,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    }
}

/// Count the LIVE `goal-board:snapshot` facts via ranked recall (which excludes
/// superseded/archived revisions).
fn live_snapshot_count(bridge: &dyn CognitiveMemoryOps) -> usize {
    bridge
        .recall_facts_ranked("goal-board:snapshot", 256, 0.0, RecallWeightSet::default())
        .expect("ranked recall")
        .into_iter()
        .filter(|f| f.concept == "goal-board:snapshot")
        .count()
}

#[test]
#[serial(cognitive_memory)]
fn repeated_snapshot_saves_supersede_not_duplicate() {
    let (_tmp, root) = isolated_state_root();
    let bridge = launch_writer_bridge(&root).expect("writer bridge");

    // Save #1: one live snapshot.
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("goal-alpha", 1)).unwrap();
    save_goal_board(&board, bridge.ops()).expect("save v1");
    assert_eq!(
        live_snapshot_count(bridge.ops()),
        1,
        "one live snapshot after first save"
    );

    // Save #2 (changed): supersedes v1 — still one live snapshot.
    add_active_goal(&mut board, active_goal("goal-beta", 2)).unwrap();
    save_goal_board(&board, bridge.ops()).expect("save v2");
    assert_eq!(live_snapshot_count(bridge.ops()), 1, "v2 supersedes v1");

    // Save #3 (changed): supersedes v2 — still one live snapshot.
    add_active_goal(&mut board, active_goal("goal-gamma", 3)).unwrap();
    save_goal_board(&board, bridge.ops()).expect("save v3");
    assert_eq!(
        live_snapshot_count(bridge.ops()),
        1,
        "repeated snapshots must supersede, never accumulate live duplicates"
    );

    // The latest snapshot content is readable and reflects all three goals.
    let loaded = load_goal_board(bridge.ops()).expect("load board");
    let ids: std::collections::HashSet<&str> =
        loaded.active.iter().map(|g| g.id.as_str()).collect();
    assert!(ids.contains("goal-alpha"));
    assert!(ids.contains("goal-beta"));
    assert!(ids.contains("goal-gamma"));
}
