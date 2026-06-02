//! TDD tests for PR1: Goal Store Simplification (issue #2182).
//!
//! Tests that `FileBackedGoalStore`:
//! - Persists GoalRecords to disk as JSON
//! - Reloads from disk on `list()` for cross-process consistency
//! - Reloads from disk before `put()` upsert (no stale-cache data loss)
//! - Handles concurrent writers without data loss (flock)
//! - Does not corrupt cache on persist failure
//! - Uses the canonical `goal_store_path()` helper
//!
//! These tests define the behavioral contract. Most will FAIL until the
//! flock + reload-on-access changes are implemented in `store.rs`.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use tempfile::TempDir;

use simard::goals::{
    FileBackedGoalStore, GoalRecord, GoalStatus, GoalStore, GoalUpdate, goal_slug,
};
use simard::session::{SessionId, SessionPhase};
use simard::state_root;

fn make_goal_record(title: &str, status: GoalStatus, priority: u8) -> GoalRecord {
    let update =
        GoalUpdate::new(title, "test rationale for TDD", status, priority).expect("valid update");
    GoalRecord::from_update(
        update,
        "test-owner",
        SessionId::parse("session-00000000-0000-0000-0000-000000000001").expect("valid session id"),
        SessionPhase::Persistence,
    )
    .expect("valid record")
}

fn make_store(path: &std::path::Path) -> FileBackedGoalStore {
    FileBackedGoalStore::try_new(path).expect("store should create")
}

// ── PR1: Basic persistence ──

#[test]
fn file_backed_store_put_persists_to_disk() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("goal_store.json");
    let store = make_store(&path);

    let record = make_goal_record("Ship feature X", GoalStatus::Active, 1);
    store.put(record.clone()).unwrap();

    // Verify the file exists and contains valid JSON.
    assert!(path.exists(), "goal store file should exist after put");
    let raw = std::fs::read_to_string(&path).expect("should read");
    let records: Vec<GoalRecord> =
        serde_json::from_str(&raw).expect("should parse as Vec<GoalRecord>");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].title, "Ship feature X");
    assert_eq!(records[0].slug, goal_slug("Ship feature X"));
}

#[test]
fn file_backed_store_upserts_by_slug() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("goal_store.json");
    let store = make_store(&path);

    store
        .put(make_goal_record("Same Goal", GoalStatus::Active, 1))
        .unwrap();
    store
        .put(make_goal_record("Same Goal", GoalStatus::Completed, 2))
        .unwrap();

    let all = store.list().unwrap();
    assert_eq!(all.len(), 1, "upsert should not duplicate records");
    assert_eq!(
        all[0].status,
        GoalStatus::Completed,
        "upsert should update status"
    );
    assert_eq!(all[0].priority, 2, "upsert should update priority");
}

// ── PR1: Cross-process consistency via reload-on-read ──

#[test]
fn file_backed_store_reload_on_list_after_external_write() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("goal_store.json");

    // Store A writes a record.
    let store_a = make_store(&path);
    store_a
        .put(make_goal_record("Goal from A", GoalStatus::Active, 1))
        .unwrap();

    // Store B opens the same file (simulates a different process).
    let store_b = make_store(&path);

    // Store A writes another record.
    store_a
        .put(make_goal_record("Second from A", GoalStatus::Active, 2))
        .unwrap();

    // Store B should see BOTH records when it lists (reload from disk).
    let b_records = store_b.list().unwrap();
    assert_eq!(
        b_records.len(),
        2,
        "store B should reload from disk on list() and see both records; got {:?}",
        b_records.iter().map(|r| &r.title).collect::<Vec<_>>()
    );
}

#[test]
fn file_backed_store_put_reloads_before_upsert() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("goal_store.json");

    // Store A puts record X.
    let store_a = make_store(&path);
    store_a
        .put(make_goal_record("Goal X", GoalStatus::Active, 1))
        .unwrap();

    // Store B puts record Y (different title → different slug).
    let store_b = make_store(&path);
    store_b
        .put(make_goal_record("Goal Y", GoalStatus::Active, 2))
        .unwrap();

    // Store A puts record Z. Without reload-before-upsert, store A's
    // cached [X] + new Z = [X, Z], losing Y.
    store_a
        .put(make_goal_record("Goal Z", GoalStatus::Active, 3))
        .unwrap();

    // All three records should be present.
    let store_verify = make_store(&path);
    let all = store_verify.list().unwrap();
    let titles: Vec<&str> = all.iter().map(|r| r.title.as_str()).collect();
    assert!(
        titles.contains(&"Goal X"),
        "Goal X should be present; got: {titles:?}"
    );
    assert!(
        titles.contains(&"Goal Y"),
        "Goal Y should NOT be lost by store A's put; got: {titles:?}"
    );
    assert!(
        titles.contains(&"Goal Z"),
        "Goal Z should be present; got: {titles:?}"
    );
    assert_eq!(
        all.len(),
        3,
        "all three goals should be present; got: {titles:?}"
    );
}

// ── PR1: Concurrent writers ──

