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
    fn list_by_scope_across_sessions(&self, scope: MemoryScope) -> SimardResult<Vec<MemoryRecord>> {
        self.list(scope)
    }

    /// Retry any pending bridge writes that failed during normal operation.
    /// Returns the number of records successfully synced.
    /// Default: no-op (only `CognitiveBridgeMemoryStore` has pending writes).
    fn flush_pending(&self) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::Provenance;

    // A minimal MemoryStore implementation for testing default methods.
    struct StubStore;

    impl MemoryStore for StubStore {
        fn descriptor(&self) -> BackendDescriptor {
            BackendDescriptor::new(
                "stub",
                Provenance::builtin("test"),
                crate::metadata::Freshness::now().unwrap(),
            )
        }

        fn put(&self, _record: MemoryRecord) -> SimardResult<()> {
            Ok(())
        }

        fn list(&self, _scope: MemoryScope) -> SimardResult<Vec<MemoryRecord>> {
            Ok(vec![])
        }

        fn list_for_session(&self, _id: &SessionId) -> SimardResult<Vec<MemoryRecord>> {
            Ok(vec![])
        }

        fn count_for_session(&self, _id: &SessionId) -> SimardResult<usize> {
            Ok(0)
        }

        fn list_all(&self) -> SimardResult<Vec<MemoryRecord>> {
            Ok(vec![])
        }

        fn list_by_time_range(
            &self,
            _start: DateTime<Utc>,
            _end: DateTime<Utc>,
        ) -> SimardResult<Vec<MemoryRecord>> {
            Ok(vec![])
        }
    }

    #[test]
    fn flush_pending_default_returns_zero() {
        let store = StubStore;
        assert_eq!(store.flush_pending(), 0);
    }

    #[test]
    fn list_by_scope_across_sessions_delegates_to_list() {
        let store = StubStore;
        let result = store
            .list_by_scope_across_sessions(MemoryScope::Project)
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn stub_store_descriptor_identity() {
        let store = StubStore;
        assert_eq!(store.descriptor().identity, "stub");
    }
}
