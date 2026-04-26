//! Tests for memory hardening: crash-safe write ordering, cross-session recall,
//! and list_all functionality.

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::memory::{
    FileBackedMemoryStore, InMemoryMemoryStore, MemoryRecord, MemoryScope, MemoryStore,
};
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
fn list_by_time_range_excludes_records_without_timestamp() {
    let store = InMemoryMemoryStore::try_default().unwrap();
    let sid = SessionId::from_uuid(Uuid::nil());

    // put() auto-stamps, so the "timestamped" record will have a created_at.
    store
        .put(make_record("timestamped", MemoryScope::Project, &sid))
        .unwrap();

    let now = Utc::now();
    let results = store
        .list_by_time_range(now - Duration::hours(1), now + Duration::hours(1))
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "timestamped");
}

// ============================================================================
// 7. Untagged scope
// ============================================================================

#[test]
fn untagged_scope_serialization_roundtrip() {
    let sid = SessionId::from_uuid(Uuid::nil());
    let record = MemoryRecord {
        key: "untagged-key".to_string(),
        scope: MemoryScope::Untagged,
        value: "test".to_string(),
        session_id: sid,
        recorded_in: SessionPhase::Execution,
        created_at: None,
    };
    let json = serde_json::to_string(&record).unwrap();
    assert!(
        json.contains("untagged"),
        "scope should serialize as 'untagged'"
    );
    let deserialized: MemoryRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.scope, MemoryScope::Untagged);
}

#[test]
fn untagged_scope_in_file_backed_store() {
    let path = unique_path("untagged-fb");
    let store = FileBackedMemoryStore::try_new(&path).unwrap();
    let sid = SessionId::from_uuid(Uuid::nil());

    store
        .put(make_record("u1", MemoryScope::Untagged, &sid))
        .unwrap();
    let results = store.list(MemoryScope::Untagged).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "u1");

    let _ = std::fs::remove_file(&path);
}

// ============================================================================
// 8. created_at auto-stamping
// ============================================================================

#[test]
fn put_stamps_created_at_when_none() {
    let store = InMemoryMemoryStore::try_default().unwrap();
    let sid = SessionId::from_uuid(Uuid::nil());
    let record = make_record("stamp-test", MemoryScope::Decision, &sid);
    assert!(record.created_at.is_none());

    store.put(record).unwrap();
    let stored = store.list(MemoryScope::Decision).unwrap();
    assert!(
        stored[0].created_at.is_some(),
        "put() should auto-stamp created_at"
    );
}

#[test]
fn put_preserves_existing_created_at() {
    let store = InMemoryMemoryStore::try_default().unwrap();
    let sid = SessionId::from_uuid(Uuid::nil());
    let fixed_time = Utc::now() - Duration::days(7);
    let mut record = make_record("preserve-test", MemoryScope::Decision, &sid);
    record.created_at = Some(fixed_time);

    store.put(record).unwrap();
    let stored = store.list(MemoryScope::Decision).unwrap();
    assert_eq!(
        stored[0].created_at.unwrap(),
        fixed_time,
        "put() should not overwrite existing created_at"
    );
}

#[test]
fn file_backed_put_auto_stamps_created_at() {
    let path = unique_path("auto-stamp-fb");
    let store = FileBackedMemoryStore::try_new(&path).unwrap();
    let sid = SessionId::from_uuid(Uuid::nil());

    store
        .put(make_record("stamp-fb", MemoryScope::Decision, &sid))
        .unwrap();
    let stored = store.list(MemoryScope::Decision).unwrap();
    assert!(
        stored[0].created_at.is_some(),
        "file-backed put() should auto-stamp created_at"
    );

    let _ = std::fs::remove_file(&path);
}

// Session lifecycle hooks would go here once on_session_start/on_session_end
// are added to the MemoryStore trait.

// ============================================================================
// 9. Memory integrity verification — checksums
// ============================================================================

