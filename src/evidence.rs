use std::sync::Mutex;

use crate::base_types::BaseTypeId;
use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness, Provenance};
use crate::session::{SessionId, SessionPhase};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvidenceSource {
    Runtime,
    BaseType(BaseTypeId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
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
        Ok(Self::new(BackendDescriptor::new(
            "evidence::in-memory",
            Provenance::injected("runtime-port:evidence-store"),
            Freshness::now()?,
        )))
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
