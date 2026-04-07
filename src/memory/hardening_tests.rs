//! Tests for memory hardening: crash-safe write ordering, cross-session recall,
//! and list_all functionality.

use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::memory::{FileBackedMemoryStore, InMemoryMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
use crate::session::{SessionId, SessionPhase};

fn make_record(key: &str, scope: MemoryScope, session_id: &SessionId) -> MemoryRecord {
    MemoryRecord {
        key: key.to_string(),
        scope,
        value: format!("value-for-{key}"),
        session_id: session_id.clone(),
        recorded_in: SessionPhase::Execution,
        created_at: None,
    }
}

fn unique_path(prefix: &str) -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{unique}.json"))
}

// ============================================================================
// 1. Crash-safe write ordering (FileBackedMemoryStore)
// ============================================================================

#[test]
fn persist_failure_leaves_in_memory_state_unchanged() {
    let path = unique_path("crash-safe");
    let store = FileBackedMemoryStore::try_new(&path).unwrap();
    let sid = SessionId::from_uuid(Uuid::nil());

    // Successfully write a record.
    store
        .put(make_record("good-key", MemoryScope::Decision, &sid))
        .unwrap();
    assert_eq!(store.list(MemoryScope::Decision).unwrap().len(), 1);

    // Make the path unwritable by replacing it with a directory.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir_all(&path).unwrap();

    // Attempt to write — should fail because persist_json can't write to a dir.
    let result = store.put(make_record("bad-key", MemoryScope::Project, &sid));
    assert!(result.is_err(), "persist to a directory path should fail");

    // In-memory state should be unchanged — still only the original record.
    assert_eq!(
        store.list(MemoryScope::Decision).unwrap().len(),
        1,
        "in-memory state should not have changed after persist failure"
    );
    assert!(
        store.list(MemoryScope::Project).unwrap().is_empty(),
        "failed record should not appear in memory"
    );

    let _ = std::fs::remove_dir_all(&path);
}

#[test]
fn successful_persist_updates_both_disk_and_memory() {
    let path = unique_path("persist-both");
    let sid = SessionId::from_uuid(Uuid::nil());

    {
        let store = FileBackedMemoryStore::try_new(&path).unwrap();
        store
            .put(make_record("persisted", MemoryScope::Decision, &sid))
            .unwrap();
    }

    // Re-open from the same file — record should be loaded from disk.
    let store2 = FileBackedMemoryStore::try_new(&path).unwrap();
    let records = store2.list(MemoryScope::Decision).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].key, "persisted");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn update_existing_key_persists_correctly() {
    let path = unique_path("update-persist");
    let sid = SessionId::from_uuid(Uuid::nil());

    let store = FileBackedMemoryStore::try_new(&path).unwrap();
    store
        .put(make_record("dup", MemoryScope::Decision, &sid))
        .unwrap();
    store
        .put(MemoryRecord {
            key: "dup".to_string(),
            scope: MemoryScope::Decision,
            value: "updated-value".to_string(),
            session_id: sid.clone(),
            recorded_in: SessionPhase::Reflection,
            created_at: None,
        })
        .unwrap();

    // Re-open and verify the update was persisted, not duplicated.
    let store2 = FileBackedMemoryStore::try_new(&path).unwrap();
    let found = store2.list(MemoryScope::Decision).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].value, "updated-value");
    assert_eq!(found[0].recorded_in, SessionPhase::Reflection);

    let _ = std::fs::remove_file(&path);
}

// ============================================================================
// 2. Cross-session recall
// ============================================================================

#[test]
fn cross_session_write_then_read() {
    let path = unique_path("cross-session");
    let session_a = SessionId::from_uuid(Uuid::from_u128(1));
    let session_b = SessionId::from_uuid(Uuid::from_u128(2));

    // Session A writes a record.
    {
        let store = FileBackedMemoryStore::try_new(&path).unwrap();
        store
            .put(make_record("from-a", MemoryScope::Decision, &session_a))
            .unwrap();
    }

    // Session B opens the same file and reads the record.
    {
        let store = FileBackedMemoryStore::try_new(&path).unwrap();
        let all = store.list(MemoryScope::Decision).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].key, "from-a");
        assert_eq!(all[0].session_id, session_a);

        assert!(store.list_for_session(&session_b).unwrap().is_empty());
        assert_eq!(store.list_for_session(&session_a).unwrap().len(), 1);
    }

    let _ = std::fs::remove_file(&path);
}

