//! [`CognitiveBridgeMemoryStore`] — the bridge between `MemoryStore` and cognitive memory.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::memory::{FileBackedMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
use crate::memory_cognitive::CognitiveFact;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::session::SessionId;

use super::convert::{fact_to_record, scope_tag, session_tag};
use super::{BRIDGE_READ_MAX_RETRIES, BRIDGE_RETRY_BACKOFF_MS, STORE_NAME};

/// `MemoryStore` implementation backed by cognitive memory via Python bridge.
///
/// Stores each `MemoryRecord` as a semantic fact:
/// - concept = record key
/// - content = record value
/// - confidence = 1.0 (internal metadata, always trusted)
/// - tags = [scope tag, session tag, phase tag]
/// - source_id = "memory-store-adapter"
///
/// Dual-writes to both the cognitive bridge and a local `FileBackedMemoryStore` for persistence.
pub struct CognitiveBridgeMemoryStore {
    bridge: Box<dyn CognitiveMemoryOps>,
    local_store: FileBackedMemoryStore,
    /// Track records locally for list/count operations since cognitive memory
    /// search is keyword-based and cannot filter by exact scope/session.
    /// Keyed by record key for O(1) dedup on put.
    pub(super) records: Mutex<HashMap<String, MemoryRecord>>,
    /// Keys whose bridge write failed — `sync_pending()` retries these.
    pending_bridge_keys: Mutex<Vec<String>>,
    descriptor: BackendDescriptor,
}

