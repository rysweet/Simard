use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::session::{SessionId, SessionPhase};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum MemoryScope {
    SessionScratch,
    SessionSummary,
    Project,
    Benchmark,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryRecord {
    pub key: String,
    pub scope: MemoryScope,
    pub value: String,
    pub session_id: SessionId,
    pub recorded_in: SessionPhase,
}

pub trait MemoryStore: Send + Sync {
    fn descriptor(&self) -> BackendDescriptor;

    fn put(&self, record: MemoryRecord) -> SimardResult<()>;

    fn list(&self, scope: MemoryScope) -> SimardResult<Vec<MemoryRecord>>;

    fn list_for_session(&self, session_id: &SessionId) -> SimardResult<Vec<MemoryRecord>>;

    fn count_for_session(&self, session_id: &SessionId) -> SimardResult<usize>;
}

#[derive(Debug)]
pub struct InMemoryMemoryStore {
    state: Mutex<MemoryStoreState>,
    descriptor: BackendDescriptor,
}

#[derive(Debug, Default)]
struct MemoryStoreState {
    records: Vec<MemoryRecord>,
    session_counts: HashMap<SessionId, usize>,
}

impl InMemoryMemoryStore {
    pub fn new(descriptor: BackendDescriptor) -> Self {
        Self {
            state: Mutex::new(MemoryStoreState::default()),
            descriptor,
        }
    }

    pub fn try_default() -> SimardResult<Self> {
        Ok(Self::new(BackendDescriptor::for_runtime_type::<Self>(
            "memory::in-memory",
            "runtime-port:memory-store",
            Freshness::now()?,
        )))
    }
}

impl MemoryStore for InMemoryMemoryStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn put(&self, record: MemoryRecord) -> SimardResult<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "memory".to_string(),
            })?;
        *state
            .session_counts
            .entry(record.session_id.clone())
            .or_insert(0) += 1;
        state.records.push(record);
        Ok(())
    }

    fn list(&self, scope: MemoryScope) -> SimardResult<Vec<MemoryRecord>> {
        let state = self
            .state
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "memory".to_string(),
            })?;
        Ok(state
            .iter()
            .filter(|record| record.scope == scope)
            .cloned()
            .collect())
    }

    fn list_for_session(&self, session_id: &SessionId) -> SimardResult<Vec<MemoryRecord>> {
        let state = self
            .state
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "memory".to_string(),
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
                store: "memory".to_string(),
            })?;
        Ok(state
            .session_counts
            .get(session_id)
            .copied()
            .unwrap_or_default())
    }
}

impl MemoryStoreState {
    fn iter(&self) -> impl Iterator<Item = &MemoryRecord> {
        self.records.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::{InMemoryMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
    use crate::session::{SessionId, SessionPhase};

    #[test]
    fn cached_session_counts_stay_in_sync_with_records() {
        let store = InMemoryMemoryStore::try_default().expect("store should initialize");
        let hot = SessionId::parse("session-00000000-0000-0000-0000-000000000001")
            .expect("session id should parse");
        let cold = SessionId::parse("session-00000000-0000-0000-0000-000000000002")
            .expect("session id should parse");

        store
            .put(MemoryRecord {
                key: "hot-scratch".to_string(),
                scope: MemoryScope::SessionScratch,
                value: "x".to_string(),
                session_id: hot.clone(),
                recorded_in: SessionPhase::Preparation,
            })
            .expect("first record should persist");
        store
            .put(MemoryRecord {
                key: "cold-summary".to_string(),
                scope: MemoryScope::SessionSummary,
                value: "y".to_string(),
                session_id: cold.clone(),
                recorded_in: SessionPhase::Persistence,
            })
            .expect("second record should persist");
        store
            .put(MemoryRecord {
                key: "hot-summary".to_string(),
                scope: MemoryScope::SessionSummary,
                value: "z".to_string(),
                session_id: hot.clone(),
                recorded_in: SessionPhase::Persistence,
            })
            .expect("third record should persist");

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
                .list(MemoryScope::SessionSummary)
                .expect("scope listing should still work")
                .len(),
            2
        );
        assert_eq!(
            store
                .list_for_session(&hot)
                .expect("session listing should still scan full records")
                .len(),
            2
        );
    }
}
