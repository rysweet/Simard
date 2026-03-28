use std::collections::HashMap;
use std::sync::Mutex;

use crate::base_types::BaseTypeId;
use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
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
    state: Mutex<EvidenceStoreState>,
    descriptor: BackendDescriptor,
}

#[derive(Debug, Default)]
struct EvidenceStoreState {
    records: Vec<EvidenceRecord>,
    session_counts: HashMap<SessionId, usize>,
}

impl InMemoryEvidenceStore {
    pub fn new(descriptor: BackendDescriptor) -> Self {
        Self {
            state: Mutex::new(EvidenceStoreState::default()),
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

impl EvidenceStore for InMemoryEvidenceStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn record(&self, record: EvidenceRecord) -> SimardResult<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "evidence".to_string(),
            })?;
        *state
            .session_counts
            .entry(record.session_id.clone())
            .or_insert(0) += 1;
        state.records.push(record);
        Ok(())
    }

    fn list_for_session(&self, session_id: &SessionId) -> SimardResult<Vec<EvidenceRecord>> {
        let state = self
            .state
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "evidence".to_string(),
            })?;
        Ok(state
            .iter()
            .filter(|record| &record.session_id == session_id)
            .cloned()
            .collect())
    }

    fn count_for_session(&self, session_id: &SessionId) -> SimardResult<usize> {
        let state = self
            .state
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "evidence".to_string(),
            })?;
        Ok(state
            .session_counts
            .get(session_id)
            .copied()
            .unwrap_or_default())
    }
}

impl EvidenceStoreState {
    fn iter(&self) -> impl Iterator<Item = &EvidenceRecord> {
        self.records.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::{EvidenceRecord, EvidenceSource, EvidenceStore, InMemoryEvidenceStore};
    use crate::session::{SessionId, SessionPhase};

    #[test]
    fn cached_session_counts_match_recorded_evidence() {
        let store = InMemoryEvidenceStore::try_default().expect("store should initialize");
        let hot = SessionId::parse("session-00000000-0000-0000-0000-000000000001")
            .expect("session id should parse");
        let cold = SessionId::parse("session-00000000-0000-0000-0000-000000000002")
            .expect("session id should parse");

        for (id, session_id) in [
            ("ev-1", hot.clone()),
            ("ev-2", cold.clone()),
            ("ev-3", hot.clone()),
        ] {
            store
                .record(EvidenceRecord {
                    id: id.to_string(),
                    session_id,
                    phase: SessionPhase::Execution,
                    detail: "recorded".to_string(),
                    source: EvidenceSource::Runtime,
                })
                .expect("record should persist");
        }

        assert_eq!(
            store
                .count_for_session(&hot)
                .expect("hot session count should be indexed"),
            2
        );
        assert_eq!(
            store
                .count_for_session(&cold)
                .expect("cold session count should be indexed"),
            1
        );
        assert_eq!(
            store
                .list_for_session(&hot)
                .expect("session listing should still work")
                .len(),
            2
        );
    }
}