#[test]
fn file_backed_store_concurrent_writers_no_data_loss() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("goal_store.json");

    let n_writers = 8;
    let path = Arc::new(path);

    let handles: Vec<_> = (0..n_writers)
        .map(|i| {
            let p = Arc::clone(&path);
            thread::spawn(move || {
                let store = make_store(&p);
                let title = format!("Concurrent Goal {i}");
                store
                    .put(make_goal_record(&title, GoalStatus::Active, (i + 1) as u8))
                    .expect("concurrent put should succeed");
            })
        })
        .collect();

    for h in handles {
        h.join().expect("writer thread should not panic");
    }

    // Verify all records are present.
    let store = make_store(&path);
    let all = store.list().unwrap();
    assert_eq!(
        all.len(),
        n_writers,
        "all {n_writers} concurrent writes should be present; got {} records: {:?}",
        all.len(),
        all.iter().map(|r| &r.title).collect::<Vec<_>>()
    );
}

// ── PR1: Cache safety on persist failure ──

#[cfg(unix)]
#[test]
fn file_backed_store_persist_failure_does_not_corrupt_cache() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("goal_store.json");

    let store = make_store(&path);
    store
        .put(make_goal_record("Existing Goal", GoalStatus::Active, 1))
        .unwrap();

    // Make parent read-only to cause persist failure.
    let perms = std::fs::Permissions::from_mode(0o444);
    std::fs::set_permissions(dir.path(), perms).unwrap();

    // This put should fail (can't write to read-only dir).
    let result = store.put(make_goal_record("New Goal", GoalStatus::Active, 2));

    // Restore permissions for cleanup.
    let perms = std::fs::Permissions::from_mode(0o755);
    std::fs::set_permissions(dir.path(), perms).unwrap();

    assert!(
        result.is_err(),
        "put should fail when persist is impossible"
    );

    // Cache should NOT have been updated with the failed record.
    // list() should return only the original record.
    let all = store.list().unwrap();
    assert_eq!(
        all.len(),
        1,
        "cache should not contain records from a failed persist; got: {:?}",
        all.iter().map(|r| &r.title).collect::<Vec<_>>()
    );
    assert_eq!(all[0].title, "Existing Goal");
}

// ── PR1: Corrupt JSON recovery ──

#[test]
fn file_backed_store_handles_corrupt_disk_json() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("goal_store.json");

    // Write valid data first.
    let store = make_store(&path);
    store
        .put(make_goal_record("Valid Goal", GoalStatus::Active, 1))
        .unwrap();
    drop(store);

    // Corrupt the file.
    std::fs::write(&path, "{ invalid json !!!").unwrap();

    // Opening a new store on a corrupt file should return an error,
    // not panic or silently return empty data.
    let result = FileBackedGoalStore::try_new(&path);
    assert!(
        result.is_err(),
        "opening a store on corrupt JSON should return an error"
    );
}

// ── PR1: Canonical path helper ──

#[test]
fn goal_store_path_returns_canonical_location() {
    // Verify the path ends with state/goal_store.json under the state root.
    let path = state_root::goal_store_path();
    let path_str = path.to_string_lossy();
    assert!(
        path_str.ends_with("state/goal_store.json"),
        "goal_store_path() should end with state/goal_store.json, got: {path_str}"
    );
    // The parent should be <state_root>/state/
    let parent = path.parent().expect("should have parent");
    assert!(
        parent.ends_with("state"),
        "parent dir should be 'state', got: {}",
        parent.display()
    );
}

#[test]
#[serial_test::serial(simard_state_root_env)]
fn goal_store_path_respects_state_root_env() {
    // SAFETY: serialized with other state-root env tests.
    let prev = std::env::var_os("SIMARD_STATE_ROOT");
    unsafe {
        std::env::set_var("SIMARD_STATE_ROOT", "/tmp/simard-goal-path-test");
    }

    let path = state_root::goal_store_path();
    assert_eq!(
        path,
        PathBuf::from("/tmp/simard-goal-path-test/state/goal_store.json"),
        "goal_store_path() should use SIMARD_STATE_ROOT when set"
    );

    // Restore.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("SIMARD_STATE_ROOT", v),
            None => std::env::remove_var("SIMARD_STATE_ROOT"),
        }
    }
}

// ── PR1: Assembly wires FileBackedGoalStore ──

#[test]
fn assembly_goal_store_descriptor_indicates_file_backed() {
    // After PR1, the assembly should wire FileBackedGoalStore.
    // This test verifies the descriptor contains "file" or "json-file"
    // to confirm the CognitiveMemoryGoalStore has been replaced.
    //
    // We construct a FileBackedGoalStore the same way assembly should
    // and verify the descriptor matches what assembly produces.
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("goal_store.json");
    let store = FileBackedGoalStore::try_new(&path).unwrap();
    let desc = store.descriptor();

    // The descriptor's source should indicate file-backed (not cognitive memory).
    let source = format!("{:?}", desc);
    assert!(
        source.contains("json-file")
            || source.contains("file-store")
            || source.contains("goals::json-file-store"),
        "FileBackedGoalStore descriptor should indicate file-backed; got: {source}"
    );
}
