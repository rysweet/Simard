//! Research topic and developer tracking.
//!
//! Tracks research topics surfaced during meetings, goal curation, or
//! engineering work. Also maintains a watch list of developers whose public
//! activity is relevant to Simard's focus areas.

mod operations;
mod types;
mod watches;

// Re-export all public items so `crate::research_tracker::X` still works.
pub use operations::{
    add_research_topic, load_research_topics, track_developer, update_topic_status,
};
pub use types::{DeveloperWatch, ResearchStatus, ResearchTopic, ResearchTracker};
pub use watches::{DEFAULT_DEVELOPER_WATCHES, default_developer_watches, seed_developer_watches};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::memory_bridge::CognitiveMemoryBridge;
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
        let topic = operations::parse_topic_from_fact(
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
