//! Failing TDD tests (issue #1590, Step 7) for the dashboard's migration off
//! `goal_records.json`.
//!
//! Spec sections 3-7: every dashboard handler that today reads or writes
//! `<state_root>/goal_records.json` must instead flow through cognitive
//! memory via `load_goal_board(open_reader_bridge(...).ops())` for reads
//! and `save_goal_board(&board, launch_writer_bridge(...).ops())` for
//! writes.
//!
//! To test this without standing up the full Axum stack, the migration
//! must expose two thin in-crate helpers that the route handlers call
//! internally:
//!
//! ```ignore
//! pub(crate) fn dashboard_goal_board_snapshot(state_root: &Path) -> SimardResult<GoalBoard>;
//! pub(crate) fn dashboard_save_goal_board(state_root: &Path, board: &GoalBoard) -> SimardResult<()>;
//! ```
//!
//! Tests below reference both helpers — they will not compile until the
//! migration adds them, which is the intended TDD red state.

use std::path::PathBuf;

use crate::cognitive_memory::NativeCognitiveMemory;
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, load_goal_board, save_goal_board};

use super::{dashboard_goal_board_snapshot, dashboard_save_goal_board};

fn fresh_state_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "simard-dashboard-mig-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn seeded_board() -> GoalBoard {
    let mut board = GoalBoard::new();
    for i in 0..5 {
        board.active.push(ActiveGoal {
            id: format!("dashboard-mig-active-goal-{i:02}"),
            description: format!("Dashboard migration active goal #{i:02}"),
            priority: (i + 1) as u32,
            status: GoalProgress::NotStarted,
            assigned_to: Some("simard".to_string()),
            current_activity: None,
            wip_refs: vec![],
        });
    }
    board
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn reader_returns_snapshot_from_cognitive_memory_without_legacy_file() {
    let root = fresh_state_root("reader");
    let board = seeded_board();
    {
        let mem = NativeCognitiveMemory::open(&root).expect("open native memory");
        save_goal_board(&board, &mem).expect("seed snapshot");
    }
    assert!(
        !root.join("goal_records.json").exists(),
        "precondition: legacy file must not exist"
    );

    let loaded = dashboard_goal_board_snapshot(&root)
        .expect("dashboard reader must succeed against cognitive-memory-only state");

    assert_eq!(
        loaded.active.len(),
        board.active.len(),
        "dashboard reader must return all 5 seeded active goals"
    );
    let returned_ids: Vec<&str> = loaded.active.iter().map(|g| g.id.as_str()).collect();
    for expected in &board.active {
        assert!(
            returned_ids.contains(&expected.id.as_str()),
            "dashboard reader missing seeded goal {}; got {:?}",
            expected.id,
            returned_ids
        );
    }

    assert!(
        !root.join("goal_records.json").exists(),
        "dashboard reader must not (re)create the legacy file"
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn writer_persists_through_cognitive_memory_without_legacy_file() {
    let root = fresh_state_root("writer");
    {
        let mem = NativeCognitiveMemory::open(&root).expect("open native memory");
        save_goal_board(&GoalBoard::new(), &mem).expect("seed empty board");
    }

    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        id: "dashboard-writer-mig-target".to_string(),
        description: "Persisted via dashboard writer helper, not std::fs::write".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: Some("simard".to_string()),
        current_activity: None,
        wip_refs: vec![],
    });

    dashboard_save_goal_board(&root, &board).expect("dashboard writer must succeed");

    let mem = NativeCognitiveMemory::open(&root).expect("reopen native memory");
    let loaded = load_goal_board(&mem).expect("load_goal_board after dashboard write");
    assert!(
        loaded
            .active
            .iter()
            .any(|g| g.id == "dashboard-writer-mig-target"),
        "dashboard writer must persist via cognitive memory; got {:?}",
        loaded.active
    );

    assert!(
        !root.join("goal_records.json").exists(),
        "dashboard writer must not produce {}",
        root.join("goal_records.json").display()
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn reader_returns_empty_board_when_snapshot_missing() {
    // Resilience contract: load_goal_board returns an empty board when no
    // snapshot exists (operations.rs:188-214). The dashboard helper must
    // inherit that behaviour so a brand-new install renders "0 goals"
    // instead of returning a 500.
    let root = fresh_state_root("reader-empty");
    {
        let _mem = NativeCognitiveMemory::open(&root).expect("open native memory");
    }

    let loaded = dashboard_goal_board_snapshot(&root)
        .expect("dashboard reader must succeed (empty board) when no snapshot exists");
    assert!(
        loaded.active.is_empty(),
        "no snapshot → empty active list; got {} goals",
        loaded.active.len()
    );
    assert!(loaded.backlog.is_empty());
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn writer_round_trip_via_dashboard_helpers() {
    // End-to-end: save through the dashboard helper, then read through the
    // dashboard helper, must round-trip without touching disk.
    let root = fresh_state_root("roundtrip");
    {
        let mem = NativeCognitiveMemory::open(&root).expect("open native memory");
        save_goal_board(&GoalBoard::new(), &mem).expect("init empty board");
    }

    let board = seeded_board();
    dashboard_save_goal_board(&root, &board).expect("dashboard writer");

    let loaded = dashboard_goal_board_snapshot(&root).expect("dashboard reader");
    assert_eq!(loaded.active.len(), board.active.len());
    for expected in &board.active {
        assert!(
            loaded.active.iter().any(|g| g.id == expected.id),
            "round-tripped board must include {}",
            expected.id
        );
    }
    assert!(!root.join("goal_records.json").exists());
}
