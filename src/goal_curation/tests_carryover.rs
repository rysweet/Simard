//! Tests for explicit meeting-to-engineer goal carryover (issue #2092).
//!
//! Verifies that:
//! - Goals survive a state-root path change when a carryover record exists.
//! - Missing carryover records produce a clear `NoRecord` result (not silent loss).
//! - Board drift (missing goals) produces a `Drifted` verification result.
//! - The carryover write-read round-trip works correctly.

use serial_test::serial;
use tempfile::TempDir;

use crate::goal_curation::{
    ActiveGoal, CarryoverVerification, GoalBoard, GoalProgress, add_active_goal,
    board_snapshot_hash, load_goal_board, read_latest_carryover, save_goal_board,
    verify_goal_carryover, write_goal_carryover,
};
use crate::memory_ipc::launch_writer_client;
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
        priority_explicit: false,
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

// ─── board_snapshot_hash ─────────────────────────────────────────────────

#[test]
fn board_snapshot_hash_is_deterministic() {
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("g1", 1)).unwrap();
    let h1 = board_snapshot_hash(&board);
    let h2 = board_snapshot_hash(&board);
    assert_eq!(h1, h2, "hash must be deterministic for the same board");
}

#[test]
fn board_snapshot_hash_changes_on_mutation() {
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("g1", 1)).unwrap();
    let h1 = board_snapshot_hash(&board);
    add_active_goal(&mut board, active_goal("g2", 2)).unwrap();
    let h2 = board_snapshot_hash(&board);
    assert_ne!(h1, h2, "hash must change when the board changes");
}

#[test]
fn board_snapshot_hash_empty_board() {
    let board = GoalBoard::new();
    let h = board_snapshot_hash(&board);
    assert!(!h.is_empty(), "hash of empty board must be non-empty");
}

// ─── write + read carryover round-trip ───────────────────────────────────

#[test]
#[serial(cognitive_memory)]
fn carryover_round_trip_succeeds() {
    let (_tmp, root) = isolated_state_root();
    let bridge = launch_writer_client(&root).expect("writer bridge");

    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("goal-alpha", 1)).unwrap();
    add_active_goal(&mut board, active_goal("goal-beta", 2)).unwrap();
    save_goal_board(&board, bridge.ops()).expect("save board");

    write_goal_carryover(&board, "meeting-2026-01-01", bridge.ops()).expect("write carryover");

    let record = read_latest_carryover(bridge.ops())
        .expect("read carryover")
        .expect("carryover record must exist");

    assert_eq!(record.meeting_id, "meeting-2026-01-01");
    assert_eq!(record.active_goal_count, 2);
    assert_eq!(record.active_goal_ids, vec!["goal-alpha", "goal-beta"]);
    assert!(!record.acknowledged);
    assert_eq!(record.board_snapshot_hash, board_snapshot_hash(&board));
}

// ─── verify_goal_carryover ───────────────────────────────────────────────

#[test]
#[serial(cognitive_memory)]
fn verify_no_record_on_fresh_state() {
    let (_tmp, root) = isolated_state_root();
    let bridge = launch_writer_client(&root).expect("writer bridge");

    let board = GoalBoard::new();
    let result = verify_goal_carryover(&board, bridge.ops()).expect("verify");
    assert_eq!(result, CarryoverVerification::NoRecord);
}

#[test]
#[serial(cognitive_memory)]
fn verify_succeeds_when_board_matches() {
    let (_tmp, root) = isolated_state_root();
    let bridge = launch_writer_client(&root).expect("writer bridge");

    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("g1", 1)).unwrap();
    save_goal_board(&board, bridge.ops()).expect("save");
    write_goal_carryover(&board, "mtg-001", bridge.ops()).expect("carryover");

    // Reload board from same bridge — should match.
    let loaded = load_goal_board(bridge.ops()).expect("load");
    let result = verify_goal_carryover(&loaded, bridge.ops()).expect("verify");
    match result {
        CarryoverVerification::Verified {
            meeting_id,
            active_goal_count,
        } => {
            assert_eq!(meeting_id, "mtg-001");
            assert_eq!(active_goal_count, 1);
        }
        other => panic!("expected Verified, got {other:?}"),
    }
}

#[test]
#[serial(cognitive_memory)]
fn verify_detects_drift_when_goals_missing() {
    let (_tmp, root) = isolated_state_root();
    let bridge = launch_writer_client(&root).expect("writer bridge");

    // Meeting produces board with 3 goals.
    let mut meeting_board = GoalBoard::new();
    add_active_goal(&mut meeting_board, active_goal("g1", 1)).unwrap();
    add_active_goal(&mut meeting_board, active_goal("g2", 2)).unwrap();
    add_active_goal(&mut meeting_board, active_goal("g3", 3)).unwrap();
    save_goal_board(&meeting_board, bridge.ops()).expect("save meeting board");
    write_goal_carryover(&meeting_board, "mtg-drift", bridge.ops()).expect("write carryover");

    // Engineer board has only 1 of the 3 goals (simulates state-root
    // divergence where g2 and g3 were lost).
    let mut engineer_board = GoalBoard::new();
    add_active_goal(&mut engineer_board, active_goal("g1", 1)).unwrap();

    let result = verify_goal_carryover(&engineer_board, bridge.ops()).expect("verify");
    match result {
        CarryoverVerification::Drifted {
            meeting_id,
            missing_goal_ids,
            ..
        } => {
            assert_eq!(meeting_id, "mtg-drift");
            assert!(
                missing_goal_ids.contains(&"g2".to_string()),
                "g2 should be reported missing"
            );
            assert!(
                missing_goal_ids.contains(&"g3".to_string()),
                "g3 should be reported missing"
            );
        }
        other => panic!("expected Drifted, got {other:?}"),
    }
}

