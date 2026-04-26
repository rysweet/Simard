use super::test_helpers::{make_record, test_store};
use crate::memory::{MemoryScope, MemoryStore};

#[test]
fn put_and_list_round_trip() {
    let store = test_store();
    let record = make_record("key1", MemoryScope::Project);
    store.put(record.clone()).unwrap();
    let listed = store.list(MemoryScope::Project).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].key, "key1");
}

#[test]
fn put_sets_created_at_when_missing() {
    let store = test_store();
    let record = make_record("key-ts", MemoryScope::Decision);
    assert!(record.created_at.is_none());
    store.put(record.clone()).unwrap();
    let all = store.list_all().unwrap();
    let found = all.iter().find(|r| r.key == "key-ts").unwrap();
    assert!(found.created_at.is_some());
}

#[test]
fn list_for_session_filters_correctly() {
    let store = test_store();
    let record = make_record("sess-key", MemoryScope::SessionScratch);
    let session_id = record.session_id.clone();
    store.put(record).unwrap();
    let result = store.list_for_session(&session_id).unwrap();
    assert_eq!(result.len(), 1);
}

#[test]
fn count_for_session_matches_list() {
    let store = test_store();
    let record = make_record("cnt-key", MemoryScope::Benchmark);
    let session_id = record.session_id.clone();
    store.put(record).unwrap();
    let count = store.count_for_session(&session_id).unwrap();
    let list = store.list_for_session(&session_id).unwrap();
    assert_eq!(count, list.len());
}

#[test]
fn list_all_includes_all_scopes() {
    let store = test_store();
    store.put(make_record("a", MemoryScope::Project)).unwrap();
    store.put(make_record("b", MemoryScope::Decision)).unwrap();
    let all = store.list_all().unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn sync_pending_returns_zero_when_empty() {
    let store = test_store();
    assert_eq!(store.sync_pending(), 0);
}

#[test]
fn descriptor_has_correct_identity() {
    let store = test_store();
    let desc = store.descriptor();
    assert!(desc.identity.contains("cognitive-bridge"));
}
