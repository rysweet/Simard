use std::sync::Mutex;

use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::session::SessionId;

use super::store::MemoryStore;
use super::types::{CognitiveMemoryType, MemoryRecord};

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
        self.records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "memory".to_string(),
            })?
            .push(record);
        Ok(())
    }

    fn list(&self, memory_type: CognitiveMemoryType) -> SimardResult<Vec<MemoryRecord>> {
        Ok(self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "memory".to_string(),
            })?
            .iter()
            .filter(|record| record.memory_type == memory_type)
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
}
