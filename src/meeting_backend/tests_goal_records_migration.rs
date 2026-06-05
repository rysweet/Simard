//! Tests for issue #1668: meeting_backend goal reads/writes flow through
//! `CognitiveMemoryGoalStore` instead of the legacy `FileBackedGoalStore`,
//! and existing `state/goal_store.json` files are migrated on first access.

use std::path::Path;
use std::sync::Arc;

use crate::cognitive_memory::NativeCognitiveMemory;
use crate::goals::{
    CognitiveMemoryGoalStore, GoalRecord, GoalStatus, GoalStore, GoalUpdate,
    migrate_file_backed_goal_store_if_present,
};
use crate::memory_ipc::{clear_in_process_writer, register_in_process_writer};
use crate::session::{SessionId, SessionPhase};
use crate::test_support::HermeticState;

fn record(title: &str, status: GoalStatus, priority: u8) -> GoalRecord {
    GoalRecord::from_update(
        GoalUpdate::new(title, "meeting migration test", status, priority).expect("valid update"),
        "meeting-close-test",
        SessionId::parse("session-018f1f7e-4c5d-7b2a-8f10-b5c0d4f7b123").expect("valid session id"),
        SessionPhase::Persistence,
    )
    .expect("valid record")
}

/// Write a legacy `state/goal_store.json` file in the format that
/// `FileBackedGoalStore` produces (a JSON array of `GoalRecord`).
fn write_legacy_goal_store(state_root: &Path, records: &[GoalRecord]) {
    let state_dir = state_root.join("state");
    std::fs::create_dir_all(&state_dir).expect("create state dir");
    let path = state_dir.join("goal_store.json");
    let json = serde_json::to_string_pretty(records).expect("serialize records");
    std::fs::write(&path, json).expect("write legacy file");
}

// ── Migration tests ──

