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
