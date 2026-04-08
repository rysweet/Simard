use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::base_types::BaseTypeId;
use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::persistence::{load_json_or_default, persist_json};
use crate::session::{SessionId, SessionPhase};

const EVIDENCE_STORE_NAME: &str = "evidence";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum EvidenceSource {
    Runtime,
    BaseType(BaseTypeId),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRecord {
    pub id: String,
    pub session_id: SessionId,
    pub phase: SessionPhase,
    pub detail: String,
    pub source: EvidenceSource,
}

pub trait EvidenceStore: Send + Sync {
    fn descriptor(&self) -> BackendDescriptor;

    fn record(&self, record: EvidenceRecord) -> SimardResult<()>;

    fn list_for_session(&self, session_id: &SessionId) -> SimardResult<Vec<EvidenceRecord>>;

    fn count_for_session(&self, session_id: &SessionId) -> SimardResult<usize>;
}

#[derive(Debug)]
pub struct InMemoryEvidenceStore {
    records: Mutex<Vec<EvidenceRecord>>,
    descriptor: BackendDescriptor,
}

impl InMemoryEvidenceStore {
    pub fn new(descriptor: BackendDescriptor) -> Self {
        Self {
            records: Mutex::new(Vec::new()),
            descriptor,
        }
    }

    pub fn try_default() -> SimardResult<Self> {
        Ok(Self::new(BackendDescriptor::for_runtime_type::<Self>(
            "evidence::in-memory",
            "runtime-port:evidence-store",
            Freshness::now()?,
        )))
    }
}

#[derive(Debug)]
pub struct FileBackedEvidenceStore {
    records: Mutex<Vec<EvidenceRecord>>,
    path: PathBuf,
    descriptor: BackendDescriptor,
}

impl FileBackedEvidenceStore {
    pub fn new(path: impl Into<PathBuf>, descriptor: BackendDescriptor) -> SimardResult<Self> {
        let path = path.into();
        Ok(Self {
            records: Mutex::new(load_json_or_default(EVIDENCE_STORE_NAME, &path)?),
            path,
            descriptor,
        })
    }

    pub fn try_new(path: impl Into<PathBuf>) -> SimardResult<Self> {
        let path = path.into();
        Self::new(
            path,
            BackendDescriptor::for_runtime_type::<Self>(
                "evidence::json-file-store",
                "runtime-port:evidence-store:file-json",
                Freshness::now()?,
            ),
        )
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn persist(&self, records: &[EvidenceRecord]) -> SimardResult<()> {
        persist_json(EVIDENCE_STORE_NAME, &self.path, &records)
    }
}

impl EvidenceStore for InMemoryEvidenceStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn record(&self, record: EvidenceRecord) -> SimardResult<()> {
        self.records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "evidence".to_string(),
            })?
            .push(record);
        Ok(())
    }

    fn list_for_session(&self, session_id: &SessionId) -> SimardResult<Vec<EvidenceRecord>> {
        Ok(self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "evidence".to_string(),
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
                store: "evidence".to_string(),
            })?
            .iter()
            .filter(|record| &record.session_id == session_id)
            .count())
    }
}

