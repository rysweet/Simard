use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::session::{SessionId, SessionPhase};

pub(crate) const MEMORY_STORE_NAME: &str = "memory";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryScope {
    SessionScratch,
    SessionSummary,
    Decision,
    Project,
    Benchmark,
    /// Explicit marker for records recovered from cognitive memory where the
    /// original scope tag was missing or unparseable.
    Untagged,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub key: String,
    pub scope: MemoryScope,
    pub value: String,
    pub session_id: SessionId,
    pub recorded_in: SessionPhase,
    /// When this record was created. `None` for records deserialized from
    /// older files that predate this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn test_session_id() -> SessionId {
        SessionId::from_uuid(Uuid::nil())
    }

    #[test]
    fn memory_scope_serde_roundtrip() {
        for scope in [
            MemoryScope::SessionScratch,
            MemoryScope::SessionSummary,
            MemoryScope::Decision,
            MemoryScope::Project,
            MemoryScope::Benchmark,
            MemoryScope::Untagged,
        ] {
            let json = serde_json::to_string(&scope).unwrap();
            let back: MemoryScope = serde_json::from_str(&json).unwrap();
            assert_eq!(back, scope);
        }
    }

    #[test]
    fn memory_scope_kebab_case() {
        let json = serde_json::to_string(&MemoryScope::SessionScratch).unwrap();
        assert_eq!(json, "\"session-scratch\"");
    }

    #[test]
    fn memory_record_serde_roundtrip() {
        let record = MemoryRecord {
            key: "k1".to_string(),
            scope: MemoryScope::Decision,
            value: "v1".to_string(),
            session_id: test_session_id(),
            recorded_in: SessionPhase::Execution,
            created_at: None,
        };
        let json = serde_json::to_string(&record).unwrap();
        let back: MemoryRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, record);
    }

    #[test]
    fn memory_record_created_at_skipped_when_none() {
        let record = MemoryRecord {
            key: "k".to_string(),
            scope: MemoryScope::Project,
            value: "v".to_string(),
            session_id: test_session_id(),
            recorded_in: SessionPhase::Reflection,
            created_at: None,
        };
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("created_at"));
    }

    #[test]
    fn memory_store_name_constant() {
        assert_eq!(MEMORY_STORE_NAME, "memory");
    }
}
