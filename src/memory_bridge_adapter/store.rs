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
    use super::super::test_helpers::{make_record, test_store};
    use super::*;
    use crate::memory::MemoryScope;

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
}