#[test]
fn cross_session_list_all_returns_both_sessions() {
    let path = unique_path("cross-list-all");
    let sid_a = SessionId::from_uuid(Uuid::from_u128(10));
    let sid_b = SessionId::from_uuid(Uuid::from_u128(20));

    // Session A writes.
    {
        let store = FileBackedMemoryStore::try_new(&path).unwrap();
        store
            .put(make_record("session-a-key", MemoryScope::Decision, &sid_a))
            .unwrap();
    }

    // Session B writes.
    {
        let store = FileBackedMemoryStore::try_new(&path).unwrap();
        store
            .put(make_record("session-b-key", MemoryScope::Project, &sid_b))
            .unwrap();
    }

    // New process reads all.
    let store = FileBackedMemoryStore::try_new(&path).unwrap();
    let all = store.list_all().unwrap();
    assert_eq!(all.len(), 2);
    let keys: Vec<&str> = all.iter().map(|r| r.key.as_str()).collect();
    assert!(keys.contains(&"session-a-key"));
    assert!(keys.contains(&"session-b-key"));

    let _ = std::fs::remove_file(&path);
}

// ============================================================================
// 3. list_all tests
// ============================================================================

#[test]
fn in_memory_list_all_returns_all_records() {
    let store = InMemoryMemoryStore::try_default().unwrap();
    let sid = SessionId::from_uuid(Uuid::nil());

    store.put(make_record("a", MemoryScope::Decision, &sid)).unwrap();
    store.put(make_record("b", MemoryScope::Project, &sid)).unwrap();
    store.put(make_record("c", MemoryScope::Benchmark, &sid)).unwrap();

    let all = store.list_all().unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn file_backed_list_all_includes_all_scopes() {
    let path = unique_path("list-all-scopes");
    let store = FileBackedMemoryStore::try_new(&path).unwrap();
    let sid = SessionId::from_uuid(Uuid::nil());

    for (i, scope) in [
        MemoryScope::SessionScratch,
        MemoryScope::SessionSummary,
        MemoryScope::Decision,
        MemoryScope::Project,
        MemoryScope::Benchmark,
    ]
    .iter()
    .enumerate()
    {
        store
            .put(make_record(&format!("key-{i}"), *scope, &sid))
            .unwrap();
    }

    let all = store.list_all().unwrap();
    assert_eq!(all.len(), 5);

    let _ = std::fs::remove_file(&path);
}

// ============================================================================
// 4. Crash recovery — orphaned temp files
// ============================================================================

#[test]
fn store_works_with_orphaned_temp_file() {
    let path = unique_path("orphan-temp");

    // Create an orphaned temp file that simulates a crash during persist.
    let temp_path = path.with_extension("json.tmp");
    std::fs::write(&temp_path, b"garbage data from incomplete write").unwrap();

    // Store should still open and function correctly.
    let store = FileBackedMemoryStore::try_new(&path).unwrap();
    let sid = SessionId::from_uuid(Uuid::nil());
    store
        .put(make_record("after-orphan", MemoryScope::Decision, &sid))
        .unwrap();
    assert_eq!(store.list(MemoryScope::Decision).unwrap().len(), 1);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&temp_path);
}

// ============================================================================
// 5. Consistency invariants
// ============================================================================

#[test]
fn count_matches_list_length() {
    let store = InMemoryMemoryStore::try_default().unwrap();
    let sid = SessionId::from_uuid(Uuid::nil());

    store.put(make_record("x", MemoryScope::Decision, &sid)).unwrap();
    store.put(make_record("y", MemoryScope::Decision, &sid)).unwrap();
    store.put(make_record("z", MemoryScope::Project, &sid)).unwrap();

    let count = store.count_for_session(&sid).unwrap();
    let list = store.list_for_session(&sid).unwrap();
    assert_eq!(count, list.len());
    assert_eq!(count, 3);
}

#[test]
fn list_all_superset_of_scoped_lists() {
    let path = unique_path("superset");
    let store = FileBackedMemoryStore::try_new(&path).unwrap();
    let sid = SessionId::from_uuid(Uuid::nil());

    store.put(make_record("d1", MemoryScope::Decision, &sid)).unwrap();
    store.put(make_record("p1", MemoryScope::Project, &sid)).unwrap();

    let all = store.list_all().unwrap();
    let decisions = store.list(MemoryScope::Decision).unwrap();
    let projects = store.list(MemoryScope::Project).unwrap();

    assert_eq!(all.len(), decisions.len() + projects.len());

    let _ = std::fs::remove_file(&path);
}
