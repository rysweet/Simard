//! Research topic and developer tracking.
//!
//! Tracks research topics surfaced during meetings, goal curation, or
//! engineering work. Also maintains a watch list of developers whose public
//! activity is relevant to Simard's focus areas.

use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{SimardError, SimardResult};
use crate::memory_bridge::CognitiveMemoryBridge;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Lifecycle status of a research topic.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ResearchStatus {
    Proposed,
    InProgress,
    Completed,
    Archived,
}

impl Display for ResearchStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Proposed => f.write_str("proposed"),
            Self::InProgress => f.write_str("in-progress"),
            Self::Completed => f.write_str("completed"),
            Self::Archived => f.write_str("archived"),
        }
    }
}

/// A research topic tracked for investigation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResearchTopic {
    pub id: String,
    pub title: String,
    pub source: String,
    pub priority: u32,
    pub status: ResearchStatus,
}

impl ResearchTopic {
    /// Short label for display.
    pub fn concise_label(&self) -> String {
        format!("p{} [{}] {}", self.priority, self.status, self.title)
    }
}

/// A developer whose public activity is tracked.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeveloperWatch {
    pub github_id: String,
    pub focus_areas: Vec<String>,
    pub last_checked: Option<u64>,
}

/// Aggregated research tracker state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ResearchTracker {
    pub topics: Vec<ResearchTopic>,
    pub watches: Vec<DeveloperWatch>,
}

