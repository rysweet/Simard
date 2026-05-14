use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::persistence::persist_json;
use crate::session::SessionId;

use super::store::MemoryStore;
use super::types::{MEMORY_STORE_NAME, MemoryRecord, MemoryScope};

/// On-disk envelope that pairs memory records with a CRC32 checksum.
#[derive(Serialize, Deserialize)]
struct ChecksummedPayload {
    crc32: u32,
    records: Vec<MemoryRecord>,
}

fn compute_crc32(records: &[MemoryRecord]) -> SimardResult<u32> {
    let bytes = serde_json::to_vec(records).map_err(|e| SimardError::PersistentStoreIo {
        store: MEMORY_STORE_NAME.to_string(),
        action: "checksum-serialize".to_string(),
        path: PathBuf::new(),
        reason: e.to_string(),
    })?;
    Ok(crc32fast::hash(&bytes))
}

/// Load memory records from a file, validating the CRC32 checksum.
/// Supports both the new checksummed format and legacy plain-array format.
#[tracing::instrument(skip_all)]
fn load_checksummed(path: &Path) -> SimardResult<Vec<MemoryRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents = std::fs::read(path).map_err(|e| SimardError::PersistentStoreIo {
        store: MEMORY_STORE_NAME.to_string(),
        action: "read".to_string(),
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;

    // Try checksummed format first.
    if let Ok(payload) = serde_json::from_slice::<ChecksummedPayload>(&contents) {
        let expected = compute_crc32(&payload.records)?;
        if payload.crc32 != expected {
            return Err(SimardError::MemoryIntegrityError {
                path: path.to_path_buf(),
                reason: format!(
                    "CRC32 mismatch: stored={:#010x}, computed={:#010x}",
                    payload.crc32, expected
                ),
            });
        }
        return Ok(payload.records);
    }

    // Try legacy plain-array format (migration support).
    serde_json::from_slice::<Vec<MemoryRecord>>(&contents).map_err(|e| {
        SimardError::PersistentStoreIo {
            store: MEMORY_STORE_NAME.to_string(),
            action: "deserialize".to_string(),
            path: path.to_path_buf(),
            reason: e.to_string(),
        }
    })
}

/// Persist memory records with a CRC32 checksum envelope.
fn persist_checksummed(path: &Path, records: &[MemoryRecord]) -> SimardResult<()> {
    let crc32 = compute_crc32(records)?;
    let payload = ChecksummedPayload {
        crc32,
        records: records.to_vec(),
    };
    persist_json(MEMORY_STORE_NAME, path, &payload)
}

#[derive(Debug)]
pub struct FileBackedMemoryStore {
    records: Mutex<Vec<MemoryRecord>>,
    path: PathBuf,
    descriptor: BackendDescriptor,
}

impl FileBackedMemoryStore {
    pub fn new(path: impl Into<PathBuf>, descriptor: BackendDescriptor) -> SimardResult<Self> {
        let path = path.into();
        Ok(Self {
            records: Mutex::new(load_checksummed(&path)?),
            path,
            descriptor,
        })
    }

    pub fn try_new(path: impl Into<PathBuf>) -> SimardResult<Self> {
        let path = path.into();
        Self::new(
            path,
            BackendDescriptor::for_runtime_type::<Self>(
                "memory::json-file-store",
                "runtime-port:memory-store:file-json",
                Freshness::now()?,
            ),
        )
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn persist(&self, records: &[MemoryRecord]) -> SimardResult<()> {
        persist_checksummed(&self.path, records)
    }

    /// Prune the records of a single `scope` down to at most `cap` entries,
    /// evicting oldest-first by `(created_at, key)` ascending order
    /// (with `None` timestamps treated as oldest).
    ///
    /// Atomicity mirrors [`MemoryStore::put`]: lock → filter → persist
    /// (checksummed) → swap-on-success. If `persist` fails, in-memory state
    /// and the on-disk file are both unchanged.
    ///
    /// When the in-scope record count is already `<= cap`, this is a strict
    /// noop: it returns `Ok(0)` and performs **no** disk I/O (the on-disk
    /// file is byte-identical before and after).
    ///
    /// Records outside the target scope are preserved both in count and in
    /// their original Vec order. Returns the number of records evicted.
    #[tracing::instrument(skip(self), fields(scope = ?scope, cap))]
    pub fn prune_scope_to_cap(&self, scope: MemoryScope, cap: usize) -> SimardResult<usize> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: MEMORY_STORE_NAME.to_string(),
            })?;

        let in_scope_count = records.iter().filter(|r| r.scope == scope).count();
        if in_scope_count <= cap {
            return Ok(0);
        }
        let to_evict = in_scope_count - cap;

        // Partition: keep out-of-scope records in their original order;
        // collect in-scope records into a separate vec we can sort.
        let mut out_of_scope: Vec<MemoryRecord> =
            Vec::with_capacity(records.len() - in_scope_count);
        let mut in_scope: Vec<MemoryRecord> = Vec::with_capacity(in_scope_count);
        for record in records.iter() {
            if record.scope == scope {
                in_scope.push(record.clone());
            } else {
                out_of_scope.push(record.clone());
            }
        }

        // FIFO eviction: sort by (created_at, key) ascending. Rust's stdlib
        // ordering on Option<T> places None < Some(_), which matches the
        // contract that `None` timestamps are treated as oldest.
        in_scope.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.key.cmp(&b.key))
        });
        // Drop the oldest `to_evict` entries; retain the most-recent `cap`.
        let retained_in_scope = in_scope.split_off(to_evict);

        let mut candidate = out_of_scope;
        candidate.extend(retained_in_scope);

        // Persist first — if this fails, in-memory state stays unchanged.
        self.persist(&candidate)?;
        *records = candidate;
        Ok(to_evict)
    }
}

