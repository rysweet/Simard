//! Tag encoding/decoding and fact-to-record conversion.

use crate::memory::{CognitiveMemoryType, MemoryRecord};
use crate::memory_cognitive::CognitiveFact;
use crate::session::{SessionId, SessionPhase};

/// Tag prefix used to encode cognitive memory type in tags.
pub(super) fn scope_tag(memory_type: CognitiveMemoryType) -> String {
    format!("scope:{memory_type:?}")
}

/// Tag prefix used to encode session ID in cognitive memory tags.
pub(super) fn session_tag(session_id: &SessionId) -> String {
    format!("session:{}", session_id.as_str())
}

/// Parse a cognitive memory type from a tag string like "scope:Semantic".
fn parse_scope_tag(tag: &str) -> Option<CognitiveMemoryType> {
    let suffix = tag.strip_prefix("scope:")?;
    match suffix {
        // New cognitive types
        "Sensory" => Some(CognitiveMemoryType::Sensory),
        "Working" => Some(CognitiveMemoryType::Working),
        "Episodic" => Some(CognitiveMemoryType::Episodic),
        "Semantic" => Some(CognitiveMemoryType::Semantic),
        "Procedural" => Some(CognitiveMemoryType::Procedural),
        "Prospective" => Some(CognitiveMemoryType::Prospective),
        // Legacy tag migration
        "SessionScratch" => Some(CognitiveMemoryType::Working),
        "SessionSummary" => Some(CognitiveMemoryType::Episodic),
        "Decision" => Some(CognitiveMemoryType::Semantic),
        "Project" => Some(CognitiveMemoryType::Semantic),
        "Benchmark" => Some(CognitiveMemoryType::Procedural),
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
    let memory_type = fact
        .tags
        .iter()
        .find_map(|t| parse_scope_tag(t))
        .unwrap_or(CognitiveMemoryType::Semantic);
    let session_id = fact
        .tags
        .iter()
        .find_map(|t| parse_session_tag(t))
        .unwrap_or_else(|| SessionId::from_uuid(uuid::Uuid::nil()));
    MemoryRecord {
        key: fact.concept.clone(),
        memory_type,
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
        assert_eq!(record.memory_type, CognitiveMemoryType::Procedural);
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
        assert_eq!(record.memory_type, CognitiveMemoryType::Semantic); // default
    }
}
