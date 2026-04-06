use crate::error::SimardResult;
use crate::metadata::BackendDescriptor;
use crate::session::SessionId;

use super::types::{MemoryRecord, MemoryScope};

pub trait MemoryStore: Send + Sync {
    fn descriptor(&self) -> BackendDescriptor;

    fn put(&self, record: MemoryRecord) -> SimardResult<()>;

    fn list(&self, scope: MemoryScope) -> SimardResult<Vec<MemoryRecord>>;

    fn list_for_session(&self, session_id: &SessionId) -> SimardResult<Vec<MemoryRecord>>;

    fn count_for_session(&self, session_id: &SessionId) -> SimardResult<usize>;
}
