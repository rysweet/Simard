//! Tests for bridge interaction: fallback queries, retry logic, and bridge hydration.

use super::store::CognitiveBridgeMemoryStore;
use super::test_helpers::make_record;
use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::memory::{MemoryScope, MemoryStore};
use crate::memory_bridge::CognitiveMemoryBridge;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[test]
fn local_miss_triggers_bridge_fallback() {
    // When local index is empty for a scope, list() queries the bridge.
    let sid = Uuid::nil();
    let transport =
        InMemoryBridgeTransport::new("test-fallback", move |method, _params| match method {
            "memory.store_fact" => Ok(json!({"id": "sem_fallback"})),
            "memory.search_facts" => Ok(json!({
                "facts": [{
                    "node_id": "n1",
                    "concept": "bridge-fact",
                    "content": "from-bridge",
                    "confidence": 1.0,
                    "source_id": "test",
                    "tags": [
                        format!("scope:Decision"),
                        format!("session:{sid}")
                    ]
                }]
            })),
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    let bridge = CognitiveMemoryBridge::new(Box::new(transport));
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("fallback-test-{unique}.json"));
    let store = CognitiveBridgeMemoryStore::new(bridge, path.clone()).unwrap();

    // No local records in Decision scope — should fall back to bridge.
    let results = store.list(MemoryScope::Decision).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "bridge-fact");
    assert_eq!(results[0].value, "from-bridge");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn bridge_timeout_triggers_retry() {
    // Bridge fails on first call, succeeds on second.
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();
    let transport = InMemoryBridgeTransport::new("test-retry", move |method, _params| {
        match method {
            "memory.store_fact" => Ok(json!({"id": "sem_retry"})),
            "memory.search_facts" => {
                let count = cc.fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    // First call fails.
                    Err(crate::bridge::BridgeErrorPayload {
                        code: -32000,
                        message: "timeout".to_string(),
                    })
                } else {
                    // Retry succeeds.
                    Ok(json!({"facts": [{
                        "node_id": "n2",
                        "concept": "retried-fact",
                        "content": "after-retry",
                        "confidence": 1.0,
                        "source_id": "test",
                        "tags": ["scope:Project", "session:00000000-0000-0000-0000-000000000000"]
                    }]}))
                }
            }
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        }
    });
    let bridge = CognitiveMemoryBridge::new(Box::new(transport));
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("retry-test-{unique}.json"));
    let store = CognitiveBridgeMemoryStore::new(bridge, path.clone()).unwrap();

    // list() for empty scope should trigger bridge fallback with retry.
    let results = store.list(MemoryScope::Project).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "retried-fact");
    // Two calls should have been made (initial + 1 retry).
    assert_eq!(call_count.load(Ordering::SeqCst), 2);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn hydrate_from_bridge_merges_new_records() {
    let transport =
        InMemoryBridgeTransport::new("test-bridge-hydrate", |method, _params| match method {
            "memory.store_fact" => Ok(json!({"id": "sem_bh"})),
            "memory.search_facts" => Ok(json!({
                "facts": [{
                    "node_id": "n3",
                    "concept": "bridge-only",
                    "content": "from-bridge-hydrate",
                    "confidence": 1.0,
                    "source_id": "memory-store-adapter",
                    "tags": ["scope:Project", "session:00000000-0000-0000-0000-000000000000"]
                }]
            })),
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    let bridge = CognitiveMemoryBridge::new(Box::new(transport));
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("bridge-hydrate-{unique}.json"));
    let store = CognitiveBridgeMemoryStore::new(bridge, path.clone()).unwrap();

    // Before hydration — local index should be empty.
    assert!(store.records.lock().unwrap().is_empty());

    // Hydrate from bridge.
    store.hydrate_from_bridge();

    // Bridge record should now be in local index.
    let records = store.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert!(records.contains_key("bridge-only"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn hydrate_from_bridge_does_not_overwrite_local() {
    let transport =
        InMemoryBridgeTransport::new("test-no-overwrite", |method, _params| match method {
            "memory.store_fact" => Ok(json!({"id": "sem_no"})),
            "memory.search_facts" => Ok(json!({
                "facts": [{
                    "node_id": "n4",
                    "concept": "shared-key",
                    "content": "bridge-version",
                    "confidence": 1.0,
                    "source_id": "memory-store-adapter",
                    "tags": ["scope:Decision", "session:00000000-0000-0000-0000-000000000000"]
                }]
            })),
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    let bridge = CognitiveMemoryBridge::new(Box::new(transport));
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("no-overwrite-{unique}.json"));
    let store = CognitiveBridgeMemoryStore::new(bridge, path.clone()).unwrap();

    // Put a local record with the same key.
    store
        .put(make_record("shared-key", MemoryScope::Decision))
        .unwrap();

    // Hydrate — should NOT overwrite the local version.
    store.hydrate_from_bridge();

    let records = store.records.lock().unwrap();
    let rec = records.get("shared-key").unwrap();
    assert_eq!(
        rec.value, "value-for-shared-key",
        "local version should be preserved over bridge version"
    );

    let _ = std::fs::remove_file(&path);
}
