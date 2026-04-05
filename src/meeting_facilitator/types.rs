//! Core types for meeting sessions.

use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

/// A single decision recorded during a meeting.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MeetingDecision {
    pub description: String,
    pub rationale: String,
    pub participants: Vec<String>,
}

/// An action item assigned during a meeting.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActionItem {
    pub description: String,
    pub owner: String,
    pub priority: u32,
    pub due_description: Option<String>,
}

/// Status of an in-progress meeting.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MeetingSessionStatus {
    Open,
    Closed,
}

impl Display for MeetingSessionStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open => f.write_str("open"),
            Self::Closed => f.write_str("closed"),
        }
    }
}

/// A running or completed meeting session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MeetingSession {
    pub topic: String,
    pub decisions: Vec<MeetingDecision>,
    pub action_items: Vec<ActionItem>,
    pub notes: Vec<String>,
    pub status: MeetingSessionStatus,
}

impl MeetingSession {
    /// Render a concise durable summary suitable for memory storage.
    pub fn durable_summary(&self) -> String {
        let decisions = if self.decisions.is_empty() {
            "none".to_string()
        } else {
            self.decisions
                .iter()
                .map(|d| d.description.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        };
        let action_items = if self.action_items.is_empty() {
            "none".to_string()
        } else {
            self.action_items
                .iter()
                .map(|a| format!("{} (owner={})", a.description, a.owner))
                .collect::<Vec<_>>()
                .join("; ")
        };
        format!(
            "meeting topic={}; decisions=[{}]; action_items=[{}]",
            self.topic, decisions, action_items,
        )
    }
}