#[test]
#[serial_test::serial(cognitive_memory)]
fn migration_imports_legacy_records_into_cognitive_memory() {
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();

    let legacy_records = vec![
        record("Deploy monitoring stack", GoalStatus::Active, 1),
        record("Refactor auth module", GoalStatus::Active, 2),
    ];
    write_legacy_goal_store(&root, &legacy_records);
    assert!(root.join("state/goal_store.json").exists());

    migrate_file_backed_goal_store_if_present(&root);

    // Legacy file should be renamed to .migrated
    assert!(
        !root.join("state/goal_store.json").exists(),
        "legacy file must be renamed after successful migration"
    );
    assert!(
        root.join("state/goal_store.json.migrated").exists(),
        "legacy file must be renamed to .migrated"
    );

    // Records should be in cognitive memory
    let store = CognitiveMemoryGoalStore::new(root).expect("store");
    let listed = store.list().expect("list");
    assert_eq!(
        listed.len(),
        2,
        "both legacy records must appear in cognitive memory after migration"
    );
    assert!(listed.iter().any(|r| r.title == "Deploy monitoring stack"));
    assert!(listed.iter().any(|r| r.title == "Refactor auth module"));
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn migration_skips_slugs_already_in_cognitive_memory() {
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();

    // Register an in-process writer so that put(), migration, and list()
    // share a single NativeCognitiveMemory handle.  Without this, each
    // launch_writer_bridge / open_reader_bridge opens a separate DB
    // instance, and LadybugDB's WAL may not be visible across sequential
    // open/close cycles under CI (coverage instrumentation, GitHub Actions
    // runners).
    let mem = Arc::new(NativeCognitiveMemory::open(&root).expect("open cognitive memory"));
    register_in_process_writer(root.clone(), mem.clone());

    // Pre-populate cognitive memory with one record
    let store = CognitiveMemoryGoalStore::new(root.clone()).expect("store");
    store
        .put(record("Deploy monitoring stack", GoalStatus::Completed, 1))
        .expect("put");

    // Write a legacy file with the same slug (different status)
    let legacy_records = vec![
        record("Deploy monitoring stack", GoalStatus::Active, 1),
        record("New legacy goal", GoalStatus::Active, 2),
    ];
    write_legacy_goal_store(&root, &legacy_records);

    migrate_file_backed_goal_store_if_present(&root);

    let listed = store.list().expect("list");
    // The stale legacy "Deploy monitoring stack" should be skipped;
    // the existing Completed version in cognitive memory should remain.
    let deploy = listed
        .iter()
        .find(|r| r.slug == "deploy-monitoring-stack")
        .expect("deploy goal must exist");
    assert_eq!(
        deploy.status,
        GoalStatus::Completed,
        "existing cognitive-memory record must not be overwritten by stale legacy data"
    );
    // The new legacy goal should be migrated
    assert!(
        listed.iter().any(|r| r.title == "New legacy goal"),
        "new legacy records must be migrated"
    );

    clear_in_process_writer();
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn migration_leaves_corrupt_file_in_place() {
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();

    let state_dir = root.join("state");
    std::fs::create_dir_all(&state_dir).expect("create state dir");
    std::fs::write(state_dir.join("goal_store.json"), "not valid json {{{").expect("write");

    migrate_file_backed_goal_store_if_present(&root);

    assert!(
        root.join("state/goal_store.json").exists(),
        "corrupt file must be left in place for operator inspection"
    );
    assert!(
        !root.join("state/goal_store.json.migrated").exists(),
        "corrupt file must not be renamed"
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn migration_is_noop_when_no_legacy_file_exists() {
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();

    // No file to migrate
    migrate_file_backed_goal_store_if_present(&root);

    assert!(!root.join("state/goal_store.json").exists());
    assert!(!root.join("state/goal_store.json.migrated").exists());
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn migration_handles_empty_records_array() {
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();

    write_legacy_goal_store(&root, &[]);

    migrate_file_backed_goal_store_if_present(&root);

    assert!(
        !root.join("state/goal_store.json").exists(),
        "empty legacy file should still be renamed"
    );
    assert!(root.join("state/goal_store.json.migrated").exists());

    let store = CognitiveMemoryGoalStore::new(root).expect("store");
    let listed = store.list().expect("list");
    assert!(listed.is_empty(), "no records to migrate from empty file");
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn write_goals_from_decisions_does_not_produce_legacy_file() {
    // After issue #1668, goal writes from meeting close must flow through
    // cognitive memory, NOT the file-backed store.
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();

    // Register an in-process writer so put() and list() share a single
    // NativeCognitiveMemory handle — avoids WAL visibility issues across
    // sequential DB open/close cycles under CI.
    let mem = Arc::new(NativeCognitiveMemory::open(&root).expect("open cognitive memory"));
    register_in_process_writer(root.clone(), mem.clone());

    // Write a goal directly through CognitiveMemoryGoalStore (matching
    // the new code path in write_goals_from_decisions)
    let store = CognitiveMemoryGoalStore::new(root.clone()).expect("store");
    store
        .put(record("Meeting decision goal", GoalStatus::Active, 1))
        .expect("put");

    assert!(
        !root.join("state/goal_store.json").exists(),
        "CognitiveMemoryGoalStore must not produce state/goal_store.json"
    );
    assert!(
        !root.join("goal_records.json").exists(),
        "CognitiveMemoryGoalStore must not produce goal_records.json"
    );

    // Verify the goal is readable
    let listed = store.list().expect("list");
    assert!(
        listed.iter().any(|r| r.title == "Meeting decision goal"),
        "goal written via CognitiveMemoryGoalStore must be readable"
    );

    clear_in_process_writer();
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn load_active_goal_titles_reads_from_cognitive_memory() {
    // After issue #1668, meeting enrichment reads from cognitive memory.
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();

    let store = CognitiveMemoryGoalStore::new(root.clone()).expect("store");
    store
        .put(record("Alpha goal", GoalStatus::Active, 1))
        .expect("put");
    store
        .put(record("Beta goal", GoalStatus::Active, 2))
        .expect("put");
    store
        .put(record("Proposed only", GoalStatus::Proposed, 1))
        .expect("put");

    // active_top_goals must return only Active records
    let active = store.active_top_goals(50).expect("active_top_goals");
    assert_eq!(active.len(), 2, "only active goals should be returned");
    assert!(active.iter().all(|r| r.status == GoalStatus::Active));

    assert!(
        !root.join("state/goal_store.json").exists(),
        "reads must not produce legacy file"
    );
}
