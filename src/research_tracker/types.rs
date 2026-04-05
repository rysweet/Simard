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
