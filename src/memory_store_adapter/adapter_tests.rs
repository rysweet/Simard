//! Tests for adapter interaction: adapter queries, retry logic, and adapter hydration.

use super::store::CognitiveMemoryStoreAdapter;
use super::test_helpers::make_record;
use crate::memory::{MemoryScope, MemoryStore};
use crate::memory_adapter::CognitiveMemoryAdapter;
use crate::server_subprocess::InMemoryServerTransport;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[test]
fn local_miss_triggers_adapter_query() {
    // When local index is empty for a scope, list() queries the adapter.
    let sid = Uuid::nil();
    let transport =
        InMemoryServerTransport::new("test-adapter-query", move |method, _params| match method {
            "memory.store_fact" => Ok(json!({"id": "sem_adapter"})),
            "memory.search_facts" => Ok(json!({
                "facts": [{
                    "node_id": "n1",
                    "concept": "adapter-fact",
                    "content": "from-adapter",
                    "confidence": 1.0,
                    "source_id": "test",
                    "tags": [
                        format!("scope:Decision"),
                        format!("session:{sid}")
                    ]
                }]
            })),
            _ => Err(crate::server_transport::ServerErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    let adapter = CognitiveMemoryAdapter::new(Box::new(transport));
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("adapter-test-{unique}.json"));
    let store = CognitiveMemoryStoreAdapter::new(adapter, path.clone()).unwrap();

    // No local records in Decision scope — should query adapter for remote records.
    let results = store.list(MemoryScope::Decision).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "adapter-fact");
    assert_eq!(results[0].value, "from-adapter");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn adapter_timeout_triggers_retry() {
    // Adapter fails on first call, succeeds on second.
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();
    let transport = InMemoryServerTransport::new("test-retry", move |method, _params| {
        match method {
            "memory.store_fact" => Ok(json!({"id": "sem_retry"})),
            "memory.search_facts" => {
                let count = cc.fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    // First call fails.
                    Err(crate::server_transport::ServerErrorPayload {
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
            _ => Err(crate::server_transport::ServerErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        }
    });
    let adapter = CognitiveMemoryAdapter::new(Box::new(transport));
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("retry-test-{unique}.json"));
    let store = CognitiveMemoryStoreAdapter::new(adapter, path.clone()).unwrap();

    // list() for empty scope should trigger adapter query with retry.
    let results = store.list(MemoryScope::Project).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "retried-fact");
    // Two calls should have been made (initial + 1 retry).
    assert_eq!(call_count.load(Ordering::SeqCst), 2);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn hydrate_from_adapter_merges_new_records() {
    let transport =
        InMemoryServerTransport::new("test-adapter-hydrate", |method, _params| match method {
            "memory.store_fact" => Ok(json!({"id": "sem_bh"})),
            "memory.search_facts" => Ok(json!({
                "facts": [{
                    "node_id": "n3",
                    "concept": "adapter-only",
                    "content": "from-adapter-hydrate",
                    "confidence": 1.0,
                    "source_id": "memory-store-adapter",
                    "tags": ["scope:Project", "session:00000000-0000-0000-0000-000000000000"]
                }]
            })),
            _ => Err(crate::server_transport::ServerErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    let adapter = CognitiveMemoryAdapter::new(Box::new(transport));
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("adapter-hydrate-{unique}.json"));
    let store = CognitiveMemoryStoreAdapter::new(adapter, path.clone()).unwrap();

    // Before hydration — local index should be empty.
    assert!(store.records.lock().unwrap().is_empty());

    // Hydrate from adapter.
    store.hydrate_from_adapter().unwrap();

    // Adapter record should now be in local index.
    let records = store.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert!(records.contains_key("adapter-only"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn hydrate_from_adapter_does_not_overwrite_local() {
    let transport =
        InMemoryServerTransport::new("test-no-overwrite", |method, _params| match method {
            "memory.store_fact" => Ok(json!({"id": "sem_no"})),
            "memory.search_facts" => Ok(json!({
                "facts": [{
                    "node_id": "n4",
                    "concept": "shared-key",
                    "content": "adapter-version",
                    "confidence": 1.0,
                    "source_id": "memory-store-adapter",
                    "tags": ["scope:Decision", "session:00000000-0000-0000-0000-000000000000"]
                }]
            })),
            _ => Err(crate::server_transport::ServerErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    let adapter = CognitiveMemoryAdapter::new(Box::new(transport));
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("no-overwrite-{unique}.json"));
    let store = CognitiveMemoryStoreAdapter::new(adapter, path.clone()).unwrap();

    // Put a local record with the same key.
    store
        .put(make_record("shared-key", MemoryScope::Decision))
        .unwrap();

    // Hydrate — should NOT overwrite the local version.
    store.hydrate_from_adapter().unwrap();

    let records = store.records.lock().unwrap();
    let rec = records.get("shared-key").unwrap();
    assert_eq!(
        rec.value, "value-for-shared-key",
        "local version should be preserved over adapter version"
    );

    let _ = std::fs::remove_file(&path);
}