impl MemoryStore for FileBackedMemoryStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    #[tracing::instrument(skip_all)]
    fn put(&self, record: MemoryRecord) -> SimardResult<()> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: MEMORY_STORE_NAME.to_string(),
            })?;
        // Stamp created_at if not already set.
        let mut record = record;
        if record.created_at.is_none() {
            record.created_at = Some(Utc::now());
        }
        // Build the updated list without mutating in-memory state yet.
        let mut candidate = records.clone();
        if let Some(existing) = candidate
            .iter_mut()
            .find(|existing| existing.key == record.key)
        {
            *existing = record;
        } else {
            candidate.push(record);
        }
        // Persist first — if this fails, in-memory state stays unchanged.
        self.persist(&candidate)?;
        *records = candidate;
        Ok(())
    }

    fn list(&self, scope: MemoryScope) -> SimardResult<Vec<MemoryRecord>> {
        Ok(self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: MEMORY_STORE_NAME.to_string(),
            })?
            .iter()
            .filter(|record| record.scope == scope)
            .cloned()
            .collect())
    }

    fn list_for_session(&self, session_id: &SessionId) -> SimardResult<Vec<MemoryRecord>> {
        Ok(self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: MEMORY_STORE_NAME.to_string(),
            })?
            .iter()
            .filter(|record| &record.session_id == session_id)
            .cloned()
            .collect())
    }

    fn count_for_session(&self, session_id: &SessionId) -> SimardResult<usize> {
        Ok(self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: MEMORY_STORE_NAME.to_string(),
            })?
            .iter()
            .filter(|record| &record.session_id == session_id)
            .count())
    }

    fn list_all(&self) -> SimardResult<Vec<MemoryRecord>> {
        Ok(self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: MEMORY_STORE_NAME.to_string(),
            })?
            .clone())
    }

    fn list_by_time_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> SimardResult<Vec<MemoryRecord>> {
        Ok(self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: MEMORY_STORE_NAME.to_string(),
            })?
            .iter()
            .filter(|r| r.created_at.is_some_and(|t| t >= start && t < end))
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionPhase;
    use chrono::Duration;
    use uuid::Uuid;

    fn test_session_id() -> SessionId {
        SessionId::from_uuid(Uuid::nil())
    }

    fn other_session_id() -> SessionId {
        SessionId::from_uuid(Uuid::from_u128(1))
    }

    fn make_record(key: &str, scope: MemoryScope, session_id: &SessionId) -> MemoryRecord {
        MemoryRecord {
            key: key.to_string(),
            scope,
            value: format!("val-{key}"),
            session_id: session_id.clone(),
            recorded_in: SessionPhase::Execution,
            created_at: None,
        }
    }

    #[test]
    fn put_and_reload_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.json");
        let sid = test_session_id();

        {
            let store = FileBackedMemoryStore::try_new(&path).unwrap();
            store
                .put(make_record("k1", MemoryScope::Project, &sid))
                .unwrap();
            store
                .put(make_record("k2", MemoryScope::Decision, &sid))
                .unwrap();
            assert_eq!(store.list_all().unwrap().len(), 2);
        }

        // Reload from the persisted file
        let store2 = FileBackedMemoryStore::try_new(&path).unwrap();
        assert_eq!(store2.list_all().unwrap().len(), 2);
    }

    #[test]
    fn put_upserts_by_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.json");
        let sid = test_session_id();
        let store = FileBackedMemoryStore::try_new(&path).unwrap();

        store
            .put(make_record("dup", MemoryScope::Project, &sid))
            .unwrap();
        let mut updated = make_record("dup", MemoryScope::Project, &sid);
        updated.value = "new-value".to_string();
        store.put(updated).unwrap();

        let all = store.list_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].value, "new-value");
    }

    #[test]
    fn put_stamps_created_at() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.json");
        let sid = test_session_id();
        let store = FileBackedMemoryStore::try_new(&path).unwrap();

        let record = make_record("k", MemoryScope::Project, &sid);
        assert!(record.created_at.is_none());
        store.put(record).unwrap();
        assert!(store.list_all().unwrap()[0].created_at.is_some());
    }

    #[test]
    fn list_filters_by_scope() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.json");
        let sid = test_session_id();
        let store = FileBackedMemoryStore::try_new(&path).unwrap();

        store
            .put(make_record("a", MemoryScope::Project, &sid))
            .unwrap();
        store
            .put(make_record("b", MemoryScope::Decision, &sid))
            .unwrap();

        assert_eq!(store.list(MemoryScope::Project).unwrap().len(), 1);
        assert_eq!(store.list(MemoryScope::Decision).unwrap().len(), 1);
        assert_eq!(store.list(MemoryScope::Benchmark).unwrap().len(), 0);
    }

    #[test]
    fn list_for_session_filters() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.json");
        let s1 = test_session_id();
        let s2 = other_session_id();
        let store = FileBackedMemoryStore::try_new(&path).unwrap();

        store
            .put(make_record("a", MemoryScope::Project, &s1))
            .unwrap();
        store
            .put(make_record("b", MemoryScope::Project, &s2))
            .unwrap();

        assert_eq!(store.list_for_session(&s1).unwrap().len(), 1);
        assert_eq!(store.count_for_session(&s2).unwrap(), 1);
    }

    #[test]
    fn list_by_time_range_filters() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.json");
        let sid = test_session_id();
        let store = FileBackedMemoryStore::try_new(&path).unwrap();

        store
            .put(make_record("a", MemoryScope::Project, &sid))
            .unwrap();

        let now = Utc::now();
        let start = now - Duration::seconds(5);
        let end = now + Duration::seconds(5);
        assert_eq!(store.list_by_time_range(start, end).unwrap().len(), 1);

        let old_end = now - Duration::seconds(50);
        assert_eq!(
            store
                .list_by_time_range(old_end - Duration::seconds(50), old_end)
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn empty_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-yet.json");
        let store = FileBackedMemoryStore::try_new(&path).unwrap();
        assert!(store.list_all().unwrap().is_empty());
    }

    #[test]
    fn path_accessor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mem.json");
        let store = FileBackedMemoryStore::try_new(&path).unwrap();
        assert_eq!(store.path(), path);
    }

    // ===================================================================
    // prune_scope_to_cap — bounded meeting-memory persistence (issue #1763)
    // ===================================================================
    //
    // Contract under test (from Step 2c locked requirements):
    //   - FIFO eviction by (created_at, key) ascending, with `None`
    //     timestamps sorting before `Some(_)` (treated as oldest).
    //   - Atomicity: lock → filter → persist-checksummed → swap-on-success.
    //     If persist fails, in-memory state is unchanged.
    //   - True noop when count <= cap: returns Ok(0) and skips the persist
    //     write entirely (no disk I/O, no on-disk byte change).
    //   - Returns the number of records evicted.
    //   - Only the target scope is touched; out-of-scope records are
    //     preserved both in count and in their original on-disk presence.

    /// Build a record with an explicit (or absent) `created_at` so we can
    /// control FIFO ordering deterministically. Bypasses `put()`'s
    /// auto-stamping by writing the record directly into the on-disk
    /// envelope, then reloading via `try_new()` — the public/test-friendly
    /// way to construct fixtures with `None` timestamps.
    fn make_record_with_ts(
        key: &str,
        scope: MemoryScope,
        session_id: &SessionId,
        created_at: Option<DateTime<Utc>>,
    ) -> MemoryRecord {
        MemoryRecord {
            key: key.to_string(),
            scope,
            value: format!("val-{key}"),
            session_id: session_id.clone(),
            recorded_in: SessionPhase::Execution,
            created_at,
        }
    }

    /// Persist a hand-crafted record set to a path via the same checksummed
    /// writer the production code uses, then return a freshly loaded store
    /// pointing at that path. This is the one supported way to construct a
    /// store containing records with `created_at: None` for tests.
    fn store_with_records(
        path: &std::path::Path,
        records: &[MemoryRecord],
    ) -> FileBackedMemoryStore {
        persist_checksummed(path, records).expect("seed records persist");
        FileBackedMemoryStore::try_new(path).expect("open seeded store")
    }

    fn ts(secs: i64) -> Option<DateTime<Utc>> {
        Some(DateTime::<Utc>::from_timestamp(1_700_000_000 + secs, 0).unwrap())
    }

    #[test]
    fn prune_under_cap_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.json");
        let sid = test_session_id();
        let records = vec![
            make_record_with_ts("d1", MemoryScope::Decision, &sid, ts(1)),
            make_record_with_ts("d2", MemoryScope::Decision, &sid, ts(2)),
            make_record_with_ts("d3", MemoryScope::Decision, &sid, ts(3)),
        ];
        let store = store_with_records(&path, &records);
        let bytes_before = std::fs::read(&path).unwrap();

        let evicted = store.prune_scope_to_cap(MemoryScope::Decision, 10).unwrap();

        assert_eq!(evicted, 0, "no eviction when under cap");
        assert_eq!(store.list(MemoryScope::Decision).unwrap().len(), 3);
        let bytes_after = std::fs::read(&path).unwrap();
        assert_eq!(
            bytes_before, bytes_after,
            "no disk write when under cap (true noop)"
        );
    }

    #[test]
    fn prune_at_cap_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.json");
        let sid = test_session_id();
        let records: Vec<MemoryRecord> = (0..5)
            .map(|i| {
                make_record_with_ts(&format!("d{i}"), MemoryScope::Decision, &sid, ts(i as i64))
            })
            .collect();
        let store = store_with_records(&path, &records);
        let bytes_before = std::fs::read(&path).unwrap();

        let evicted = store.prune_scope_to_cap(MemoryScope::Decision, 5).unwrap();

        assert_eq!(evicted, 0, "no eviction when exactly at cap");
        assert_eq!(store.list(MemoryScope::Decision).unwrap().len(), 5);
        let bytes_after = std::fs::read(&path).unwrap();
        assert_eq!(
            bytes_before, bytes_after,
            "no disk write when at cap (true noop)"
        );
    }

    #[test]
    fn prune_evicts_oldest_by_created_at() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.json");
        let sid = test_session_id();
        // Insert in non-chronological order to confirm sort-by-created_at,
        // not insertion order.
        let records = vec![
            make_record_with_ts("d-newest", MemoryScope::Decision, &sid, ts(500)),
            make_record_with_ts("d-oldest", MemoryScope::Decision, &sid, ts(100)),
            make_record_with_ts("d-middle", MemoryScope::Decision, &sid, ts(300)),
            make_record_with_ts("d-second-newest", MemoryScope::Decision, &sid, ts(400)),
        ];
        let store = store_with_records(&path, &records);

        let evicted = store.prune_scope_to_cap(MemoryScope::Decision, 2).unwrap();

        assert_eq!(evicted, 2);
        let kept_keys: std::collections::BTreeSet<String> = store
            .list(MemoryScope::Decision)
            .unwrap()
            .into_iter()
            .map(|r| r.key)
            .collect();
        let expected: std::collections::BTreeSet<String> =
            ["d-newest".to_string(), "d-second-newest".to_string()]
                .into_iter()
                .collect();
        assert_eq!(
            kept_keys, expected,
            "the two NEWEST records survive; oldest two are evicted"
        );
    }

    #[test]
    fn prune_tiebreaks_by_key_when_created_at_equal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.json");
        let sid = test_session_id();
        // All four records share the SAME timestamp — the tiebreaker is
        // `key` ascending. With cap=2, the lexicographically-smallest two
        // keys must be evicted.
        let same = ts(42);
        let records = vec![
            make_record_with_ts("d-charlie", MemoryScope::Decision, &sid, same),
            make_record_with_ts("d-alpha", MemoryScope::Decision, &sid, same),
            make_record_with_ts("d-bravo", MemoryScope::Decision, &sid, same),
            make_record_with_ts("d-delta", MemoryScope::Decision, &sid, same),
        ];
        let store = store_with_records(&path, &records);

        let evicted = store.prune_scope_to_cap(MemoryScope::Decision, 2).unwrap();

        assert_eq!(evicted, 2);
        let kept_keys: std::collections::BTreeSet<String> = store
            .list(MemoryScope::Decision)
            .unwrap()
            .into_iter()
            .map(|r| r.key)
            .collect();
        let expected: std::collections::BTreeSet<String> =
            ["d-charlie".to_string(), "d-delta".to_string()]
                .into_iter()
                .collect();
        assert_eq!(
            kept_keys, expected,
            "with equal timestamps, keys 'd-alpha' and 'd-bravo' are evicted (smallest first)"
        );
    }

    #[test]
    fn prune_treats_none_created_at_as_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.json");
        let sid = test_session_id();
        // Mix of None (treated as oldest) and Some(_). With cap=2, only the
        // two records with the LATEST Some(_) timestamps may survive.
        let records = vec![
            make_record_with_ts("d-no-ts-a", MemoryScope::Decision, &sid, None),
            make_record_with_ts("d-no-ts-b", MemoryScope::Decision, &sid, None),
            make_record_with_ts("d-ts-old", MemoryScope::Decision, &sid, ts(10)),
            make_record_with_ts("d-ts-mid", MemoryScope::Decision, &sid, ts(20)),
            make_record_with_ts("d-ts-new", MemoryScope::Decision, &sid, ts(30)),
        ];
        let store = store_with_records(&path, &records);

        let evicted = store.prune_scope_to_cap(MemoryScope::Decision, 2).unwrap();

        assert_eq!(evicted, 3);
        let kept_keys: std::collections::BTreeSet<String> = store
            .list(MemoryScope::Decision)
            .unwrap()
            .into_iter()
            .map(|r| r.key)
            .collect();
        let expected: std::collections::BTreeSet<String> =
            ["d-ts-mid".to_string(), "d-ts-new".to_string()]
                .into_iter()
                .collect();
        assert_eq!(
            kept_keys, expected,
            "None < Some(_) — both no-ts records are evicted before any timestamped one"
        );
    }

    #[test]
    fn prune_only_affects_target_scope() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.json");
        let sid = test_session_id();
        let records = vec![
            make_record_with_ts("d-1", MemoryScope::Decision, &sid, ts(1)),
            make_record_with_ts("d-2", MemoryScope::Decision, &sid, ts(2)),
            make_record_with_ts("d-3", MemoryScope::Decision, &sid, ts(3)),
            make_record_with_ts("p-1", MemoryScope::Project, &sid, ts(0)),
            make_record_with_ts("p-2", MemoryScope::Project, &sid, ts(0)),
            make_record_with_ts("ss-1", MemoryScope::SessionSummary, &sid, ts(0)),
            make_record_with_ts("scratch-1", MemoryScope::SessionScratch, &sid, None),
        ];
        let store = store_with_records(&path, &records);

        let evicted = store.prune_scope_to_cap(MemoryScope::Decision, 1).unwrap();

        assert_eq!(evicted, 2);
        assert_eq!(
            store.list(MemoryScope::Decision).unwrap().len(),
            1,
            "Decision scope reduced to cap"
        );
        assert_eq!(
            store.list(MemoryScope::Project).unwrap().len(),
            2,
            "Project scope untouched"
        );
        assert_eq!(
            store.list(MemoryScope::SessionSummary).unwrap().len(),
            1,
            "SessionSummary scope untouched"
        );
        assert_eq!(
            store.list(MemoryScope::SessionScratch).unwrap().len(),
            1,
            "SessionScratch scope untouched"
        );
    }

    #[test]
    fn prune_persists_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.json");
        let sid = test_session_id();
        let records = vec![
            make_record_with_ts("d-1", MemoryScope::Decision, &sid, ts(1)),
            make_record_with_ts("d-2", MemoryScope::Decision, &sid, ts(2)),
            make_record_with_ts("d-3", MemoryScope::Decision, &sid, ts(3)),
            make_record_with_ts("d-4", MemoryScope::Decision, &sid, ts(4)),
            make_record_with_ts("p-1", MemoryScope::Project, &sid, ts(99)),
        ];
        let store = store_with_records(&path, &records);

        store.prune_scope_to_cap(MemoryScope::Decision, 2).unwrap();

        // Drop the store handle; reload from disk and verify the cap held.
        drop(store);
        let reloaded = FileBackedMemoryStore::try_new(&path).unwrap();
        let decisions: Vec<String> = reloaded
            .list(MemoryScope::Decision)
            .unwrap()
            .into_iter()
            .map(|r| r.key)
            .collect();
        let decisions_set: std::collections::BTreeSet<String> = decisions.into_iter().collect();
        let expected: std::collections::BTreeSet<String> =
            ["d-3".to_string(), "d-4".to_string()].into_iter().collect();
        assert_eq!(
            decisions_set, expected,
            "newest two Decision records survive a process restart"
        );
        assert_eq!(
            reloaded.list(MemoryScope::Project).unwrap().len(),
            1,
            "out-of-scope records survive round trip"
        );
    }

    #[test]
    fn prune_returns_eviction_count() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.json");
        let sid = test_session_id();
        let records: Vec<MemoryRecord> = (0..10)
            .map(|i| {
                make_record_with_ts(
                    &format!("d{i:02}"),
                    MemoryScope::Decision,
                    &sid,
                    ts(i as i64),
                )
            })
            .collect();
        let store = store_with_records(&path, &records);

        // count=10, cap=4 → expect 6 evicted.
        let evicted = store.prune_scope_to_cap(MemoryScope::Decision, 4).unwrap();
        assert_eq!(evicted, 6);
        assert_eq!(store.list(MemoryScope::Decision).unwrap().len(), 4);

        // Re-running with the same cap is a noop and returns 0.
        let evicted_again = store.prune_scope_to_cap(MemoryScope::Decision, 4).unwrap();
        assert_eq!(evicted_again, 0);
    }
}
