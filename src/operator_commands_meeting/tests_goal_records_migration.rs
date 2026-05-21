//! Failing TDD tests (issue #1590, Step 7) for the meeting paths' migration
//! off `goal_records.json`.
//!
//! Spec sections 8 (`goal_curation.rs`) and 9 (`improvement_curation.rs`):
//! these read probes must obtain their goal data via
//! `active_goals_as_records(&load_goal_board(&launch_writer_bridge(...)))`
//! rather than `FileBackedGoalStore::try_new(... "goal_records.json")`.
//!
//! These tests will not compile until the migration adds
//! `crate::goal_curation::active_goals_as_records` and
//! `crate::memory_ipc::launch_writer_bridge`.

use crate::cognitive_memory::NativeCognitiveMemory;
use crate::goal_curation::{
    ActiveGoal, GoalBoard, GoalProgress, active_goals_as_records, load_goal_board, save_goal_board,
};
use crate::goals::GoalStatus;
use crate::test_support::HermeticState;

use super::run_goal_curation_read_probe;

fn seed_active_only(state_root: &std::path::Path, n: usize) -> GoalBoard {
    let mem = NativeCognitiveMemory::open(state_root).expect("open native memory");
    let mut board = GoalBoard::new();
    for i in 0..n {
        board.active.push(ActiveGoal {
            id: format!("meeting-mig-active-goal-{i:02}"),
            description: format!("Meeting migration active goal #{i:02}"),
            priority: (i + 1) as u32,
            status: GoalProgress::NotStarted,
            assigned_to: Some("simard".to_string()),
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        });
    }
    save_goal_board(&board, &mem).expect("seed active goals");
    board
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn meeting_goal_curation_read_probe_succeeds_with_only_cognitive_memory() {
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();
    let _seeded = seed_active_only(&root, 3);
    assert!(
        !root.join("goal_records.json").exists(),
        "precondition: legacy file must not exist"
    );

    let result =
        run_goal_curation_read_probe("local-harness", "single-process", Some(root.clone()));

    assert!(
        result.is_ok(),
        "read probe must succeed against cognitive memory only (no legacy file present): {:?}",
        result.err()
    );
    assert!(
        !root.join("goal_records.json").exists(),
        "read probe must not write the legacy file"
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn meeting_goal_curation_read_probe_succeeds_with_empty_cognitive_memory() {
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();
    {
        let mem = NativeCognitiveMemory::open(&root).expect("open native memory");
        save_goal_board(&GoalBoard::new(), &mem).expect("save empty board");
    }
    assert!(!root.join("goal_records.json").exists());

    let result =
        run_goal_curation_read_probe("local-harness", "single-process", Some(root.clone()));
    assert!(
        result.is_ok(),
        "empty-memory read probe must succeed: {:?}",
        result.err()
    );
    assert!(!root.join("goal_records.json").exists());
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn active_goals_as_records_round_trips_through_cognitive_memory() {
    // End-to-end: seeded active goals come back via load_goal_board +
    // active_goals_as_records as `Vec<GoalRecord>` with the right shape
    // and the right count — same contract that meeting / engineer rely on.
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();
    let seeded = seed_active_only(&root, 4);

    let mem = NativeCognitiveMemory::open(&root).expect("reopen native memory");
    let loaded = load_goal_board(&mem).expect("load_goal_board");
    let records = active_goals_as_records(&loaded);

    assert_eq!(
        records.len(),
        seeded.active.len(),
        "all seeded active goals must surface as GoalRecords"
    );
    for (i, r) in records.iter().enumerate() {
        assert_eq!(
            r.status,
            GoalStatus::Active,
            "non-completed seeded goals must map to GoalStatus::Active (record #{i} got {:?})",
            r.status
        );
        assert!(
            !r.slug.is_empty(),
            "every record must have a non-empty slug"
        );
        assert!(
            !r.title.is_empty(),
            "every record must have a non-empty title"
        );
    }
}
