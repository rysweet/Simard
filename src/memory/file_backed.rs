use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::persistence::{load_json_or_default, persist_json};
use crate::session::SessionId;

use super::store::MemoryStore;
use super::types::{CognitiveMemoryType, MEMORY_STORE_NAME, MemoryRecord};

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
            records: Mutex::new(load_json_or_default(MEMORY_STORE_NAME, &path)?),
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
        persist_json(MEMORY_STORE_NAME, &self.path, &records)
    }
}

impl MemoryStore for FileBackedMemoryStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn put(&self, record: MemoryRecord) -> SimardResult<()> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: MEMORY_STORE_NAME.to_string(),
            })?;
        if let Some(existing) = records
            .iter_mut()
            .find(|existing| existing.key == record.key)
        {
            *existing = record;
        } else {
            records.push(record);
        }
        self.persist(&records)
    }

    fn list(&self, memory_type: CognitiveMemoryType) -> SimardResult<Vec<MemoryRecord>> {
        Ok(self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: MEMORY_STORE_NAME.to_string(),
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
}
