//! Adapter that implements [`MemoryStore`] by delegating to [`CognitiveMemoryBridge`].
//!
//! This bridges the gap between the simple key-value `MemoryStore` trait (used
//! by `RuntimePorts`) and the six-type cognitive memory system backed by Kuzu.
//! Each `MemoryRecord` is stored as a semantic fact in the cognitive graph, with
//! the record key as concept and scope+session encoded in tags.
//!
//! When the cognitive bridge is unavailable (honest degradation), the adapter
//! falls back to a `FileBackedMemoryStore` so the runtime always functions.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::{SimardError, SimardResult};
use crate::memory::{FileBackedMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
use crate::memory_bridge::CognitiveMemoryBridge;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::session::SessionId;

const STORE_NAME: &str = "cognitive-bridge-memory";

/// Tag prefix used to encode scope in cognitive memory tags.
fn scope_tag(scope: MemoryScope) -> String {
    format!("scope:{scope:?}")
}

/// Tag prefix used to encode session ID in cognitive memory tags.
fn session_tag(session_id: &SessionId) -> String {
    format!("session:{}", session_id.as_str())
}

/// `MemoryStore` implementation backed by cognitive memory via Python bridge.
///
/// Stores each `MemoryRecord` as a semantic fact:
/// - concept = record key
/// - content = record value
/// - confidence = 1.0 (internal metadata, always trusted)
/// - tags = [scope tag, session tag, phase tag]
/// - source_id = "memory-store-adapter"
///
/// Falls back to `FileBackedMemoryStore` if the bridge fails.
pub struct CognitiveBridgeMemoryStore {
    bridge: CognitiveMemoryBridge,
    fallback: FileBackedMemoryStore,
    /// Track records locally for list/count operations since cognitive memory
    /// search is keyword-based and cannot filter by exact scope/session.
    /// Keyed by record key for O(1) dedup on put.
    records: Mutex<HashMap<String, MemoryRecord>>,
    descriptor: BackendDescriptor,
}

impl CognitiveBridgeMemoryStore {
    pub fn new(
        bridge: CognitiveMemoryBridge,
        fallback_path: impl Into<PathBuf>,
    ) -> SimardResult<Self> {
        let fallback = FileBackedMemoryStore::try_new(fallback_path)?;
        let mut store = Self {
            bridge,
            records: Mutex::new(HashMap::new()),
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "memory::cognitive-bridge",
                "runtime-port:memory-store:cognitive-bridge",
                Freshness::now()?,
            ),
            fallback,
        };
        store.hydrate_from_fallback();
        Ok(store)
    }

    /// Populate the in-memory index from the file-backed fallback store so that
    /// records persisted in prior sessions are visible after restart. Each scope
    /// is loaded independently — failures in one scope do not prevent others
    /// from hydrating (Pillar 11).
    fn hydrate_from_fallback(&mut self) {
        use crate::memory::MemoryScope;

        const ALL_SCOPES: [MemoryScope; 5] = [
            MemoryScope::SessionScratch,
            MemoryScope::SessionSummary,
            MemoryScope::Decision,
            MemoryScope::Project,
            MemoryScope::Benchmark,
        ];

        let mut hydrated = 0usize;
        for scope in ALL_SCOPES {
            match self.fallback.list(scope) {
                Ok(records) => {
                    // Lock is safe here — called only during construction, no
                    // contention possible.
                    if let Ok(mut map) = self.records.lock() {
                        for record in records {
                            map.insert(record.key.clone(), record);
                            hydrated += 1;
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "[simard] cognitive-bridge hydration: \
                         failed to load scope {scope:?}: {e}"
                    );
                }
            }
        }
        if hydrated > 0 {
            eprintln!("[simard] cognitive-bridge: hydrated {hydrated} records from fallback");
        }
    }

    /// Store a record in cognitive memory as a semantic fact.
    fn store_as_fact(&self, record: &MemoryRecord) -> SimardResult<String> {
        let tags = vec![
            scope_tag(record.scope),
            session_tag(&record.session_id),
            format!("phase:{:?}", record.recorded_in),
        ];
        self.bridge.store_fact(
            &record.key,
            &record.value,
            1.0,
            &tags,
            "memory-store-adapter",
        )
    }
}

impl MemoryStore for CognitiveBridgeMemoryStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn put(&self, record: MemoryRecord) -> SimardResult<()> {
        // Always persist to file fallback for handoff/recovery.
        self.fallback.put(record.clone())?;

        // Store in cognitive bridge — propagate errors instead of silently
        // falling back (Pillar 11: no silent memory fallbacks).
        self.store_as_fact(&record)?;

        // Maintain local index for list/count — O(1) insert/overwrite via HashMap.
        let mut records = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: STORE_NAME.to_string(),
            })?;
        records.insert(record.key.clone(), record);
        Ok(())
    }

    fn list(&self, scope: MemoryScope) -> SimardResult<Vec<MemoryRecord>> {
        let records = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: STORE_NAME.to_string(),
            })?;
        Ok(records
            .values()
            .filter(|r| r.scope == scope)
            .cloned()
            .collect())
    }

    fn list_for_session(&self, session_id: &SessionId) -> SimardResult<Vec<MemoryRecord>> {
        let records = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: STORE_NAME.to_string(),
            })?;
        Ok(records
            .values()
            .filter(|r| &r.session_id == session_id)
            .cloned()
            .collect())
    }

    fn count_for_session(&self, session_id: &SessionId) -> SimardResult<usize> {
        let records = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: STORE_NAME.to_string(),
            })?;
        Ok(records
            .values()
            .filter(|r| &r.session_id == session_id)
            .count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::session::SessionPhase;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};
    use uuid::Uuid;

    fn test_store() -> CognitiveBridgeMemoryStore {
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

    fn make_record(key: &str, scope: MemoryScope) -> MemoryRecord {
        MemoryRecord {
            key: key.to_string(),
            scope,
            value: format!("value-for-{key}"),
            session_id: SessionId::from_uuid(Uuid::nil()),
            recorded_in: SessionPhase::Execution,
        }
    }

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
        let transport =
            InMemoryBridgeTransport::new("test-hydrate", |method, _params| match method {
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

        let transport =
            InMemoryBridgeTransport::new("test-merge", |method, _params| match method {
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

        let transport =
            InMemoryBridgeTransport::new("test-session", |method, _params| match method {
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
}
