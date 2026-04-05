//! Core types for the goal board: goals, backlog items, and board state.

use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

/// Maximum number of concurrently active goals.
pub const MAX_ACTIVE_GOALS: usize = 5;

/// Progress state for an active goal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GoalProgress {
    NotStarted,
    InProgress { percent: u32 },
    Blocked(String),
    Completed,
}

impl Display for GoalProgress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotStarted => f.write_str("not-started"),
            Self::InProgress { percent } => write!(f, "in-progress({percent}%)"),
            Self::Blocked(reason) => write!(f, "blocked: {reason}"),
            Self::Completed => f.write_str("completed"),
        }
    }
}

/// An active goal on the board. Active goals are limited to `MAX_ACTIVE_GOALS`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActiveGoal {
    pub id: String,
    pub description: String,
    pub priority: u32,
    pub status: GoalProgress,
    pub assigned_to: Option<String>,
}

impl ActiveGoal {
    /// Short label for display.
    pub fn concise_label(&self) -> String {
        format!("p{} [{}] {}", self.priority, self.status, self.description)
    }
}

/// A backlog item scored for future promotion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BacklogItem {
    pub id: String,
    pub description: String,
    pub source: String,
    pub score: f64,
}

/// The goal board: active goals + scored backlog.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoalBoard {
    pub active: Vec<ActiveGoal>,
    pub backlog: Vec<BacklogItem>,
}

impl GoalBoard {
    /// Create an empty goal board.
    pub fn new() -> Self {
        Self {
            active: Vec::new(),
            backlog: Vec::new(),
        }
    }

    /// How many active goal slots remain.
    pub fn active_slots_remaining(&self) -> usize {
        MAX_ACTIVE_GOALS.saturating_sub(self.active.len())
    }

    /// Render a durable summary of the board state.
    pub fn durable_summary(&self) -> String {
        let active_labels: Vec<String> = self.active.iter().map(|g| g.concise_label()).collect();
        let active_text = if active_labels.is_empty() {
            "none".to_string()
        } else {
            active_labels.join("; ")
        };
        let backlog_text = if self.backlog.is_empty() {
            "none".to_string()
        } else {
            format!("{} items", self.backlog.len())
        };
        format!("active=[{active_text}]; backlog={backlog_text}")
    }
}

impl Default for GoalBoard {
    fn default() -> Self {
        Self::new()
    }
}
