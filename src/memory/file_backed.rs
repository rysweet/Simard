use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};

use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::persistence::{load_json_or_default, persist_json};
use crate::session::SessionId;

use super::store::MemoryStore;
use super::types::{MEMORY_STORE_NAME, MemoryRecord, MemoryScope};

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
            .filter(|r| {
                r.created_at
                    .map(|t| t >= start && t < end)
                    .unwrap_or(false)
            })
            .cloned()
            .collect())
    }
}
