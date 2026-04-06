//! [`CognitiveBridgeMemoryStore`] — the bridge between `MemoryStore` and cognitive memory.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::{SimardError, SimardResult};
use crate::memory::{CognitiveMemoryType, FileBackedMemoryStore, MemoryRecord, MemoryStore};
use crate::memory_bridge::CognitiveMemoryBridge;
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
/// Falls back to `FileBackedMemoryStore` if the bridge fails.
pub struct CognitiveBridgeMemoryStore {
    bridge: CognitiveMemoryBridge,
    fallback: FileBackedMemoryStore,
    /// Track records locally for list/count operations since cognitive memory
    /// search is keyword-based and cannot filter by exact scope/session.
    /// Keyed by record key for O(1) dedup on put.
    pub(super) records: Mutex<HashMap<String, MemoryRecord>>,
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
        use crate::memory::CognitiveMemoryType;

        const ALL_TYPES: [CognitiveMemoryType; 6] = [
            CognitiveMemoryType::Sensory,
            CognitiveMemoryType::Working,
            CognitiveMemoryType::Episodic,
            CognitiveMemoryType::Semantic,
            CognitiveMemoryType::Procedural,
            CognitiveMemoryType::Prospective,
        ];

        let mut hydrated = 0usize;
        for memory_type in ALL_TYPES {
            match self.fallback.list(memory_type) {
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
                         failed to load scope {memory_type:?}: {e}"
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
    fn bridge_fallback_list(&self, memory_type: CognitiveMemoryType) -> Vec<MemoryRecord> {
        let query = format!("scope:{memory_type:?}");
        match self.search_facts_with_retry(&query, 200, 0.0) {
            Ok(facts) => {
                let records: Vec<MemoryRecord> = facts
                    .iter()
                    .map(fact_to_record)
                    .filter(|r| r.memory_type == memory_type)
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
                    "[simard] cognitive-bridge: bridge fallback for scope {memory_type:?} failed: {e}"
                );
                Vec::new()
            }
        }
    }

    /// Store a record in cognitive memory as a semantic fact.
    fn store_as_fact(&self, record: &MemoryRecord) -> SimardResult<String> {
        let tags = vec![
            scope_tag(record.memory_type),
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

    fn list(&self, memory_type: CognitiveMemoryType) -> SimardResult<Vec<MemoryRecord>> {
        let records = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: STORE_NAME.to_string(),
            })?;
        let local: Vec<MemoryRecord> = records
            .values()
            .filter(|r| r.memory_type == memory_type)
            .cloned()
            .collect();
        if !local.is_empty() {
            return Ok(local);
        }
        // Local miss — try bridge fallback to recover cross-session data.
        drop(records); // release lock before bridge call
        let bridged = self.bridge_fallback_list(memory_type);
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
