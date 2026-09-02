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
        labels: Vec::new(),
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
    let memory = launch_writer_client(&root).expect("writer memory");

    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("goal-alpha", 1)).unwrap();
    add_active_goal(&mut board, active_goal("goal-beta", 2)).unwrap();
    save_goal_board(&board, memory.ops()).expect("save board");

    write_goal_carryover(&board, "meeting-2026-01-01", memory.ops()).expect("write carryover");

    let record = read_latest_carryover(memory.ops())
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
    let memory = launch_writer_client(&root).expect("writer memory");

    let board = GoalBoard::new();
    let result = verify_goal_carryover(&board, memory.ops()).expect("verify");
    assert_eq!(result, CarryoverVerification::NoRecord);
}

#[test]
#[serial(cognitive_memory)]
fn verify_succeeds_when_board_matches() {
    let (_tmp, root) = isolated_state_root();
    let memory = launch_writer_client(&root).expect("writer memory");

    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("g1", 1)).unwrap();
    save_goal_board(&board, memory.ops()).expect("save");
    write_goal_carryover(&board, "mtg-001", memory.ops()).expect("carryover");

    // Reload board from same memory — should match.
    let loaded = load_goal_board(memory.ops()).expect("load");
    let result = verify_goal_carryover(&loaded, memory.ops()).expect("verify");
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
    let memory = launch_writer_client(&root).expect("writer memory");

    // Meeting produces board with 3 goals.
    let mut meeting_board = GoalBoard::new();
    add_active_goal(&mut meeting_board, active_goal("g1", 1)).unwrap();
    add_active_goal(&mut meeting_board, active_goal("g2", 2)).unwrap();
    add_active_goal(&mut meeting_board, active_goal("g3", 3)).unwrap();
    save_goal_board(&meeting_board, memory.ops()).expect("save meeting board");
    write_goal_carryover(&meeting_board, "mtg-drift", memory.ops()).expect("write carryover");

    // Engineer board has only 1 of the 3 goals (simulates state-root
    // divergence where g2 and g3 were lost).
    let mut engineer_board = GoalBoard::new();
    add_active_goal(&mut engineer_board, active_goal("g1", 1)).unwrap();

    let result = verify_goal_carryover(&engineer_board, memory.ops()).expect("verify");
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
    let meeting_memory = launch_writer_client(&root).expect("meeting memory");
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("survive-1", 1)).unwrap();
    add_active_goal(&mut board, active_goal("survive-2", 2)).unwrap();
    save_goal_board(&board, meeting_memory.ops()).expect("save");
    write_goal_carryover(&board, "mtg-survive", meeting_memory.ops()).expect("carryover");
    drop(meeting_memory);

    // Engineer reads from same state root — should verify clean.
    let eng_memory = launch_writer_client(&root).expect("eng memory");
    let loaded = load_goal_board(eng_memory.ops()).expect("load");
    assert_eq!(loaded.active.len(), 2, "both goals must survive");

    let result = verify_goal_carryover(&loaded, eng_memory.ops()).expect("verify");
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
    let memory1 = launch_writer_client(&root1).expect("memory1");
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("lost-goal", 1)).unwrap();
    save_goal_board(&board, memory1.ops()).expect("save");
    write_goal_carryover(&board, "mtg-lost", memory1.ops()).expect("carryover");
    drop(memory1);

    // Engineer reads from root2 — different state root entirely.
    let tmp2 = tempfile::tempdir().expect("tempdir2");
    let root2 = tmp2.path().to_path_buf();
    unsafe {
        std::env::set_var(STATE_ROOT_ENV, &root2);
    }
    let memory2 = launch_writer_client(&root2).expect("memory2");
    let loaded = load_goal_board(memory2.ops()).expect("load from new root");

    // Board is empty because the new state root has no data.
    assert!(loaded.active.is_empty(), "new root should have no goals");

    // Carryover record is also absent on the new root.
    let result = verify_goal_carryover(&loaded, memory2.ops()).expect("verify");
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
    let memory = launch_writer_client(&root).expect("memory");

    // First meeting writes 1 goal.
    let mut board1 = GoalBoard::new();
    add_active_goal(&mut board1, active_goal("old-goal", 1)).unwrap();
    save_goal_board(&board1, memory.ops()).expect("save1");
    write_goal_carryover(&board1, "mtg-old", memory.ops()).expect("carryover1");

    // Second meeting writes 2 goals (supersedes first).
    let mut board2 = GoalBoard::new();
    add_active_goal(&mut board2, active_goal("new-g1", 1)).unwrap();
    add_active_goal(&mut board2, active_goal("new-g2", 2)).unwrap();
    save_goal_board(&board2, memory.ops()).expect("save2");
    write_goal_carryover(&board2, "mtg-new", memory.ops()).expect("carryover2");

    let record = read_latest_carryover(memory.ops())
        .expect("read")
        .expect("must have record");
    assert_eq!(record.meeting_id, "mtg-new");
    assert_eq!(record.active_goal_count, 2);

    // Verify against the latest board.
    let result = verify_goal_carryover(&board2, memory.ops()).expect("verify");
    match result {
        CarryoverVerification::Verified { meeting_id, .. } => {
            assert_eq!(meeting_id, "mtg-new");
        }
        other => panic!("expected Verified, got {other:?}"),
    }
}

// ─── tombstone filter on the board READ path ─────────────────────────────
//
// Regression for the roster-goal escalation storm: a goal removed from the
// board and tombstoned could still linger inside an older
// `goal-board:snapshot` fact in cognitive memory. The OODA cycle loads via
// `load_goal_board` -> `read_latest_snapshot`, which historically did NOT
// consult the tombstone set on this hot path — so the dead goal was
// re-materialised every cycle and the no-progress breaker filed a fresh
// duplicate escalation issue forever. `load_goal_board` must now drop any
// tombstoned goal on read.
#[test]
#[serial(cognitive_memory)]
fn load_goal_board_filters_tombstoned_goals_from_snapshot() {
    let (_tmp, root) = isolated_state_root();
    let memory = launch_writer_client(&root).expect("writer memory");

    // Persist a snapshot that still contains a since-tombstoned goal.
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, active_goal("goal-keep", 1)).unwrap();
    add_active_goal(&mut board, active_goal("goal-doomed", 2)).unwrap();
    save_goal_board(&board, memory.ops()).expect("save board");

    // Tombstone the doomed goal (as `simard goal remove/complete` does).
    crate::ooda_loop::tombstone_goals(&root, &["goal-doomed".to_string()])
        .expect("tombstone doomed goal");

    // The OODA read path must not resurrect the tombstoned goal.
    let loaded = load_goal_board(memory.ops()).expect("load");
    let ids: Vec<&str> = loaded.active.iter().map(|g| g.id.as_str()).collect();
    assert!(
        ids.contains(&"goal-keep"),
        "live goal must survive the tombstone filter, got {ids:?}"
    );
    assert!(
        !ids.contains(&"goal-doomed"),
        "tombstoned goal must be filtered from the loaded board, got {ids:?}"
    );
}
