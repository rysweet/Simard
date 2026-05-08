//! Failing TDD tests (issue #1590, Step 7) for the engineer loop's migration
//! off `goal_records.json`.
//!
//! Spec section 10: `src/engineer_loop/mod.rs:276` currently calls
//! `FileBackedGoalStore::try_new(state_root.join("goal_records.json"))?`
//! `.active_top_goals(5)?`. That must become:
//!
//! ```ignore
//! let bridge = launch_writer_bridge(state_root)?;
//! let board = load_goal_board(bridge.ops())?;
//! let records = active_goals_as_records(&board);
//! records.into_iter().take(5).collect()
//! ```
//!
//! These tests prove the equivalent pipeline through the public adapter is
//! sound. They fail to compile until `active_goals_as_records` lands.

use std::path::PathBuf;

use crate::cognitive_memory::NativeCognitiveMemory;
use crate::goal_curation::{
    ActiveGoal, GoalBoard, GoalProgress, active_goals_as_records, load_goal_board, save_goal_board,
};

fn fresh_state_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "simard-engineer-mig-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn seed_active_only(state_root: &std::path::Path, n: usize) -> GoalBoard {
    let mem = NativeCognitiveMemory::open(state_root).expect("open native memory");
    let mut board = GoalBoard::new();
    for i in 0..n {
        board.active.push(ActiveGoal {
            id: format!("engineer-mig-active-goal-{i:02}"),
            description: format!("Engineer migration active goal #{i:02}"),
            priority: (i + 1) as u32,
            status: GoalProgress::NotStarted,
            assigned_to: Some("simard".to_string()),
            current_activity: None,
            wip_refs: vec![],
        });
    }
    save_goal_board(&board, &mem).expect("seed active goals");
    board
}

#[test]
fn top_5_active_records_come_from_cognitive_memory_in_seeded_order() {
    let root = fresh_state_root("top5");
    let seeded = seed_active_only(&root, 7);
    assert_eq!(seeded.active.len(), 7);

    let mem = NativeCognitiveMemory::open(&root).expect("reopen native memory");
    let loaded = load_goal_board(&mem).expect("load_goal_board");
    let top: Vec<_> = active_goals_as_records(&loaded)
        .into_iter()
        .take(5)
        .collect();

    assert_eq!(top.len(), 5, "engineer dispatch must take exactly 5");
    for (i, r) in top.iter().enumerate() {
        assert_eq!(
            r.slug,
            format!("engineer-mig-active-goal-{i:02}"),
            "top-5 must preserve insertion order from the cognitive-memory snapshot"
        );
    }
    assert!(
        !root.join("goal_records.json").exists(),
        "engineer pipeline through cognitive memory must not produce a legacy file"
    );
}

#[test]
fn engineer_pipeline_returns_empty_top_5_when_no_snapshot() {
    // load_goal_board on an uninitialised memory returns an empty board;
    // active_goals_as_records returns an empty Vec; .take(5) returns 0.
    // The engineer loop must therefore tolerate "no goals" gracefully
    // (currently `FileBackedGoalStore::active_top_goals(5)` does the same
    // by returning an empty Vec when the legacy file is absent).
    let root = fresh_state_root("empty-top5");
    {
        let _mem = NativeCognitiveMemory::open(&root).expect("open native memory");
    }

    let mem = NativeCognitiveMemory::open(&root).expect("reopen native memory");
    let loaded = load_goal_board(&mem).expect("load_goal_board");
    let top: Vec<_> = active_goals_as_records(&loaded)
        .into_iter()
        .take(5)
        .collect();
    assert!(top.is_empty(), "no snapshot → no top-5 goals");
}

#[test]
fn engineer_pipeline_caps_at_five_even_when_more_goals_exist() {
    // Goal board enforces MAX_ACTIVE_GOALS = 5, so seeding 5 + take(5)
    // must return exactly 5 records and never panic.
    let root = fresh_state_root("cap-at-5");
    seed_active_only(&root, 5);

    let mem = NativeCognitiveMemory::open(&root).expect("reopen native memory");
    let loaded = load_goal_board(&mem).expect("load_goal_board");
    let top: Vec<_> = active_goals_as_records(&loaded)
        .into_iter()
        .take(5)
        .collect();
    assert_eq!(top.len(), 5);
}
