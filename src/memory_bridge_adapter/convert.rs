//! Tag encoding/decoding and fact-to-record conversion.

use crate::memory::{MemoryRecord, MemoryScope};
use crate::memory_cognitive::CognitiveFact;
use crate::session::{SessionId, SessionPhase};

/// Tag prefix used to encode scope in cognitive memory tags.
pub(super) fn scope_tag(scope: MemoryScope) -> String {
    format!("scope:{scope:?}")
}

/// Tag prefix used to encode session ID in cognitive memory tags.
pub(super) fn session_tag(session_id: &SessionId) -> String {
    format!("session:{}", session_id.as_str())
}

/// Parse a scope from a tag string like "scope:Decision".
fn parse_scope_tag(tag: &str) -> Option<MemoryScope> {
    let suffix = tag.strip_prefix("scope:")?;
    match suffix {
        "SessionScratch" => Some(MemoryScope::SessionScratch),
        "SessionSummary" => Some(MemoryScope::SessionSummary),
        "Decision" => Some(MemoryScope::Decision),
        "Project" => Some(MemoryScope::Project),
        "Benchmark" => Some(MemoryScope::Benchmark),
        _ => None,
    }
}

/// Parse a session ID from a tag string like "session:<uuid>".
fn parse_session_tag(tag: &str) -> Option<SessionId> {
    let suffix = tag.strip_prefix("session:")?;
    uuid::Uuid::parse_str(suffix).ok().map(SessionId::from_uuid)
}

/// Convert a `CognitiveFact` back to a `MemoryRecord` by parsing encoded tags.
pub(super) fn fact_to_record(fact: &CognitiveFact) -> MemoryRecord {
    let scope = fact
        .tags
        .iter()
        .find_map(|t| parse_scope_tag(t))
        .unwrap_or(MemoryScope::Project);
    let session_id = fact
        .tags
        .iter()
        .find_map(|t| parse_session_tag(t))
        .unwrap_or_else(|| SessionId::from_uuid(uuid::Uuid::nil()));
    MemoryRecord {
        key: fact.concept.clone(),
        scope,
        value: fact.content.clone(),
        session_id,
        recorded_in: SessionPhase::Execution,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_cognitive::CognitiveFact;

    #[test]
    fn fact_to_record_parses_tags_correctly() {
        let fact = CognitiveFact {
            node_id: "n1".to_string(),
            concept: "test-concept".to_string(),
            content: "test-content".to_string(),
            confidence: 0.9,
            source_id: "test".to_string(),
            tags: vec![
                "scope:Benchmark".to_string(),
                "session:00000000-0000-0000-0000-000000000001".to_string(),
            ],
        };
        let record = fact_to_record(&fact);
        assert_eq!(record.key, "test-concept");
        assert_eq!(record.value, "test-content");
        assert_eq!(record.scope, MemoryScope::Benchmark);
    }

    #[test]
    fn fact_to_record_defaults_on_missing_tags() {
        let fact = CognitiveFact {
            node_id: "n2".to_string(),
            concept: "no-tags".to_string(),
            content: "content".to_string(),
            confidence: 0.5,
            source_id: "test".to_string(),
            tags: vec![],
        };
        let record = fact_to_record(&fact);
        assert_eq!(record.scope, MemoryScope::Project); // default
    }
}
