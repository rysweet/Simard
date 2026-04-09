//! Validation helpers and public API for research tracking operations.

use serde_json::json;

use crate::error::{SimardError, SimardResult};
use crate::memory_bridge::CognitiveMemoryBridge;

use super::types::{DeveloperWatch, ResearchStatus, ResearchTopic};

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn required_field(field: &str, value: &str) -> SimardResult<()> {
    if value.trim().is_empty() {
        return Err(SimardError::InvalidResearchRecord {
            field: field.to_string(),
            reason: "value cannot be empty".to_string(),
        });
    }
    Ok(())
}

pub(super) fn validate_topic(topic: &ResearchTopic) -> SimardResult<()> {
    required_field("research_topic.id", &topic.id)?;
    required_field("research_topic.title", &topic.title)?;
    required_field("research_topic.source", &topic.source)?;
    if topic.priority == 0 {
        return Err(SimardError::InvalidResearchRecord {
            field: "research_topic.priority".to_string(),
            reason: "priority must be at least 1".to_string(),
        });
    }
    Ok(())
}

pub(super) fn validate_watch(watch: &DeveloperWatch) -> SimardResult<()> {
    required_field("developer_watch.github_id", &watch.github_id)?;
    if watch.focus_areas.is_empty() {
        return Err(SimardError::InvalidResearchRecord {
            field: "developer_watch.focus_areas".to_string(),
            reason: "at least one focus area is required".to_string(),
        });
    }
    for area in &watch.focus_areas {
        required_field("developer_watch.focus_areas[]", area)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Add a research topic to the tracker and store it as a semantic fact.
pub fn add_research_topic(
    topic: ResearchTopic,
    bridge: &CognitiveMemoryBridge,
) -> SimardResult<()> {
    validate_topic(&topic)?;

    bridge.store_fact(
        &format!("research:{}", topic.id),
        &format!(
            "title={}; source={}; priority={}; status={}",
            topic.title, topic.source, topic.priority, topic.status
        ),
        0.8,
        &["research".to_string(), topic.source.clone()],
        "research-tracker",
    )?;

    bridge.store_episode(
        &format!("Research topic added: {}", topic.concise_label()),
        "research-tracker",
        Some(&json!({"topic_id": topic.id})),
    )?;

    Ok(())
}

/// Track a developer's public activity and store as a semantic fact.
pub fn track_developer(watch: DeveloperWatch, bridge: &CognitiveMemoryBridge) -> SimardResult<()> {
    validate_watch(&watch)?;

    let areas = watch.focus_areas.join(", ");
    bridge.store_fact(
        &format!("dev-watch:{}", watch.github_id),
        &format!("github_id={}; focus_areas={areas}", watch.github_id),
        0.7,
        &["developer-watch".to_string()],
        "research-tracker",
    )?;

    Ok(())
}

/// Update the status of a research topic by its id.
pub fn update_topic_status(
    topic_id: &str,
    new_status: ResearchStatus,
    bridge: &CognitiveMemoryBridge,
) -> SimardResult<()> {
    required_field("topic_id", topic_id)?;

    bridge.store_fact(
        &format!("research:{topic_id}:status"),
        &format!("status={new_status}"),
        0.8,
        &["research".to_string(), "status-update".to_string()],
        "research-tracker",
    )?;

    bridge.store_episode(
        &format!("Research topic '{topic_id}' status changed to {new_status}"),
        "research-tracker",
        Some(&json!({"topic_id": topic_id, "new_status": new_status.to_string()})),
    )?;

    Ok(())
}

/// Load tracked research topics from cognitive memory.
pub fn load_research_topics(bridge: &CognitiveMemoryBridge) -> SimardResult<Vec<ResearchTopic>> {
    let facts = bridge.search_facts("research:", 50, 0.0)?;
    let mut topics = Vec::new();
    for fact in facts {
        if fact.concept.starts_with("research:")
            && !fact.concept.contains(":status")
            && fact.content.contains("title=")
            && let Some(topic) = parse_topic_from_fact(&fact.concept, &fact.content)
        {
            topics.push(topic);
        }
    }
    Ok(topics)
}

pub(super) fn parse_topic_from_fact(concept: &str, content: &str) -> Option<ResearchTopic> {
    let id = concept.strip_prefix("research:")?;
    let title = extract_field(content, "title=")?;
    let source = extract_field(content, "source=")?;
    let priority_str = extract_field(content, "priority=")?;
    let priority = priority_str.parse::<u32>().ok()?;
    let status_str = extract_field(content, "status=")?;
    let status = match status_str.as_str() {
        "proposed" => ResearchStatus::Proposed,
        "in-progress" => ResearchStatus::InProgress,
        "completed" => ResearchStatus::Completed,
        "archived" => ResearchStatus::Archived,
        _ => return None,
    };
    Some(ResearchTopic {
        id: id.to_string(),
        title,
        source,
        priority,
        status,
    })
}

fn extract_field(content: &str, prefix: &str) -> Option<String> {
    let start = content.find(prefix)?;
    let value_start = start + prefix.len();
    let rest = &content[value_start..];
    let end = rest.find("; ").unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_topic(id: &str, title: &str, source: &str, priority: u32) -> ResearchTopic {
        ResearchTopic {
            id: id.to_string(),
            title: title.to_string(),
            source: source.to_string(),
            priority,
            status: ResearchStatus::Proposed,
        }
    }

    fn make_watch(github_id: &str, areas: Vec<&str>) -> DeveloperWatch {
        DeveloperWatch {
            github_id: github_id.to_string(),
            focus_areas: areas.into_iter().map(String::from).collect(),
            last_checked: None,
        }
    }

    // -- validate_topic --

    #[test]
    fn validate_topic_accepts_valid_topic() {
        let topic = make_topic("t1", "Rust async", "arxiv", 1);
        assert!(validate_topic(&topic).is_ok());
    }

    #[test]
    fn validate_topic_rejects_empty_id() {
        let topic = make_topic("", "Title", "source", 1);
        let err = validate_topic(&topic).unwrap_err();
        assert!(err.to_string().contains("empty"), "{err}");
    }

    #[test]
    fn validate_topic_rejects_whitespace_title() {
        let topic = make_topic("t1", "   ", "source", 1);
        assert!(validate_topic(&topic).is_err());
    }

    #[test]
    fn validate_topic_rejects_empty_source() {
        let topic = make_topic("t1", "Title", "", 1);
        assert!(validate_topic(&topic).is_err());
    }

    #[test]
    fn validate_topic_rejects_zero_priority() {
        let topic = make_topic("t1", "Title", "src", 0);
        let err = validate_topic(&topic).unwrap_err();
        assert!(err.to_string().contains("priority"), "{err}");
    }

    // -- validate_watch --

    #[test]
    fn validate_watch_accepts_valid_watch() {
        let watch = make_watch("user42", vec!["llm", "agents"]);
        assert!(validate_watch(&watch).is_ok());
    }

    #[test]
    fn validate_watch_rejects_empty_github_id() {
        let watch = make_watch("", vec!["area"]);
        assert!(validate_watch(&watch).is_err());
    }

    #[test]
    fn validate_watch_rejects_empty_focus_areas() {
        let watch = make_watch("user42", vec![]);
        let err = validate_watch(&watch).unwrap_err();
        assert!(err.to_string().contains("focus"), "{err}");
    }

    #[test]
    fn validate_watch_rejects_blank_focus_area_entry() {
        let watch = make_watch("user42", vec!["ok", "  "]);
        assert!(validate_watch(&watch).is_err());
    }

    // -- parse_topic_from_fact --

    #[test]
    fn parse_topic_from_fact_roundtrips_proposed() {
        let concept = "research:abc";
        let content = "title=My Topic; source=arxiv; priority=3; status=proposed";
        let topic = parse_topic_from_fact(concept, content).unwrap();
        assert_eq!(topic.id, "abc");
        assert_eq!(topic.title, "My Topic");
        assert_eq!(topic.source, "arxiv");
        assert_eq!(topic.priority, 3);
        assert_eq!(topic.status, ResearchStatus::Proposed);
    }

    #[test]
    fn parse_topic_from_fact_parses_in_progress() {
        let concept = "research:x";
        let content = "title=T; source=S; priority=1; status=in-progress";
        let topic = parse_topic_from_fact(concept, content).unwrap();
        assert_eq!(topic.status, ResearchStatus::InProgress);
    }

    #[test]
    fn parse_topic_from_fact_parses_completed() {
        let content = "title=T; source=S; priority=1; status=completed";
        let topic = parse_topic_from_fact("research:z", content).unwrap();
        assert_eq!(topic.status, ResearchStatus::Completed);
    }

    #[test]
    fn parse_topic_from_fact_parses_archived() {
        let content = "title=T; source=S; priority=1; status=archived";
        let topic = parse_topic_from_fact("research:z", content).unwrap();
        assert_eq!(topic.status, ResearchStatus::Archived);
    }

    #[test]
    fn parse_topic_from_fact_returns_none_for_bad_concept_prefix() {
        let content = "title=T; source=S; priority=1; status=proposed";
        assert!(parse_topic_from_fact("other:x", content).is_none());
    }

    #[test]
    fn parse_topic_from_fact_returns_none_for_unknown_status() {
        let content = "title=T; source=S; priority=1; status=unknown";
        assert!(parse_topic_from_fact("research:x", content).is_none());
    }

    #[test]
    fn parse_topic_from_fact_returns_none_for_missing_title() {
        let content = "source=S; priority=1; status=proposed";
        assert!(parse_topic_from_fact("research:x", content).is_none());
    }

    #[test]
    fn parse_topic_from_fact_returns_none_for_bad_priority() {
        let content = "title=T; source=S; priority=abc; status=proposed";
        assert!(parse_topic_from_fact("research:x", content).is_none());
    }

    // -- extract_field --

    #[test]
    fn extract_field_finds_first_field() {
        let content = "title=Hello World; source=arxiv";
        assert_eq!(extract_field(content, "title=").unwrap(), "Hello World");
    }

    #[test]
    fn extract_field_finds_last_field_without_trailing_separator() {
        let content = "title=Hello; source=arxiv";
        assert_eq!(extract_field(content, "source=").unwrap(), "arxiv");
    }

    #[test]
    fn extract_field_returns_none_for_missing_prefix() {
        assert!(extract_field("title=X", "source=").is_none());
    }

    // -- required_field --

    #[test]
    fn required_field_ok_for_nonempty() {
        assert!(required_field("f", "value").is_ok());
    }

    #[test]
    fn required_field_err_for_empty() {
        assert!(required_field("f", "").is_err());
    }

    #[test]
    fn required_field_err_for_whitespace() {
        assert!(required_field("f", "   ").is_err());
    }
}
