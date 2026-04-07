use chrono::{DateTime, Utc};

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

    /// Return all records across every scope and session.
    /// Enables cross-session recall and memory consolidation.
    fn list_all(&self) -> SimardResult<Vec<MemoryRecord>>;

    /// Return records whose `created_at` falls within [start, end).
    /// Records without a timestamp are excluded.
    fn list_by_time_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> SimardResult<Vec<MemoryRecord>>;

    /// Return records matching the given scope from ALL sessions.
    /// Default implementation delegates to `list()` since most stores
    /// already return cross-session data.
    fn list_by_scope_across_sessions(
        &self,
        scope: MemoryScope,
    ) -> SimardResult<Vec<MemoryRecord>> {
        self.list(scope)
    }
}
