//! Shared test helpers for the `memory_bridge_adapter` module.

use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::memory::{MemoryRecord, MemoryScope};
use crate::memory_bridge::CognitiveMemoryBridge;
use crate::session::{SessionId, SessionPhase};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use super::store::CognitiveBridgeMemoryStore;

pub(super) fn test_store() -> CognitiveBridgeMemoryStore {
    let transport =
        InMemoryBridgeTransport::new("test-adapter", |method, _params| match method {
            "memory.store_fact" => Ok(json!({"id": "sem_adapter_test"})),
            "memory.search_facts" => Ok(json!({"facts": []})),
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
    let path = std::env::temp_dir().join(format!("adapter-test-{unique}.json"));
    CognitiveBridgeMemoryStore::new(bridge, path).unwrap()
}

pub(super) fn make_record(key: &str, scope: MemoryScope) -> MemoryRecord {
    MemoryRecord {
        key: key.to_string(),
        scope,
        value: format!("value-for-{key}"),
        session_id: SessionId::from_uuid(Uuid::nil()),
        recorded_in: SessionPhase::Execution,
    }
}
