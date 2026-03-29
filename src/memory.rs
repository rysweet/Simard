use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::persistence::{load_json_or_default, persist_json};
use crate::session::{SessionId, SessionPhase};

const MEMORY_STORE_NAME: &str = "memory";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryScope {
    SessionScratch,
    SessionSummary,
    Decision,
    Project,
    Benchmark,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
}
