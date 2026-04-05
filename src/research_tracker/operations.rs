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