#[test]
fn checksummed_file_survives_close_and_reopen() {
    let path = unique_path("checksum-reopen");
    let sid = SessionId::from_uuid(Uuid::nil());

    {
        let store = FileBackedMemoryStore::try_new(&path).unwrap();
        store
            .put(make_record("ck1", MemoryScope::Decision, &sid))
            .unwrap();
        store
            .put(make_record("ck2", MemoryScope::Project, &sid))
            .unwrap();
    }

    let store2 = FileBackedMemoryStore::try_new(&path).unwrap();
    let all = store2.list_all().unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].key, "ck1");
    assert_eq!(all[1].key, "ck2");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn corrupted_checksum_returns_integrity_error() {
    use crate::error::SimardError;

    let path = unique_path("corrupt-cksum");
    let sid = SessionId::from_uuid(Uuid::nil());

    {
        let store = FileBackedMemoryStore::try_new(&path).unwrap();
        store
            .put(make_record("c1", MemoryScope::Decision, &sid))
            .unwrap();
    }

    // Tamper with the stored CRC32 value.
    let contents = std::fs::read_to_string(&path).unwrap();
    let mut parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
    parsed["crc32"] = serde_json::Value::from(0xDEADBEEFu64);
    std::fs::write(&path, serde_json::to_string(&parsed).unwrap()).unwrap();

    let result = FileBackedMemoryStore::try_new(&path);
    assert!(result.is_err(), "should fail with corrupted checksum");
    let err = result.unwrap_err();
    assert!(
        matches!(err, SimardError::MemoryIntegrityError { .. }),
        "expected MemoryIntegrityError, got: {err:?}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn corrupted_record_data_returns_integrity_error() {
    use crate::error::SimardError;

    let path = unique_path("corrupt-data");
    let sid = SessionId::from_uuid(Uuid::nil());

    {
        let store = FileBackedMemoryStore::try_new(&path).unwrap();
        store
            .put(make_record("d1", MemoryScope::Decision, &sid))
            .unwrap();
    }

    // Tamper with the record value while leaving the CRC32 unchanged.
    let mut parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    parsed["records"][0]["value"] = serde_json::Value::from("TAMPERED");
    std::fs::write(&path, serde_json::to_string(&parsed).unwrap()).unwrap();

    let result = FileBackedMemoryStore::try_new(&path);
    assert!(result.is_err(), "should fail with corrupted record data");
    assert!(
        matches!(
            result.unwrap_err(),
            SimardError::MemoryIntegrityError { .. }
        ),
        "expected MemoryIntegrityError"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn truncated_file_returns_error() {
    let path = unique_path("truncated");
    let sid = SessionId::from_uuid(Uuid::nil());

    {
        let store = FileBackedMemoryStore::try_new(&path).unwrap();
        store
            .put(make_record("t1", MemoryScope::Decision, &sid))
            .unwrap();
    }

    // Truncate the file mid-way.
    let contents = std::fs::read_to_string(&path).unwrap();
    let truncated = &contents[..contents.len() / 2];
    std::fs::write(&path, truncated).unwrap();

    let result = FileBackedMemoryStore::try_new(&path);
    assert!(
        result.is_err(),
        "should fail on truncated (unparseable) file"
    );

    let _ = std::fs::remove_file(&path);
}

// ============================================================================
// 10. Recall validation after consolidation cycle
// ============================================================================

#[test]
fn consolidated_memories_survive_reopen_and_are_recallable() {
    let path = unique_path("consolidation-recall");
    let session_a = SessionId::from_uuid(Uuid::from_u128(100));
    let session_b = SessionId::from_uuid(Uuid::from_u128(200));

    // Phase 1: Write memories from two sessions.
    {
        let store = FileBackedMemoryStore::try_new(&path).unwrap();
        store
            .put(make_record("insight-a", MemoryScope::Decision, &session_a))
            .unwrap();
        store
            .put(make_record("insight-b", MemoryScope::Project, &session_b))
            .unwrap();
    }

    // Phase 2: Simulate consolidation — reopen, add a summary record.
    {
        let store = FileBackedMemoryStore::try_new(&path).unwrap();
        assert_eq!(store.list_all().unwrap().len(), 2);
        store
            .put(make_record(
                "consolidated-summary",
                MemoryScope::SessionSummary,
                &session_a,
            ))
            .unwrap();
    }

    // Phase 3: Drop and reopen — all records should be recallable.
    let store = FileBackedMemoryStore::try_new(&path).unwrap();
    let all = store.list_all().unwrap();
    assert_eq!(all.len(), 3);

    // Verify recall by scope.
    assert_eq!(store.list(MemoryScope::Decision).unwrap().len(), 1);
    assert_eq!(store.list(MemoryScope::Project).unwrap().len(), 1);
    assert_eq!(store.list(MemoryScope::SessionSummary).unwrap().len(), 1);

    // Verify recall by session.
    assert_eq!(store.list_for_session(&session_a).unwrap().len(), 2);
    assert_eq!(store.list_for_session(&session_b).unwrap().len(), 1);

    // Verify time-range recall.
    let now = Utc::now();
    let results = store
        .list_by_time_range(now - Duration::minutes(1), now + Duration::minutes(1))
        .unwrap();
    assert_eq!(results.len(), 3);

    let _ = std::fs::remove_file(&path);
}

// ============================================================================
// 11. Durability — full lifecycle
// ============================================================================

#[test]
fn full_lifecycle_write_close_reopen_verify() {
    let path = unique_path("lifecycle");
    let sid = SessionId::from_uuid(Uuid::nil());

    // Write.
    {
        let store = FileBackedMemoryStore::try_new(&path).unwrap();
        for i in 0..10 {
            store
                .put(make_record(
                    &format!("item-{i}"),
                    MemoryScope::Project,
                    &sid,
                ))
                .unwrap();
        }
    }

    // Close + reopen.
    let store = FileBackedMemoryStore::try_new(&path).unwrap();
    let all = store.list_all().unwrap();
    assert_eq!(all.len(), 10);

    for (i, record) in all.iter().enumerate() {
        assert_eq!(record.key, format!("item-{i}"));
        assert_eq!(record.value, format!("value-for-item-{i}"));
        assert!(record.created_at.is_some());
    }

    let _ = std::fs::remove_file(&path);
}

#[test]
fn legacy_plain_json_file_loads_without_error() {
    let path = unique_path("legacy-compat");
    let sid = SessionId::from_uuid(Uuid::nil());

    // Write a legacy plain-array JSON file (no checksum envelope).
    let records = vec![MemoryRecord {
        key: "legacy-key".to_string(),
        scope: MemoryScope::Project,
        value: "legacy-value".to_string(),
        session_id: sid.clone(),
        recorded_in: SessionPhase::Execution,
        created_at: Some(Utc::now()),
    }];
    std::fs::write(&path, serde_json::to_string(&records).unwrap()).unwrap();

    // Should load fine via legacy format support.
    let store = FileBackedMemoryStore::try_new(&path).unwrap();
    let all = store.list_all().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].key, "legacy-key");

    // Writing should upgrade to checksummed format.
    store
        .put(make_record("new-key", MemoryScope::Decision, &sid))
        .unwrap();

    // Reopen should validate the checksum.
    let store2 = FileBackedMemoryStore::try_new(&path).unwrap();
    assert_eq!(store2.list_all().unwrap().len(), 2);

    let _ = std::fs::remove_file(&path);
}