impl CognitiveBridgeMemoryStore {
    pub fn new(
        bridge: impl CognitiveMemoryOps + 'static,
        local_store_path: impl Into<PathBuf>,
    ) -> SimardResult<Self> {
        let local_store = FileBackedMemoryStore::try_new(local_store_path)?;
        let mut store = Self {
            bridge: Box::new(bridge),
            records: Mutex::new(HashMap::new()),
            pending_bridge_keys: Mutex::new(Vec::new()),
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "memory::cognitive-bridge",
                "runtime-port:memory-store:cognitive-bridge",
                Freshness::now()?,
            ),
            local_store,
        };
        store.hydrate_from_file_store()?;
        Ok(store)
    }

    /// Populate the in-memory index from the file-backed store so that
    /// records persisted in prior sessions are visible after restart.
    /// Scope load failures propagate per PHILOSOPHY.md.
    fn hydrate_from_file_store(&mut self) -> SimardResult<()> {
        use crate::memory::MemoryScope;

        const ALL_SCOPES: [MemoryScope; 6] = [
            MemoryScope::SessionScratch,
            MemoryScope::SessionSummary,
            MemoryScope::Decision,
            MemoryScope::Project,
            MemoryScope::Benchmark,
            MemoryScope::Untagged,
        ];

        let mut hydrated = 0usize;
        for scope in ALL_SCOPES {
            let records = self.local_store.list(scope)?;
            if let Ok(mut map) = self.records.lock() {
                for record in records {
                    map.insert(record.key.clone(), record);
                    hydrated += 1;
                }
            }
        }
        if hydrated > 0 {
            eprintln!("[simard] cognitive-bridge: hydrated {hydrated} records from file store");
        }
        Ok(())
    }

    /// Pull facts from the cognitive bridge (Python subprocess) and merge into
    /// the local in-memory index. This supplements file-store hydration by
    /// recovering records that were persisted to the graph but not yet in the
    /// local JSON file (e.g., written by another Simard process).
    ///
    /// Bridge errors propagate via `?` per PHILOSOPHY.md.
    pub fn hydrate_from_bridge(&self) -> SimardResult<()> {
        let facts = self.search_facts_with_retry("memory-store-adapter", 500, 0.0)?;
        if facts.is_empty() {
            return Ok(());
        }
        let mut hydrated = 0usize;
        if let Ok(mut map) = self.records.lock() {
            for fact in &facts {
                let record = fact_to_record(fact);
                if !map.contains_key(&record.key) {
                    map.insert(record.key.clone(), record);
                    hydrated += 1;
                }
            }
        }
        if hydrated > 0 {
            eprintln!("[simard] cognitive-bridge: hydrated {hydrated} records from bridge");
        }
        Ok(())
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
        Err(last_err.expect("retry loop ensures last_err is set on all-failures path"))
    }

    /// Query the bridge for records matching a scope, converting facts back to
    /// `MemoryRecord`s. Used when the local index has no results.
    fn bridge_list(&self, scope: MemoryScope) -> SimardResult<Vec<MemoryRecord>> {
        let query = format!("scope:{scope:?}");
        let facts = self.search_facts_with_retry(&query, 200, 0.0)?;
        let records: Vec<MemoryRecord> = facts
            .iter()
            .map(fact_to_record)
            .filter(|r| r.scope == scope)
            .collect();
        // Merge into local index for future reads.
        if !records.is_empty() {
            let mut map = self
                .records
                .lock()
                .map_err(|_| SimardError::StoragePoisoned {
                    store: STORE_NAME.to_string(),
                })?;
            for record in &records {
                map.entry(record.key.clone())
                    .or_insert_with(|| record.clone());
            }
        }
        Ok(records)
    }

    /// Retry bridge writes for records that were persisted to the local file
    /// store but failed to reach the cognitive bridge. Returns the number of
    /// successfully synced records.
    pub fn sync_pending(&self) -> usize {
        let keys: Vec<String> = {
            let Ok(pending) = self.pending_bridge_keys.lock() else {
                return 0;
            };
            pending.clone()
        };
        if keys.is_empty() {
            return 0;
        }

        let records_map = match self.records.lock() {
            Ok(m) => m.clone(),
            Err(_) => return 0,
        };

        let mut synced = 0usize;
        let mut still_pending = Vec::new();
        for key in &keys {
            if let Some(record) = records_map.get(key) {
                match self.store_as_fact(record) {
                    Ok(_) => synced += 1,
                    Err(e) => {
                        eprintln!(
                            "[simard] cognitive-bridge: sync_pending retry failed \
                             for key {:?}: {e}",
                            key,
                        );
                        still_pending.push(key.clone());
                    }
                }
            }
        }

        if let Ok(mut pending) = self.pending_bridge_keys.lock() {
            *pending = still_pending;
        }
        if synced > 0 {
            eprintln!("[simard] cognitive-bridge: sync_pending synced {synced} records");
        }
        synced
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
        // Stamp created_at if not already set.
        let mut record = record;
        if record.created_at.is_none() {
            record.created_at = Some(Utc::now());
        }

        // Write to cognitive bridge — failure is an error, not silently swallowed.
        self.store_as_fact(&record)?;

        // Also persist to local file store for handoff/recovery.
        self.local_store.put(record.clone())?;

        // Maintain local index for list/count — O(1) insert/overwrite via HashMap.
        let mut records = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: STORE_NAME.to_string(),
            })?;
        let key = record.key.clone();
        records.insert(key.clone(), record);
        drop(records);

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
        // Local miss — query bridge for cross-session data.
        drop(records); // release lock before bridge call
        self.bridge_list(scope)
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

    fn list_all(&self) -> SimardResult<Vec<MemoryRecord>> {
        let records = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: STORE_NAME.to_string(),
            })?;
        Ok(records.values().cloned().collect())
    }

    fn list_by_time_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> SimardResult<Vec<MemoryRecord>> {
        let records = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: STORE_NAME.to_string(),
            })?;
        Ok(records
            .values()
            .filter(|r| r.created_at.is_some_and(|t| t >= start && t < end))
            .cloned()
            .collect())
    }

    fn flush_pending(&self) -> usize {
        self.sync_pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{BridgeRequest, BridgeResponse};
    use crate::memory::{MemoryRecord, MemoryScope, MemoryStore};
    use crate::memory_bridge::CognitiveMemoryBridge;
    use crate::metadata::{BackendDescriptor, Freshness};
    use crate::session::{SessionId, SessionPhase};

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A mock bridge transport that returns configurable responses.
    struct MockTransport {
        /// Counts how many calls were made.
        call_count: AtomicUsize,
        /// If true, store_fact calls succeed; if false, they fail.
        store_succeeds: bool,
        /// If true, search_facts returns empty results; if false, returns an error.
        search_returns_empty: bool,
    }

    impl MockTransport {
        fn new_succeeding() -> Self {
            Self {
                call_count: AtomicUsize::new(0),
                store_succeeds: true,
                search_returns_empty: true,
            }
        }

        fn new_failing() -> Self {
            Self {
                call_count: AtomicUsize::new(0),
                store_succeeds: false,
                search_returns_empty: false,
            }
        }
    }

    impl crate::bridge::BridgeTransport for MockTransport {
        fn call(&self, request: BridgeRequest) -> SimardResult<BridgeResponse> {
            self.call_count.fetch_add(1, Ordering::Relaxed);
            if request.method.contains("store") {
                if self.store_succeeds {
                    Ok(BridgeResponse {
                        id: request.id,
                        result: Some(serde_json::json!({"id": "mock-node-id"})),
                        error: None,
                    })
                } else {
                    Err(SimardError::BridgeCallFailed {
                        bridge: "mock".to_string(),
                        method: request.method,
                        reason: "mock failure".to_string(),
                    })
                }
            } else if request.method.contains("search") {
                if self.search_returns_empty {
                    Ok(BridgeResponse {
                        id: request.id,
                        result: Some(serde_json::json!([])),
                        error: None,
                    })
                } else {
                    Err(SimardError::BridgeCallFailed {
                        bridge: "mock".to_string(),
                        method: request.method,
                        reason: "mock search failure".to_string(),
                    })
                }
            } else {
                Ok(BridgeResponse {
                    id: request.id,
                    result: Some(serde_json::json!(null)),
                    error: None,
                })
            }
        }

        fn descriptor(&self) -> BackendDescriptor {
            BackendDescriptor::for_runtime_type::<Self>(
                "mock-bridge",
                "test",
                Freshness::now().unwrap(),
            )
        }
    }

    fn make_store(transport: MockTransport) -> CognitiveBridgeMemoryStore {
        let dir = std::env::temp_dir().join(format!(
            "simard_test_cbs_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let bridge = CognitiveMemoryBridge::new(Box::new(transport));
        CognitiveBridgeMemoryStore::new(bridge, dir.join("local_store.json")).unwrap()
    }

    fn make_record(key: &str, scope: MemoryScope) -> MemoryRecord {
        MemoryRecord {
            key: key.to_string(),
            scope,
            value: format!("value-for-{key}"),
            session_id: SessionId::from_uuid(uuid::Uuid::nil()),
            recorded_in: SessionPhase::Execution,
            created_at: None,
        }
    }

    #[test]
    fn put_and_list_records() {
        let store = make_store(MockTransport::new_succeeding());
        store.put(make_record("k1", MemoryScope::Decision)).unwrap();
        store.put(make_record("k2", MemoryScope::Decision)).unwrap();
        store.put(make_record("k3", MemoryScope::Project)).unwrap();

        let decisions = store.list(MemoryScope::Decision).unwrap();
        assert_eq!(decisions.len(), 2);

        let projects = store.list(MemoryScope::Project).unwrap();
        assert_eq!(projects.len(), 1);
    }

    #[test]
    fn put_stamps_created_at() {
        let store = make_store(MockTransport::new_succeeding());
        let record = make_record("k1", MemoryScope::Decision);
        assert!(record.created_at.is_none());
        store.put(record.clone()).unwrap();

        let all = store.list_all().unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].created_at.is_some());
    }

    #[test]
    fn list_all_returns_all_scopes() {
        let store = make_store(MockTransport::new_succeeding());
        store.put(make_record("a", MemoryScope::Decision)).unwrap();
        store.put(make_record("b", MemoryScope::Project)).unwrap();
        store.put(make_record("c", MemoryScope::Benchmark)).unwrap();

        let all = store.list_all().unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn list_for_session_filters_by_session() {
        let store = make_store(MockTransport::new_succeeding());
        store.put(make_record("k1", MemoryScope::Decision)).unwrap();

        let nil_session = SessionId::from_uuid(uuid::Uuid::nil());
        let other_session = SessionId::from_uuid(uuid::Uuid::from_u128(1));

        let results = store.list_for_session(&nil_session).unwrap();
        assert_eq!(results.len(), 1);

        let results = store.list_for_session(&other_session).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn count_for_session() {
        let store = make_store(MockTransport::new_succeeding());
        store.put(make_record("k1", MemoryScope::Decision)).unwrap();
        store.put(make_record("k2", MemoryScope::Project)).unwrap();

        let nil_session = SessionId::from_uuid(uuid::Uuid::nil());
        assert_eq!(store.count_for_session(&nil_session).unwrap(), 2);
    }

    #[test]
    fn list_by_time_range() {
        let store = make_store(MockTransport::new_succeeding());
        store.put(make_record("k1", MemoryScope::Decision)).unwrap();

        let start = Utc::now() - chrono::Duration::seconds(10);
        let end = Utc::now() + chrono::Duration::seconds(10);
        let results = store.list_by_time_range(start, end).unwrap();
        assert_eq!(results.len(), 1);

        // Range in the past should return nothing
        let old_start = Utc::now() - chrono::Duration::hours(2);
        let old_end = Utc::now() - chrono::Duration::hours(1);
        let results = store.list_by_time_range(old_start, old_end).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn put_overwrites_same_key() {
        let store = make_store(MockTransport::new_succeeding());
        store.put(make_record("k1", MemoryScope::Decision)).unwrap();
        let mut updated = make_record("k1", MemoryScope::Decision);
        updated.value = "updated-value".to_string();
        store.put(updated).unwrap();

        let all = store.list_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].value, "updated-value");
    }

    #[test]
    fn put_with_bridge_failure_returns_error() {
        let store = make_store(MockTransport::new_failing());
        // Bridge store failure propagates — no silent degradation to file-only.
        let result = store.put(make_record("k1", MemoryScope::Decision));
        assert!(
            result.is_err(),
            "bridge failure must propagate, not silently persist"
        );
    }

    #[test]
    fn sync_pending_returns_zero_when_empty() {
        let store = make_store(MockTransport::new_succeeding());
        assert_eq!(store.sync_pending(), 0);
    }

    #[test]
    fn descriptor_returns_bridge_backend() {
        let store = make_store(MockTransport::new_succeeding());
        let desc = store.descriptor();
        assert!(desc.identity.contains("cognitive-bridge"));
    }
}