impl EvidenceStore for FileBackedEvidenceStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn record(&self, record: EvidenceRecord) -> SimardResult<()> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: EVIDENCE_STORE_NAME.to_string(),
            })?;
        if let Some(existing) = records.iter_mut().find(|existing| existing.id == record.id) {
            *existing = record;
        } else {
            records.push(record);
        }
        self.persist(&records)
    }

    fn list_for_session(&self, session_id: &SessionId) -> SimardResult<Vec<EvidenceRecord>> {
        Ok(self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: EVIDENCE_STORE_NAME.to_string(),
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
                store: EVIDENCE_STORE_NAME.to_string(),
            })?
            .iter()
            .filter(|record| &record.session_id == session_id)
            .count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn test_session_id() -> SessionId {
        SessionId::from_uuid(Uuid::nil())
    }

    fn other_session_id() -> SessionId {
        SessionId::from_uuid(Uuid::from_u128(1))
    }

    fn make_record(id: &str, session_id: &SessionId) -> EvidenceRecord {
        EvidenceRecord {
            id: id.to_string(),
            session_id: session_id.clone(),
            phase: SessionPhase::Execution,
            detail: format!("detail-{id}"),
            source: EvidenceSource::Runtime,
        }
    }

    // ── EvidenceSource / EvidenceRecord serde round-trip ────────────

    #[test]
    fn evidence_record_serde_round_trip() {
        let record = make_record("r1", &test_session_id());
        let json = serde_json::to_string(&record).unwrap();
        let back: EvidenceRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, back);
    }

    #[test]
    fn evidence_source_variants_serde() {
        let runtime = EvidenceSource::Runtime;
        let base = EvidenceSource::BaseType(BaseTypeId::new("bt-1"));
        let rt_json = serde_json::to_string(&runtime).unwrap();
        let bt_json = serde_json::to_string(&base).unwrap();
        assert_eq!(
            serde_json::from_str::<EvidenceSource>(&rt_json).unwrap(),
            runtime
        );
        assert_eq!(
            serde_json::from_str::<EvidenceSource>(&bt_json).unwrap(),
            base
        );
    }

    // ── InMemoryEvidenceStore ───────────────────────────────────────

    #[test]
    fn in_memory_record_and_list() {
        let store = InMemoryEvidenceStore::try_default().unwrap();
        let sid = test_session_id();
        store.record(make_record("a", &sid)).unwrap();
        store.record(make_record("b", &sid)).unwrap();

        let records = store.list_for_session(&sid).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(store.count_for_session(&sid).unwrap(), 2);
    }

    #[test]
    fn in_memory_list_empty_session() {
        let store = InMemoryEvidenceStore::try_default().unwrap();
        let sid = test_session_id();
        assert!(store.list_for_session(&sid).unwrap().is_empty());
        assert_eq!(store.count_for_session(&sid).unwrap(), 0);
    }

    #[test]
    fn in_memory_filters_by_session() {
        let store = InMemoryEvidenceStore::try_default().unwrap();
        let s1 = test_session_id();
        let s2 = other_session_id();
        store.record(make_record("a", &s1)).unwrap();
        store.record(make_record("b", &s2)).unwrap();
        assert_eq!(store.list_for_session(&s1).unwrap().len(), 1);
        assert_eq!(store.list_for_session(&s2).unwrap().len(), 1);
    }

    #[test]
    fn in_memory_descriptor_not_empty() {
        let store = InMemoryEvidenceStore::try_default().unwrap();
        let desc = store.descriptor();
        assert!(!format!("{desc:?}").is_empty());
    }

    // ── FileBackedEvidenceStore ─────────────────────────────────────

    #[test]
    fn file_backed_persist_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("evidence.json");
        let sid = test_session_id();

        {
            let store = FileBackedEvidenceStore::try_new(&path).unwrap();
            store.record(make_record("x", &sid)).unwrap();
            store.record(make_record("y", &sid)).unwrap();
            assert_eq!(store.count_for_session(&sid).unwrap(), 2);
        }

        // Reload from disk
        let store2 = FileBackedEvidenceStore::try_new(&path).unwrap();
        assert_eq!(store2.list_for_session(&sid).unwrap().len(), 2);
    }

    #[test]
    fn file_backed_upsert_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("evidence.json");
        let sid = test_session_id();

        let store = FileBackedEvidenceStore::try_new(&path).unwrap();
        store.record(make_record("dup", &sid)).unwrap();

        let mut updated = make_record("dup", &sid);
        updated.detail = "updated-detail".to_string();
        store.record(updated.clone()).unwrap();

        let records = store.list_for_session(&sid).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].detail, "updated-detail");
    }

    #[test]
    fn file_backed_empty_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let store = FileBackedEvidenceStore::try_new(&path).unwrap();
        assert!(
            store
                .list_for_session(&test_session_id())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn file_backed_path_accessor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ev.json");
        let store = FileBackedEvidenceStore::try_new(&path).unwrap();
        assert_eq!(store.path(), path);
    }
}
