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

    // Fall back to legacy plain-array format.
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
}
