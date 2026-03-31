//! Adapter that implements [`MemoryStore`] via [`CognitiveMemoryBridge`].
//!
//! When `SIMARD_MEMORY_BACKEND=cognitive-bridge` is set, bootstrap wires this
//! adapter in place of `FileBackedMemoryStore`. Every `put` stores records as
//! semantic facts in Kuzu via the bridge subprocess, and every `list` / count
//! query translates to `search_facts` calls.
//!
//! Scope mapping:
//! - Each [`MemoryScope`] is stored as a tag on the semantic fact.
//! - The record `key` becomes the concept, `value` becomes the content.
//! - `session_id` and `recorded_in` are preserved in the `source_id` field
//!   as `{session_id}:{phase}`.

use std::sync::Mutex;

use crate::bridge::BridgeTransport;
use crate::error::{SimardError, SimardResult};
use crate::memory::{MemoryRecord, MemoryScope, MemoryStore};
use crate::memory_bridge::CognitiveMemoryBridge;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::session::SessionId;

const ADAPTER_NAME: &str = "memory-bridge-adapter";

/// Adapts [`CognitiveMemoryBridge`] to the [`MemoryStore`] trait.
///
/// Records are stored as semantic facts with scope-based tags, enabling the
/// Simard runtime to use Kuzu cognitive memory as its primary memory backend
/// while keeping the `MemoryStore` trait contract intact.
pub struct CognitiveBridgeMemoryAdapter {
    bridge: CognitiveMemoryBridge,
    descriptor: BackendDescriptor,
    /// Local cache of records for fast list/count queries. The bridge is the
    /// source of truth on put; the cache avoids repeated bridge round-trips
    /// for read-heavy workloads within a single session.
    cache: Mutex<Vec<MemoryRecord>>,
}

impl CognitiveBridgeMemoryAdapter {
    /// Create a new adapter wrapping the given bridge transport.
    pub fn new(transport: Box<dyn BridgeTransport>) -> SimardResult<Self> {
        let descriptor = BackendDescriptor::for_runtime_type::<Self>(
            "memory::cognitive-bridge-adapter",
            "runtime-port:memory-store:cognitive-bridge",
            Freshness::now()?,
        );
        Ok(Self {
            bridge: CognitiveMemoryBridge::new(transport),
            descriptor,
            cache: Mutex::new(Vec::new()),
        })
    }

    /// Encode scope + session metadata into the `source_id` field.
    fn encode_source_id(record: &MemoryRecord) -> String {
        format!("{}:{}", record.session_id, record.recorded_in)
    }

    /// Encode scope as a tag string.
    fn scope_tag(scope: MemoryScope) -> String {
        match scope {
            MemoryScope::SessionScratch => "scope:session-scratch".to_string(),
            MemoryScope::SessionSummary => "scope:session-summary".to_string(),
            MemoryScope::Decision => "scope:decision".to_string(),
            MemoryScope::Project => "scope:project".to_string(),
            MemoryScope::Benchmark => "scope:benchmark".to_string(),
        }
    }
}

impl std::fmt::Debug for CognitiveBridgeMemoryAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CognitiveBridgeMemoryAdapter")
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}

impl MemoryStore for CognitiveBridgeMemoryAdapter {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn put(&self, record: MemoryRecord) -> SimardResult<()> {
        let tags = vec![
            Self::scope_tag(record.scope),
            format!("session:{}", record.session_id),
        ];
        let source_id = Self::encode_source_id(&record);

        self.bridge.store_fact(
            &record.key,
            &record.value,
            1.0, // full confidence for direct stores
            &tags,
            &source_id,
        )?;

