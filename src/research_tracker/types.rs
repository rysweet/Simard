//! Core data types for research tracking.

use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

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
            watches: super::watches::default_developer_watches(),
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── ResearchStatus ──────────────────────────────────────────────

    #[test]
    fn research_status_display_all_variants() {
        assert_eq!(ResearchStatus::Proposed.to_string(), "proposed");
        assert_eq!(ResearchStatus::InProgress.to_string(), "in-progress");
        assert_eq!(ResearchStatus::Completed.to_string(), "completed");
        assert_eq!(ResearchStatus::Archived.to_string(), "archived");
    }

    #[test]
    fn research_status_serde_round_trip() {
        for status in [
            ResearchStatus::Proposed,
            ResearchStatus::InProgress,
            ResearchStatus::Completed,
            ResearchStatus::Archived,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let s2: ResearchStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, s2);
        }
    }

    // ── ResearchTopic ───────────────────────────────────────────────

    fn sample_topic() -> ResearchTopic {
        ResearchTopic {
            id: "rt-1".to_string(),
            title: "LLM Agents".to_string(),
            source: "paper".to_string(),
            priority: 2,
            status: ResearchStatus::InProgress,
        }
    }

    #[test]
    fn research_topic_serde_round_trip() {
        let t = sample_topic();
        let json = serde_json::to_string(&t).unwrap();
        let t2: ResearchTopic = serde_json::from_str(&json).unwrap();
        assert_eq!(t, t2);
    }

    #[test]
    fn concise_label_contains_priority_status_title() {
        let t = sample_topic();
        let label = t.concise_label();
        assert!(label.contains("p2"));
        assert!(label.contains("in-progress"));
        assert!(label.contains("LLM Agents"));
    }

    #[test]
    fn concise_label_zero_priority() {
        let t = ResearchTopic {
            id: "rt-0".to_string(),
            title: "Low".to_string(),
            source: "internal".to_string(),
            priority: 0,
            status: ResearchStatus::Proposed,
        };
        assert!(t.concise_label().starts_with("p0"));
    }

    // ── DeveloperWatch ──────────────────────────────────────────────

    #[test]
    fn developer_watch_serde_round_trip() {
        let w = DeveloperWatch {
            github_id: "octocat".to_string(),
            focus_areas: vec!["rust".to_string(), "wasm".to_string()],
            last_checked: Some(1234567890),
        };
        let json = serde_json::to_string(&w).unwrap();
        let w2: DeveloperWatch = serde_json::from_str(&json).unwrap();
        assert_eq!(w, w2);
    }

    #[test]
    fn developer_watch_last_checked_none() {
        let w = DeveloperWatch {
            github_id: "dev".to_string(),
            focus_areas: vec![],
            last_checked: None,
        };
        let json = serde_json::to_string(&w).unwrap();
        let w2: DeveloperWatch = serde_json::from_str(&json).unwrap();
        assert_eq!(w2.last_checked, None);
    }

    #[test]
    fn developer_watch_empty_focus_areas() {
        let w = DeveloperWatch {
            github_id: "solo".to_string(),
            focus_areas: vec![],
            last_checked: None,
        };
        assert!(w.focus_areas.is_empty());
    }

    // ── ResearchTracker ─────────────────────────────────────────────

    #[test]
    fn tracker_default_is_empty() {
        let t = ResearchTracker::default();
        assert!(t.topics.is_empty());
        assert!(t.watches.is_empty());
    }

    #[test]
    fn tracker_new_equals_default() {
        assert_eq!(ResearchTracker::new(), ResearchTracker::default());
    }

    #[test]
    fn tracker_with_default_watches_has_watches() {
        let t = ResearchTracker::with_default_watches();
        assert!(t.topics.is_empty());
        assert!(!t.watches.is_empty());
    }

    #[test]
    fn tracker_serde_round_trip() {
        let t = ResearchTracker {
            topics: vec![sample_topic()],
            watches: vec![DeveloperWatch {
                github_id: "dev".to_string(),
                focus_areas: vec!["ai".to_string()],
                last_checked: Some(100),
            }],
        };
        let json = serde_json::to_string(&t).unwrap();
        let t2: ResearchTracker = serde_json::from_str(&json).unwrap();
        assert_eq!(t, t2);
    }

    #[test]
    fn durable_summary_empty_tracker() {
        let t = ResearchTracker::new();
        let s = t.durable_summary();
        assert!(s.contains("topics=[none]"));
        assert!(s.contains("watches=[none]"));
    }

    #[test]
    fn durable_summary_with_topics_and_watches() {
        let t = ResearchTracker {
            topics: vec![sample_topic()],
            watches: vec![DeveloperWatch {
                github_id: "octocat".to_string(),
                focus_areas: vec![],
                last_checked: None,
            }],
        };
        let s = t.durable_summary();
        assert!(s.contains("LLM Agents"));
        assert!(s.contains("octocat"));
    }

    #[test]
    fn durable_summary_multiple_topics_semicolon_separated() {
        let t = ResearchTracker {
            topics: vec![
                ResearchTopic {
                    id: "1".to_string(),
                    title: "Alpha".to_string(),
                    source: "s".to_string(),
                    priority: 1,
                    status: ResearchStatus::Proposed,
                },
                ResearchTopic {
                    id: "2".to_string(),
                    title: "Beta".to_string(),
                    source: "s".to_string(),
                    priority: 2,
                    status: ResearchStatus::Completed,
                },
            ],
            watches: vec![],
        };
        let s = t.durable_summary();
        assert!(s.contains("Alpha"));
        assert!(s.contains("Beta"));
        assert!(s.contains("; "));
    }
}
