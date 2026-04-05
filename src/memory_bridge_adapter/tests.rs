//! Core store and hydration-from-fallback tests.

use super::store::CognitiveBridgeMemoryStore;
use super::test_helpers::{make_record, test_store};
use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::memory::{FileBackedMemoryStore, MemoryScope, MemoryStore};
use crate::memory_bridge::CognitiveMemoryBridge;
use crate::session::SessionId;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[test]
fn put_and_list_by_scope() {
    let store = test_store();
    store.put(make_record("a", MemoryScope::Decision)).unwrap();
    store.put(make_record("b", MemoryScope::Project)).unwrap();
    store.put(make_record("c", MemoryScope::Decision)).unwrap();

    let decisions = store.list(MemoryScope::Decision).unwrap();
    assert_eq!(decisions.len(), 2);
    let projects = store.list(MemoryScope::Project).unwrap();
    assert_eq!(projects.len(), 1);
}

#[test]
fn put_deduplicates_by_key() {
    let store = test_store();
    store
        .put(make_record("dup", MemoryScope::Decision))
        .unwrap();
    store
        .put(make_record("dup", MemoryScope::Decision))
        .unwrap();

    let all = store.list(MemoryScope::Decision).unwrap();
    assert_eq!(all.len(), 1);
}

#[test]
fn list_for_session_filters_correctly() {
    let store = test_store();
    store
        .put(make_record("x", MemoryScope::SessionScratch))
        .unwrap();

    let session = SessionId::from_uuid(Uuid::nil());
    let records = store.list_for_session(&session).unwrap();
    assert_eq!(records.len(), 1);

    let other = SessionId::from_uuid(Uuid::from_u128(1));
    let records = store.list_for_session(&other).unwrap();
    assert_eq!(records.len(), 0);
}

#[test]
fn count_for_session() {
    let store = test_store();
    store.put(make_record("p", MemoryScope::Benchmark)).unwrap();
    store.put(make_record("q", MemoryScope::Benchmark)).unwrap();

    let session = SessionId::from_uuid(Uuid::nil());
    assert_eq!(store.count_for_session(&session).unwrap(), 2);
}

#[test]
fn descriptor_identifies_cognitive_bridge() {
    let store = test_store();
    let desc = store.descriptor();
    assert!(desc.identity.contains("cognitive-bridge"));
}

#[test]
fn hydration_loads_records_from_fallback() {
    // Step 1: create a fallback file with pre-existing records.
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("hydrate-test-{unique}.json"));

    // Seed the fallback with records via a standalone FileBackedMemoryStore.
    {
        let seed = FileBackedMemoryStore::try_new(&path).unwrap();
        seed.put(make_record("prior-a", MemoryScope::Decision))
            .unwrap();
        seed.put(make_record("prior-b", MemoryScope::Project))
            .unwrap();
    }

    // Step 2: create a CognitiveBridgeMemoryStore that reads the same path.
    let transport = InMemoryBridgeTransport::new("test-hydrate", |method, _params| match method {
        "memory.store_fact" => Ok(json!({"id": "sem_hydrate"})),
        "memory.search_facts" => Ok(json!({"facts": []})),
        _ => Err(crate::bridge::BridgeErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    let bridge = CognitiveMemoryBridge::new(Box::new(transport));
    let store = CognitiveBridgeMemoryStore::new(bridge, &path).unwrap();

    // Step 3: verify hydration — records visible without any put().
    let decisions = store.list(MemoryScope::Decision).unwrap();
    assert_eq!(decisions.len(), 1, "decision record should be hydrated");
    assert_eq!(decisions[0].key, "prior-a");

    let projects = store.list(MemoryScope::Project).unwrap();
    assert_eq!(projects.len(), 1, "project record should be hydrated");
    assert_eq!(projects[0].key, "prior-b");

    // Clean up.
    let _ = std::fs::remove_file(&path);
}

#[test]
fn hydration_with_empty_fallback_starts_empty() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("hydrate-empty-{unique}.json"));

    let transport =
        InMemoryBridgeTransport::new("test-hydrate-empty", |method, _params| match method {
            "memory.store_fact" => Ok(json!({"id": "sem_empty"})),
            "memory.search_facts" => Ok(json!({"facts": []})),
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    let bridge = CognitiveMemoryBridge::new(Box::new(transport));
    let store = CognitiveBridgeMemoryStore::new(bridge, &path).unwrap();

    // No records should exist — hydration from empty fallback is a no-op.
    for scope in [
        MemoryScope::SessionScratch,
        MemoryScope::SessionSummary,
        MemoryScope::Decision,
        MemoryScope::Project,
        MemoryScope::Benchmark,
    ] {
        assert!(
            store.list(scope).unwrap().is_empty(),
            "scope {scope:?} should be empty after hydrating empty fallback"
        );
    }

    let _ = std::fs::remove_file(&path);
}

#[test]
fn hydration_plus_new_put_merge_correctly() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("hydrate-merge-{unique}.json"));

    // Seed with one record.
    {
        let seed = FileBackedMemoryStore::try_new(&path).unwrap();
        seed.put(make_record("old-key", MemoryScope::Decision))
            .unwrap();
    }

    let transport = InMemoryBridgeTransport::new("test-merge", |method, _params| match method {
        "memory.store_fact" => Ok(json!({"id": "sem_merge"})),
        "memory.search_facts" => Ok(json!({"facts": []})),
        _ => Err(crate::bridge::BridgeErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    let bridge = CognitiveMemoryBridge::new(Box::new(transport));
    let store = CognitiveBridgeMemoryStore::new(bridge, &path).unwrap();

    // Add a new record via put.
    store
        .put(make_record("new-key", MemoryScope::Decision))
        .unwrap();

    // Both old (hydrated) and new should be visible.
    let decisions = store.list(MemoryScope::Decision).unwrap();
    assert_eq!(decisions.len(), 2);
    let keys: Vec<&str> = decisions.iter().map(|r| r.key.as_str()).collect();
    assert!(keys.contains(&"old-key"));
    assert!(keys.contains(&"new-key"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn list_for_session_includes_hydrated_records() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("hydrate-session-{unique}.json"));

    {
        let seed = FileBackedMemoryStore::try_new(&path).unwrap();
        seed.put(make_record("sess-rec", MemoryScope::SessionScratch))
            .unwrap();
    }

    let transport = InMemoryBridgeTransport::new("test-session", |method, _params| match method {
        "memory.store_fact" => Ok(json!({"id": "sem_sess"})),
        "memory.search_facts" => Ok(json!({"facts": []})),
        _ => Err(crate::bridge::BridgeErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    let bridge = CognitiveMemoryBridge::new(Box::new(transport));
    let store = CognitiveBridgeMemoryStore::new(bridge, &path).unwrap();

    let session = SessionId::from_uuid(Uuid::nil());
    let records = store.list_for_session(&session).unwrap();
    assert_eq!(
        records.len(),
        1,
        "hydrated records should be visible via list_for_session"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn local_read_hits_without_bridge() {
    // When local index has records, list() returns them without calling bridge.
    let store = test_store();
    store
        .put(make_record("local-hit", MemoryScope::Decision))
        .unwrap();

    let results = store.list(MemoryScope::Decision).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "local-hit");
}

#[test]
fn write_through_updates_local_cache() {
    // After put(), the record should be immediately visible via list()
    // without needing a bridge call.
    let store = test_store();

    assert!(store.list(MemoryScope::Benchmark).unwrap().is_empty());
    store
        .put(make_record("cached", MemoryScope::Benchmark))
        .unwrap();
    let results = store.list(MemoryScope::Benchmark).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "cached");
}
