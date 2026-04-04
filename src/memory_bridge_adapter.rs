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
use crate::memory_cognitive::CognitiveFact;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::session::{SessionId, SessionPhase};

const STORE_NAME: &str = "cognitive-bridge-memory";

/// Maximum retries for bridge read operations.
const BRIDGE_READ_MAX_RETRIES: usize = 1;

/// Backoff between bridge retries in milliseconds.
const BRIDGE_RETRY_BACKOFF_MS: u64 = 200;

/// Tag prefix used to encode scope in cognitive memory tags.
fn scope_tag(scope: MemoryScope) -> String {
    format!("scope:{scope:?}")
}

/// Tag prefix used to encode session ID in cognitive memory tags.
fn session_tag(session_id: &SessionId) -> String {
    format!("session:{}", session_id.as_str())
}

/// Parse a scope from a tag string like "scope:Decision".
fn parse_scope_tag(tag: &str) -> Option<MemoryScope> {
    let suffix = tag.strip_prefix("scope:")?;
    match suffix {
        "SessionScratch" => Some(MemoryScope::SessionScratch),
        "SessionSummary" => Some(MemoryScope::SessionSummary),
        "Decision" => Some(MemoryScope::Decision),
        "Project" => Some(MemoryScope::Project),
        "Benchmark" => Some(MemoryScope::Benchmark),
        _ => None,
    }
}

/// Parse a session ID from a tag string like "session:<uuid>".
fn parse_session_tag(tag: &str) -> Option<SessionId> {
    let suffix = tag.strip_prefix("session:")?;
    uuid::Uuid::parse_str(suffix).ok().map(SessionId::from_uuid)
}

/// Convert a `CognitiveFact` back to a `MemoryRecord` by parsing encoded tags.
fn fact_to_record(fact: &CognitiveFact) -> MemoryRecord {
    let scope = fact
        .tags
        .iter()
        .find_map(|t| parse_scope_tag(t))
        .unwrap_or(MemoryScope::Project);
    let session_id = fact
        .tags
        .iter()
        .find_map(|t| parse_session_tag(t))
        .unwrap_or_else(|| SessionId::from_uuid(uuid::Uuid::nil()));
    MemoryRecord {
        key: fact.concept.clone(),
        scope,
        value: fact.content.clone(),
        session_id,
        recorded_in: SessionPhase::Execution,
    }
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

    /// Pull facts from the cognitive bridge (Python subprocess) and merge into
    /// the local in-memory index. This supplements fallback hydration by
    /// recovering records that were persisted to the graph but not yet in the
    /// local JSON file (e.g., written by another Simard process).
    ///
    /// Uses retry logic: if the bridge returns an error, retry once with
    /// backoff before giving up (Pillar 11: honest degradation).
    pub fn hydrate_from_bridge(&self) {
        let facts = self.search_facts_with_retry("memory-store-adapter", 500, 0.0);
        let facts = match facts {
            Ok(f) => f,
            Err(e) => {
                eprintln!("[simard] cognitive-bridge: bridge hydration failed: {e}");
                return;
            }
        };
        if facts.is_empty() {
            return;
        }
        let mut hydrated = 0usize;
        if let Ok(mut map) = self.records.lock() {
            for fact in &facts {
                let record = fact_to_record(fact);
                // Only insert if not already present — local data is fresher.
                if !map.contains_key(&record.key) {
                    map.insert(record.key.clone(), record);
                    hydrated += 1;
                }
            }
        }
        if hydrated > 0 {
            eprintln!("[simard] cognitive-bridge: hydrated {hydrated} records from bridge");
        }
    }

    /// Search facts via the cognitive bridge with retry logic.
    fn search_facts_with_retry(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
    ) -> SimardResult<Vec<CognitiveFact>> {
        let mut last_err = None;
        for attempt in 0..=BRIDGE_READ_MAX_RETRIES {
            match self.bridge.search_facts(query, limit, min_confidence) {
                Ok(facts) => return Ok(facts),
                Err(e) => {
                    if attempt < BRIDGE_READ_MAX_RETRIES {
                        eprintln!(
                            "[simard] cognitive-bridge: search_facts retry {}/{} after error: {e}",
                            attempt + 1,
                            BRIDGE_READ_MAX_RETRIES
                        );
                        std::thread::sleep(std::time::Duration::from_millis(
                            BRIDGE_RETRY_BACKOFF_MS,
                        ));
                    }
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap())
    }

    /// Query the bridge for records matching a scope, converting facts back to
    /// `MemoryRecord`s. Used as fallback when the local index has no results.
    fn bridge_fallback_list(&self, scope: MemoryScope) -> Vec<MemoryRecord> {
        let query = format!("scope:{scope:?}");
        match self.search_facts_with_retry(&query, 200, 0.0) {
            Ok(facts) => {
                let records: Vec<MemoryRecord> = facts
                    .iter()
                    .map(fact_to_record)
                    .filter(|r| r.scope == scope)
                    .collect();
                // Merge into local index for future reads.
                if !records.is_empty()
                    && let Ok(mut map) = self.records.lock()
                {
                    for record in &records {
                        map.entry(record.key.clone())
                            .or_insert_with(|| record.clone());
                    }
                }
                records
            }
            Err(e) => {
                eprintln!(
                    "[simard] cognitive-bridge: bridge fallback for scope {scope:?} failed: {e}"
                );
                Vec::new()
            }
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
        let local: Vec<MemoryRecord> = records
            .values()
            .filter(|r| r.scope == scope)
            .cloned()
            .collect();
        if !local.is_empty() {
            return Ok(local);
        }
        // Local miss — try bridge fallback to recover cross-session data.
        drop(records); // release lock before bridge call
        let bridged = self.bridge_fallback_list(scope);
        Ok(bridged)
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

    #[test]
    fn fact_to_record_parses_tags_correctly() {
        let fact = CognitiveFact {
            node_id: "n1".to_string(),
            concept: "test-concept".to_string(),
            content: "test-content".to_string(),
            confidence: 0.9,
            source_id: "test".to_string(),
            tags: vec![
                "scope:Benchmark".to_string(),
                "session:00000000-0000-0000-0000-000000000001".to_string(),
            ],
        };
        let record = fact_to_record(&fact);
        assert_eq!(record.key, "test-concept");
        assert_eq!(record.value, "test-content");
        assert_eq!(record.scope, MemoryScope::Benchmark);
    }

    #[test]
    fn fact_to_record_defaults_on_missing_tags() {
        let fact = CognitiveFact {
            node_id: "n2".to_string(),
            concept: "no-tags".to_string(),
            content: "content".to_string(),
            confidence: 0.5,
            source_id: "test".to_string(),
            tags: vec![],
        };
        let record = fact_to_record(&fact);
        assert_eq!(record.scope, MemoryScope::Project); // default
    }
}