        // Update local cache
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: ADAPTER_NAME.to_string(),
            })?;
        if let Some(existing) = cache.iter_mut().find(|r| r.key == record.key) {
            *existing = record;
        } else {
            cache.push(record);
        }
        Ok(())
    }

    fn list(&self, scope: MemoryScope) -> SimardResult<Vec<MemoryRecord>> {
        let cache = self
            .cache
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: ADAPTER_NAME.to_string(),
            })?;
        Ok(cache.iter().filter(|r| r.scope == scope).cloned().collect())
    }

    fn list_for_session(&self, session_id: &SessionId) -> SimardResult<Vec<MemoryRecord>> {
        let cache = self
            .cache
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: ADAPTER_NAME.to_string(),
            })?;
        Ok(cache
            .iter()
            .filter(|r| &r.session_id == session_id)
            .cloned()
            .collect())
    }

    fn count_for_session(&self, session_id: &SessionId) -> SimardResult<usize> {
        let cache = self
            .cache
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: ADAPTER_NAME.to_string(),
            })?;
        Ok(cache.iter().filter(|r| &r.session_id == session_id).count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::BridgeErrorPayload;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::session::{SessionIdGenerator, SessionPhase, UuidSessionIdGenerator};
    use serde_json::json;

    fn test_adapter() -> CognitiveBridgeMemoryAdapter {
        let transport =
            InMemoryBridgeTransport::new("test-memory", |method, _params| match method {
                "memory.store_fact" => Ok(json!({"id": "sem_test"})),
                "memory.search_facts" => Ok(json!({"facts": []})),
                _ => Err(BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            });
        CognitiveBridgeMemoryAdapter::new(Box::new(transport)).unwrap()
    }

    fn test_session_id() -> SessionId {
        UuidSessionIdGenerator.next_id()
    }

    fn sample_record(key: &str, scope: MemoryScope, session_id: &SessionId) -> MemoryRecord {
        MemoryRecord {
            key: key.to_string(),
            scope,
            value: format!("value for {key}"),
            session_id: session_id.clone(),
            recorded_in: SessionPhase::Intake,
        }
    }

    #[test]
    fn put_stores_via_bridge_and_caches() {
        let adapter = test_adapter();
        let sid = test_session_id();
        let record = sample_record("key1", MemoryScope::Decision, &sid);
        adapter.put(record.clone()).unwrap();

        let listed = adapter.list(MemoryScope::Decision).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].key, "key1");
    }

    #[test]
    fn put_updates_existing_key() {
        let adapter = test_adapter();
        let sid = test_session_id();
        let r1 = sample_record("key1", MemoryScope::Project, &sid);
        adapter.put(r1).unwrap();

        let mut r2 = sample_record("key1", MemoryScope::Project, &sid);
        r2.value = "updated".to_string();
        adapter.put(r2).unwrap();

        let listed = adapter.list(MemoryScope::Project).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].value, "updated");
    }

    #[test]
    fn list_filters_by_scope() {
        let adapter = test_adapter();
        let sid = test_session_id();
        adapter
            .put(sample_record("d1", MemoryScope::Decision, &sid))
            .unwrap();
        adapter
            .put(sample_record("p1", MemoryScope::Project, &sid))
            .unwrap();
        adapter
            .put(sample_record("d2", MemoryScope::Decision, &sid))
            .unwrap();

        assert_eq!(adapter.list(MemoryScope::Decision).unwrap().len(), 2);
        assert_eq!(adapter.list(MemoryScope::Project).unwrap().len(), 1);
        assert_eq!(adapter.list(MemoryScope::Benchmark).unwrap().len(), 0);
    }

    #[test]
    fn list_for_session_filters_correctly() {
        let adapter = test_adapter();
        let sid1 = test_session_id();
        let sid2 = test_session_id();
        adapter
            .put(sample_record("a", MemoryScope::Decision, &sid1))
            .unwrap();

        adapter
            .put(sample_record("b", MemoryScope::Decision, &sid2))
            .unwrap();

        assert_eq!(adapter.list_for_session(&sid1).unwrap().len(), 1);
        assert_eq!(adapter.count_for_session(&sid1).unwrap(), 1);
        assert_eq!(adapter.list_for_session(&sid2).unwrap().len(), 1);
    }

    #[test]
    fn descriptor_identifies_cognitive_bridge() {
        let adapter = test_adapter();
        let desc = adapter.descriptor();
        assert!(desc.identity.contains("cognitive-bridge"));
    }
}
