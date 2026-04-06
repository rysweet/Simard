//! Core store and hydration-from-fallback tests.

use super::store::CognitiveBridgeMemoryStore;
use super::test_helpers::{make_record, test_store};
use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::memory::{CognitiveMemoryType, FileBackedMemoryStore, MemoryStore};
use crate::memory_bridge::CognitiveMemoryBridge;
use crate::session::SessionId;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[test]
fn put_and_list_by_scope() {
    let store = test_store();
    store
        .put(make_record("a", CognitiveMemoryType::Semantic))
        .unwrap();
    store
        .put(make_record("b", CognitiveMemoryType::Semantic))
        .unwrap();
    store
        .put(make_record("c", CognitiveMemoryType::Episodic))
        .unwrap();

    let semantic = store.list(CognitiveMemoryType::Semantic).unwrap();
    assert_eq!(semantic.len(), 2);
    let episodic = store.list(CognitiveMemoryType::Episodic).unwrap();
    assert_eq!(episodic.len(), 1);
}

#[test]
fn put_deduplicates_by_key() {
    let store = test_store();
    store
        .put(make_record("dup", CognitiveMemoryType::Semantic))
        .unwrap();
    store
        .put(make_record("dup", CognitiveMemoryType::Semantic))
        .unwrap();

    let all = store.list(CognitiveMemoryType::Semantic).unwrap();
    assert_eq!(all.len(), 1);
}

#[test]
fn list_for_session_filters_correctly() {
    let store = test_store();
    store
        .put(make_record("x", CognitiveMemoryType::Working))
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
    store
        .put(make_record("p", CognitiveMemoryType::Procedural))
        .unwrap();
    store
        .put(make_record("q", CognitiveMemoryType::Procedural))
        .unwrap();

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
        seed.put(make_record("prior-a", CognitiveMemoryType::Semantic))
            .unwrap();
        seed.put(make_record("prior-b", CognitiveMemoryType::Episodic))
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
    let semantic = store.list(CognitiveMemoryType::Semantic).unwrap();
    assert_eq!(semantic.len(), 1, "semantic record should be hydrated");
    assert_eq!(semantic[0].key, "prior-a");

    let episodic = store.list(CognitiveMemoryType::Episodic).unwrap();
    assert_eq!(episodic.len(), 1, "episodic record should be hydrated");
    assert_eq!(episodic[0].key, "prior-b");

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
    for mt in [
        CognitiveMemoryType::Sensory,
        CognitiveMemoryType::Working,
        CognitiveMemoryType::Episodic,
        CognitiveMemoryType::Semantic,
        CognitiveMemoryType::Procedural,
        CognitiveMemoryType::Prospective,
    ] {
        assert!(
            store.list(mt).unwrap().is_empty(),
            "type {mt:?} should be empty after hydrating empty fallback"
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
        seed.put(make_record("old-key", CognitiveMemoryType::Semantic))
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
        .put(make_record("new-key", CognitiveMemoryType::Semantic))
        .unwrap();

    // Both old (hydrated) and new should be visible.
    let decisions = store.list(CognitiveMemoryType::Semantic).unwrap();
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
        seed.put(make_record("sess-rec", CognitiveMemoryType::Working))
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
        .put(make_record("local-hit", CognitiveMemoryType::Semantic))
        .unwrap();

    let results = store.list(CognitiveMemoryType::Semantic).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "local-hit");
}

#[test]
fn write_through_updates_local_cache() {
    // After put(), the record should be immediately visible via list()
    // without needing a bridge call.
    let store = test_store();

    assert!(
        store
            .list(CognitiveMemoryType::Procedural)
            .unwrap()
            .is_empty()
    );
    store
        .put(make_record("cached", CognitiveMemoryType::Procedural))
        .unwrap();
    let results = store.list(CognitiveMemoryType::Procedural).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "cached");
}
