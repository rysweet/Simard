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
        let active_labels: Vec<String> =
            self.active.iter().map(ActiveGoal::concise_label).collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── GoalProgress Display ────────────────────────────────────────

    #[test]
    fn goal_progress_display_not_started() {
        assert_eq!(GoalProgress::NotStarted.to_string(), "not-started");
    }

    #[test]
    fn goal_progress_display_in_progress() {
        let p = GoalProgress::InProgress { percent: 42 };
        assert_eq!(p.to_string(), "in-progress(42%)");
    }

    #[test]
    fn goal_progress_display_in_progress_zero() {
        let p = GoalProgress::InProgress { percent: 0 };
        assert_eq!(p.to_string(), "in-progress(0%)");
    }

    #[test]
    fn goal_progress_display_in_progress_hundred() {
        let p = GoalProgress::InProgress { percent: 100 };
        assert_eq!(p.to_string(), "in-progress(100%)");
    }

    #[test]
    fn goal_progress_display_blocked() {
        let p = GoalProgress::Blocked("waiting on review".to_string());
        assert_eq!(p.to_string(), "blocked: waiting on review");
    }

    #[test]
    fn goal_progress_display_completed() {
        assert_eq!(GoalProgress::Completed.to_string(), "completed");
    }

    // ── GoalProgress Serde ──────────────────────────────────────────

    #[test]
    fn goal_progress_serde_all_variants() {
        let variants = vec![
            GoalProgress::NotStarted,
            GoalProgress::InProgress { percent: 50 },
            GoalProgress::Blocked("reason".to_string()),
            GoalProgress::Completed,
        ];
        for v in variants {
            let json = serde_json::to_string(&v).unwrap();
            let v2: GoalProgress = serde_json::from_str(&json).unwrap();
            assert_eq!(v, v2);
        }
    }

    // ── ActiveGoal ──────────────────────────────────────────────────

    fn sample_goal() -> ActiveGoal {
        ActiveGoal {
            id: "g-1".to_string(),
            description: "Ship MVP".to_string(),
            priority: 1,
            status: GoalProgress::InProgress { percent: 75 },
            assigned_to: Some("team-a".to_string()),
        }
    }

    #[test]
    fn active_goal_serde_round_trip() {
        let g = sample_goal();
        let json = serde_json::to_string(&g).unwrap();
        let g2: ActiveGoal = serde_json::from_str(&json).unwrap();
        assert_eq!(g, g2);
    }

    #[test]
    fn active_goal_concise_label() {
        let g = sample_goal();
        let label = g.concise_label();
        assert!(label.contains("p1"));
        assert!(label.contains("in-progress(75%)"));
        assert!(label.contains("Ship MVP"));
    }

    #[test]
    fn active_goal_assigned_to_none() {
        let g = ActiveGoal {
            id: "g-2".to_string(),
            description: "Unassigned".to_string(),
            priority: 3,
            status: GoalProgress::NotStarted,
            assigned_to: None,
        };
        let json = serde_json::to_string(&g).unwrap();
        let g2: ActiveGoal = serde_json::from_str(&json).unwrap();
        assert_eq!(g2.assigned_to, None);
    }

    // ── BacklogItem ─────────────────────────────────────────────────

    #[test]
    fn backlog_item_serde_round_trip() {
        let b = BacklogItem {
            id: "b-1".to_string(),
            description: "Refactor auth".to_string(),
            source: "review".to_string(),
            score: 0.85,
        };
        let json = serde_json::to_string(&b).unwrap();
        let b2: BacklogItem = serde_json::from_str(&json).unwrap();
        assert_eq!(b, b2);
    }

    #[test]
    fn backlog_item_zero_score() {
        let b = BacklogItem {
            id: "b-0".to_string(),
            description: "Low priority".to_string(),
            source: "auto".to_string(),
            score: 0.0,
        };
        assert_eq!(b.score, 0.0);
    }

    // ── GoalBoard ───────────────────────────────────────────────────

    #[test]
    fn goal_board_new_is_empty() {
        let board = GoalBoard::new();
        assert!(board.active.is_empty());
        assert!(board.backlog.is_empty());
    }

    #[test]
    fn goal_board_default_equals_new() {
        assert_eq!(GoalBoard::default(), GoalBoard::new());
    }

    #[test]
    fn goal_board_active_slots_remaining_empty() {
        let board = GoalBoard::new();
        assert_eq!(board.active_slots_remaining(), MAX_ACTIVE_GOALS);
    }

    #[test]
    fn goal_board_active_slots_remaining_partial() {
        let board = GoalBoard {
            active: vec![sample_goal(), sample_goal()],
            backlog: vec![],
        };
        assert_eq!(board.active_slots_remaining(), MAX_ACTIVE_GOALS - 2);
    }

    #[test]
    fn goal_board_active_slots_remaining_full() {
        let goals: Vec<ActiveGoal> = (0..MAX_ACTIVE_GOALS)
            .map(|i| ActiveGoal {
                id: format!("g-{i}"),
                description: format!("Goal {i}"),
                priority: 1,
                status: GoalProgress::NotStarted,
                assigned_to: None,
            })
            .collect();
        let board = GoalBoard {
            active: goals,
            backlog: vec![],
        };
        assert_eq!(board.active_slots_remaining(), 0);
    }

    #[test]
    fn goal_board_active_slots_remaining_overflow_saturates() {
        let goals: Vec<ActiveGoal> = (0..MAX_ACTIVE_GOALS + 2)
            .map(|i| ActiveGoal {
                id: format!("g-{i}"),
                description: format!("Goal {i}"),
                priority: 1,
                status: GoalProgress::NotStarted,
                assigned_to: None,
            })
            .collect();
        let board = GoalBoard {
            active: goals,
            backlog: vec![],
        };
        assert_eq!(board.active_slots_remaining(), 0);
    }

    #[test]
    fn goal_board_serde_round_trip() {
        let board = GoalBoard {
            active: vec![sample_goal()],
            backlog: vec![BacklogItem {
                id: "b-1".to_string(),
                description: "Later".to_string(),
                source: "auto".to_string(),
                score: 0.5,
            }],
        };
        let json = serde_json::to_string(&board).unwrap();
        let b2: GoalBoard = serde_json::from_str(&json).unwrap();
        assert_eq!(board, b2);
    }

    #[test]
    fn durable_summary_empty_board() {
        let board = GoalBoard::new();
        let s = board.durable_summary();
        assert!(s.contains("active=[none]"));
        assert!(s.contains("backlog=none"));
    }

    #[test]
    fn durable_summary_with_goals_and_backlog() {
        let board = GoalBoard {
            active: vec![sample_goal()],
            backlog: vec![
                BacklogItem {
                    id: "b-1".to_string(),
                    description: "X".to_string(),
                    source: "s".to_string(),
                    score: 0.1,
                },
                BacklogItem {
                    id: "b-2".to_string(),
                    description: "Y".to_string(),
                    source: "s".to_string(),
                    score: 0.2,
                },
            ],
        };
        let s = board.durable_summary();
        assert!(s.contains("Ship MVP"));
        assert!(s.contains("2 items"));
    }

    #[test]
    fn max_active_goals_constant() {
        assert_eq!(MAX_ACTIVE_GOALS, 5);
    }
}