/// Goals survive when engineer reads from the *same* state root the
/// meeting wrote to — the carryover record confirms the handoff.
#[test]
#[serial(cognitive_memory)]
fn goals_survive_same_state_root() {
    let (_tmp, root) = isolated_state_root();

    // Meeting writes goals + carryover.
    let meeting_bridge = launch_writer_client(&root).expect("meeting bridge");
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("survive-1", 1)).unwrap();
    add_active_goal(&mut board, active_goal("survive-2", 2)).unwrap();
    save_goal_board(&board, meeting_bridge.ops()).expect("save");
    write_goal_carryover(&board, "mtg-survive", meeting_bridge.ops()).expect("carryover");
    drop(meeting_bridge);

    // Engineer reads from same state root — should verify clean.
    let eng_bridge = launch_writer_client(&root).expect("eng bridge");
    let loaded = load_goal_board(eng_bridge.ops()).expect("load");
    assert_eq!(loaded.active.len(), 2, "both goals must survive");

    let result = verify_goal_carryover(&loaded, eng_bridge.ops()).expect("verify");
    match result {
        CarryoverVerification::Verified { meeting_id, .. } => {
            assert_eq!(meeting_id, "mtg-survive");
        }
        other => panic!("expected Verified, got {other:?}"),
    }
}

/// When the state root diverges (different TempDir), the engineer's board
/// is empty and the carryover record is absent — `NoRecord` is the
/// expected result, not a silent success with zero goals.
#[test]
#[serial(cognitive_memory)]
fn diverged_state_root_produces_no_record() {
    let (_tmp1, root1) = isolated_state_root();

    // Meeting writes to root1.
    let bridge1 = launch_writer_client(&root1).expect("bridge1");
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("lost-goal", 1)).unwrap();
    save_goal_board(&board, bridge1.ops()).expect("save");
    write_goal_carryover(&board, "mtg-lost", bridge1.ops()).expect("carryover");
    drop(bridge1);

    // Engineer reads from root2 — different state root entirely.
    let tmp2 = tempfile::tempdir().expect("tempdir2");
    let root2 = tmp2.path().to_path_buf();
    unsafe {
        std::env::set_var(STATE_ROOT_ENV, &root2);
    }
    let bridge2 = launch_writer_client(&root2).expect("bridge2");
    let loaded = load_goal_board(bridge2.ops()).expect("load from new root");

    // Board is empty because the new state root has no data.
    assert!(loaded.active.is_empty(), "new root should have no goals");

    // Carryover record is also absent on the new root.
    let result = verify_goal_carryover(&loaded, bridge2.ops()).expect("verify");
    assert_eq!(
        result,
        CarryoverVerification::NoRecord,
        "diverged state root must produce NoRecord, not silent success"
    );
}

/// Multiple carryover writes: only the latest is used for verification.
#[test]
#[serial(cognitive_memory)]
fn latest_carryover_record_wins() {
    let (_tmp, root) = isolated_state_root();
    let bridge = launch_writer_client(&root).expect("bridge");

    // First meeting writes 1 goal.
    let mut board1 = GoalBoard::new();
    add_active_goal(&mut board1, active_goal("old-goal", 1)).unwrap();
    save_goal_board(&board1, bridge.ops()).expect("save1");
    write_goal_carryover(&board1, "mtg-old", bridge.ops()).expect("carryover1");

    // Second meeting writes 2 goals (supersedes first).
    let mut board2 = GoalBoard::new();
    add_active_goal(&mut board2, active_goal("new-g1", 1)).unwrap();
    add_active_goal(&mut board2, active_goal("new-g2", 2)).unwrap();
    save_goal_board(&board2, bridge.ops()).expect("save2");
    write_goal_carryover(&board2, "mtg-new", bridge.ops()).expect("carryover2");

    let record = read_latest_carryover(bridge.ops())
        .expect("read")
        .expect("must have record");
    assert_eq!(record.meeting_id, "mtg-new");
    assert_eq!(record.active_goal_count, 2);

    // Verify against the latest board.
    let result = verify_goal_carryover(&board2, bridge.ops()).expect("verify");
    match result {
        CarryoverVerification::Verified { meeting_id, .. } => {
            assert_eq!(meeting_id, "mtg-new");
        }
        other => panic!("expected Verified, got {other:?}"),
    }
}
