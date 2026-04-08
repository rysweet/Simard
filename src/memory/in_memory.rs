use std::sync::Mutex;

use chrono::{DateTime, Utc};

use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::session::SessionId;

use super::store::MemoryStore;
use super::types::{MemoryRecord, MemoryScope};

#[derive(Debug)]
pub struct InMemoryMemoryStore {
    records: Mutex<Vec<MemoryRecord>>,
    descriptor: BackendDescriptor,
}

impl InMemoryMemoryStore {
    pub fn new(descriptor: BackendDescriptor) -> Self {
        Self {
            records: Mutex::new(Vec::new()),
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
        let mut record = record;
        if record.created_at.is_none() {
            record.created_at = Some(Utc::now());
        }
        self.records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "memory".to_string(),
            })?
            .push(record);
        Ok(())
    }

    fn list(&self, scope: MemoryScope) -> SimardResult<Vec<MemoryRecord>> {
        Ok(self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "memory".to_string(),
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
                store: "memory".to_string(),
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
                store: "memory".to_string(),
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
                store: "memory".to_string(),
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
                store: "memory".to_string(),
            })?
            .iter()
            .filter(|r| r.created_at.map(|t| t >= start && t < end).unwrap_or(false))
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
    fn put_stamps_created_at() {
        let store = InMemoryMemoryStore::try_default().unwrap();
        let sid = test_session_id();
        let record = make_record("k1", MemoryScope::Project, &sid);
        assert!(record.created_at.is_none());
        store.put(record).unwrap();

        let all = store.list_all().unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].created_at.is_some());
    }

    #[test]
    fn list_filters_by_scope() {
        let store = InMemoryMemoryStore::try_default().unwrap();
        let sid = test_session_id();
        store
            .put(make_record("a", MemoryScope::Project, &sid))
            .unwrap();
        store
            .put(make_record("b", MemoryScope::Decision, &sid))
            .unwrap();
        store
            .put(make_record("c", MemoryScope::Project, &sid))
            .unwrap();

        assert_eq!(store.list(MemoryScope::Project).unwrap().len(), 2);
        assert_eq!(store.list(MemoryScope::Decision).unwrap().len(), 1);
        assert_eq!(store.list(MemoryScope::Benchmark).unwrap().len(), 0);
    }

    #[test]
    fn list_for_session_and_count() {
        let store = InMemoryMemoryStore::try_default().unwrap();
        let s1 = test_session_id();
        let s2 = other_session_id();
        store
            .put(make_record("a", MemoryScope::Project, &s1))
            .unwrap();
        store
            .put(make_record("b", MemoryScope::Project, &s2))
            .unwrap();

        assert_eq!(store.list_for_session(&s1).unwrap().len(), 1);
        assert_eq!(store.count_for_session(&s1).unwrap(), 1);
        assert_eq!(store.count_for_session(&s2).unwrap(), 1);
    }

    #[test]
    fn list_all_returns_everything() {
        let store = InMemoryMemoryStore::try_default().unwrap();
        let sid = test_session_id();
        store
            .put(make_record("a", MemoryScope::Project, &sid))
            .unwrap();
        store
            .put(make_record("b", MemoryScope::Decision, &sid))
            .unwrap();
        assert_eq!(store.list_all().unwrap().len(), 2);
    }

    #[test]
    fn list_by_time_range_filters_correctly() {
        let store = InMemoryMemoryStore::try_default().unwrap();
        let sid = test_session_id();
        store
            .put(make_record("a", MemoryScope::Project, &sid))
            .unwrap();

        let now = Utc::now();
        let start = now - Duration::seconds(5);
        let end = now + Duration::seconds(5);
        assert_eq!(store.list_by_time_range(start, end).unwrap().len(), 1);

        // Range entirely in the past should return nothing
        let old_start = now - Duration::seconds(100);
        let old_end = now - Duration::seconds(50);
        assert_eq!(
            store.list_by_time_range(old_start, old_end).unwrap().len(),
            0
        );
    }

    #[test]
    fn empty_store_returns_empty() {
        let store = InMemoryMemoryStore::try_default().unwrap();
        let sid = test_session_id();
        assert!(store.list_for_session(&sid).unwrap().is_empty());
        assert_eq!(store.count_for_session(&sid).unwrap(), 0);
        assert!(store.list_all().unwrap().is_empty());
    }
}