impl ResearchTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a tracker pre-populated with the default developer watch list.
    pub fn with_default_watches() -> Self {
        Self {
            topics: Vec::new(),
            watches: default_developer_watches(),
        }
    }

    /// Durable summary of tracker state.
    pub fn durable_summary(&self) -> String {
        let topics_text = if self.topics.is_empty() {
            "none".to_string()
        } else {
            self.topics
                .iter()
                .map(|t| t.concise_label())
                .collect::<Vec<_>>()
                .join("; ")
        };
        let watches_text = if self.watches.is_empty() {
            "none".to_string()
        } else {
            self.watches
                .iter()
                .map(|w| w.github_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!("research topics=[{topics_text}]; watches=[{watches_text}]")
    }
}

// ---------------------------------------------------------------------------
// Default developer watch list
// ---------------------------------------------------------------------------

/// Developers whose public activity Simard tracks by default.
/// Each tuple: (github_id, focus_areas).
pub const DEFAULT_DEVELOPER_WATCHES: [(&str, &[&str]); 5] = [
    (
        "ramparte",
        &["agentic-coding", "agent-frameworks", "developer-tools"],
    ),
    (
        "simonw",
        &["llm-tooling", "sqlite", "datasette", "prompt-engineering"],
    ),
    (
        "steveyegge",
        &["ai-coding", "developer-experience", "platform-engineering"],
    ),
    (
        "bkrabach",
        &["multi-agent-systems", "azure-ai", "agent-orchestration"],
    ),
    (
        "robotdad",
        &[
            "rust-tooling",
            "systems-programming",
            "developer-productivity",
        ],
    ),
];

/// Build the default developer watch list from the compile-time constant.
pub fn default_developer_watches() -> Vec<DeveloperWatch> {
    DEFAULT_DEVELOPER_WATCHES
        .iter()
        .map(|(github_id, areas)| DeveloperWatch {
            github_id: (*github_id).to_string(),
            focus_areas: areas.iter().map(|a| (*a).to_string()).collect(),
            last_checked: None,
        })
        .collect()
}

/// Seed the default developer watches into cognitive memory if not already
/// tracked. Returns the number of watches stored.
pub fn seed_developer_watches(bridge: &CognitiveMemoryBridge) -> usize {
    let mut seeded = 0;
    for watch in default_developer_watches() {
        if track_developer(watch, bridge).is_ok() {
            seeded += 1;
        }
    }
    seeded
}

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

fn validate_topic(topic: &ResearchTopic) -> SimardResult<()> {
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

fn validate_watch(watch: &DeveloperWatch) -> SimardResult<()> {
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
        // Only parse facts whose concept starts with "research:" and whose
        // content has the expected structured fields.
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

fn parse_topic_from_fact(concept: &str, content: &str) -> Option<ResearchTopic> {
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
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use serde_json::json;

    fn mock_bridge() -> CognitiveMemoryBridge {
        let transport =
            InMemoryBridgeTransport::new("test-research", |method, _params| match method {
                "memory.store_fact" => Ok(json!({"id": "sem_r1"})),
                "memory.store_episode" => Ok(json!({"id": "epi_r1"})),
                "memory.search_facts" => Ok(json!({"facts": []})),
                _ => Err(crate::bridge::BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown method: {method}"),
                }),
            });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    #[test]
    fn add_and_track_research_topic() {
        let bridge = mock_bridge();
        add_research_topic(
            ResearchTopic {
                id: "rt-1".to_string(),
                title: "Memory consolidation strategies".to_string(),
                source: "meeting".to_string(),
                priority: 2,
                status: ResearchStatus::Proposed,
            },
            &bridge,
        )
        .unwrap();
    }

    #[test]
    fn track_developer_watch() {
        let bridge = mock_bridge();
        track_developer(
            DeveloperWatch {
                github_id: "octocat".to_string(),
                focus_areas: vec!["agent-frameworks".to_string()],
                last_checked: None,
            },
            &bridge,
        )
        .unwrap();
    }

    #[test]
    fn rejects_empty_topic_id() {
        let bridge = mock_bridge();
        let err = add_research_topic(
            ResearchTopic {
                id: "".to_string(),
                title: "Something".to_string(),
                source: "test".to_string(),
                priority: 1,
                status: ResearchStatus::Proposed,
            },
            &bridge,
        )
        .unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn rejects_watch_without_focus_areas() {
        let bridge = mock_bridge();
        let err = track_developer(
            DeveloperWatch {
                github_id: "someone".to_string(),
                focus_areas: vec![],
                last_checked: None,
            },
            &bridge,
        )
        .unwrap_err();
        assert!(err.to_string().contains("focus area"));
    }

    #[test]
    fn parse_topic_from_fact_round_trip() {
        let topic = parse_topic_from_fact(
            "research:rt-1",
            "title=Memory consolidation; source=meeting; priority=2; status=proposed",
        );
        assert!(topic.is_some());
        let topic = topic.unwrap();
        assert_eq!(topic.id, "rt-1");
        assert_eq!(topic.title, "Memory consolidation");
        assert_eq!(topic.priority, 2);
        assert_eq!(topic.status, ResearchStatus::Proposed);
    }

    #[test]
    fn default_developer_watches_contains_five_developers() {
        let watches = default_developer_watches();
        assert_eq!(watches.len(), 5);
        let ids: Vec<&str> = watches.iter().map(|w| w.github_id.as_str()).collect();
        assert!(ids.contains(&"ramparte"));
        assert!(ids.contains(&"simonw"));
        assert!(ids.contains(&"steveyegge"));
        assert!(ids.contains(&"bkrabach"));
        assert!(ids.contains(&"robotdad"));
    }

    #[test]
    fn default_watches_have_focus_areas() {
        for watch in default_developer_watches() {
            assert!(
                !watch.focus_areas.is_empty(),
                "{} should have focus areas",
                watch.github_id
            );
        }
    }

    #[test]
    fn with_default_watches_pre_populates_tracker() {
        let tracker = ResearchTracker::with_default_watches();
        assert_eq!(tracker.watches.len(), 5);
        assert!(tracker.topics.is_empty());
    }

    #[test]
    fn seed_developer_watches_stores_all_five() {
        let bridge = mock_bridge();
        let seeded = seed_developer_watches(&bridge);
        assert_eq!(seeded, 5);
    }

    #[test]
    fn tracker_summary_includes_developer_ids() {
        let tracker = ResearchTracker::with_default_watches();
        let summary = tracker.durable_summary();
        assert!(summary.contains("ramparte"));
        assert!(summary.contains("simonw"));
        assert!(summary.contains("steveyegge"));
    }
}
