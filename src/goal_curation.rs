//! Top-5 goal board with active goals and backlog curation.
//!
//! `GoalBoard` maintains a strict maximum of 5 active goals. Promotion from
//! backlog to active enforces the cap, and progress updates track completion.

use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{SimardError, SimardResult};
use crate::memory_bridge::CognitiveMemoryBridge;

/// Maximum number of concurrently active goals.
pub const MAX_ACTIVE_GOALS: usize = 5;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

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

fn required_field(field: &str, value: &str) -> SimardResult<()> {
    if value.trim().is_empty() {
        return Err(SimardError::InvalidGoalRecord {
            field: field.to_string(),
            reason: "value cannot be empty".to_string(),
        });
    }
    Ok(())
}

fn validate_priority(field: &str, priority: u32) -> SimardResult<()> {
    if priority == 0 {
        return Err(SimardError::InvalidGoalRecord {
            field: field.to_string(),
            reason: "priority must be at least 1".to_string(),
        });
    }
    Ok(())
}

fn validate_active_goal(goal: &ActiveGoal) -> SimardResult<()> {
    required_field("active_goal.id", &goal.id)?;
    required_field("active_goal.description", &goal.description)?;
    validate_priority("active_goal.priority", goal.priority)?;
    if let GoalProgress::InProgress { percent } = &goal.status
        && *percent > 100
    {
        return Err(SimardError::InvalidGoalRecord {
            field: "active_goal.status".to_string(),
            reason: "progress percent cannot exceed 100".to_string(),
        });
    }
    Ok(())
}

fn validate_backlog_item(item: &BacklogItem) -> SimardResult<()> {
    required_field("backlog_item.id", &item.id)?;
    required_field("backlog_item.description", &item.description)?;
    required_field("backlog_item.source", &item.source)?;
    Ok(())
}

/// Load a goal board from cognitive memory. Searches for the latest board
/// snapshot stored as a semantic fact and falls back to an empty board.
pub fn load_goal_board(bridge: &CognitiveMemoryBridge) -> SimardResult<GoalBoard> {
    let facts = bridge.search_facts("goal-board:snapshot", 1, 0.0)?;
    if let Some(fact) = facts.first() {
        let board = serde_json::from_str::<GoalBoard>(&fact.content).map_err(|e| {
            SimardError::InvalidGoalRecord {
                field: "board".to_string(),
                reason: format!("failed to deserialize goal board: {e}"),
            }
        })?;
        return Ok(board);
    }
    Ok(GoalBoard::new())
}

/// Save the current board state as a semantic fact in cognitive memory.
pub fn save_goal_board(board: &GoalBoard, bridge: &CognitiveMemoryBridge) -> SimardResult<()> {
    let snapshot = serde_json::to_string(board).map_err(|e| SimardError::InvalidGoalRecord {
        field: "board".to_string(),
        reason: format!("failed to serialize goal board: {e}"),
    })?;
    bridge.store_fact(
        "goal-board:snapshot",
        &snapshot,
        1.0,
        &["goal-board".to_string()],
        "goal-curator",
    )?;
    Ok(())
}

/// Add a new active goal. Fails if the board is already at capacity.
pub fn add_active_goal(board: &mut GoalBoard, goal: ActiveGoal) -> SimardResult<()> {
    validate_active_goal(&goal)?;
    if board.active.len() >= MAX_ACTIVE_GOALS {
        return Err(SimardError::InvalidGoalRecord {
            field: "active".to_string(),
            reason: format!("cannot add active goal — board is at capacity ({MAX_ACTIVE_GOALS})"),
        });
    }
    if board.active.iter().any(|g| g.id == goal.id) {
        return Err(SimardError::InvalidGoalRecord {
            field: "active_goal.id".to_string(),
            reason: format!("goal '{}' is already active", goal.id),
        });
    }
    board.active.push(goal);
    Ok(())
}

/// Add a backlog item.
pub fn add_backlog_item(board: &mut GoalBoard, item: BacklogItem) -> SimardResult<()> {
    validate_backlog_item(&item)?;
    if board.backlog.iter().any(|b| b.id == item.id) {
        return Err(SimardError::InvalidGoalRecord {
            field: "backlog_item.id".to_string(),
            reason: format!("backlog item '{}' already exists", item.id),
        });
    }
    board.backlog.push(item);
    Ok(())
}

/// Promote a backlog item to an active goal. The item is removed from the
/// backlog and inserted as a `NotStarted` active goal with the given priority.
pub fn promote_to_active(
    board: &mut GoalBoard,
    backlog_id: &str,
    priority: u32,
    assigned_to: Option<String>,
) -> SimardResult<()> {
    validate_priority("priority", priority)?;
    if board.active.len() >= MAX_ACTIVE_GOALS {
        return Err(SimardError::InvalidGoalRecord {
            field: "active".to_string(),
            reason: format!("cannot promote — board is at capacity ({MAX_ACTIVE_GOALS})"),
        });
    }
    let position = board
        .backlog
        .iter()
        .position(|item| item.id == backlog_id)
        .ok_or_else(|| SimardError::InvalidGoalRecord {
            field: "backlog_id".to_string(),
            reason: format!("backlog item '{backlog_id}' not found"),
        })?;
    let item = board.backlog.remove(position);
    board.active.push(ActiveGoal {
        id: item.id,
        description: item.description,
        priority,
        status: GoalProgress::NotStarted,
        assigned_to,
    });
    Ok(())
}

/// Update the progress of an active goal.
pub fn update_goal_progress(
    board: &mut GoalBoard,
    goal_id: &str,
    progress: GoalProgress,
) -> SimardResult<()> {
    if let GoalProgress::InProgress { percent } = &progress
        && *percent > 100
    {
        return Err(SimardError::InvalidGoalRecord {
            field: "progress.percent".to_string(),
            reason: "progress percent cannot exceed 100".to_string(),
        });
    }
    let goal = board
        .active
        .iter_mut()
        .find(|g| g.id == goal_id)
        .ok_or_else(|| SimardError::InvalidGoalRecord {
            field: "goal_id".to_string(),
            reason: format!("active goal '{goal_id}' not found"),
        })?;
    goal.status = progress;
    Ok(())
}

/// Remove completed goals from the active list. Returns the removed goals.
pub fn archive_completed(board: &mut GoalBoard) -> Vec<ActiveGoal> {
    let mut archived = Vec::new();
    board.active.retain(|goal| {
        if matches!(goal.status, GoalProgress::Completed) {
            archived.push(goal.clone());
            false
        } else {
            true
        }
    });
    archived
}

/// Persist the board state and record an episode for recall.
pub fn persist_board(board: &GoalBoard, bridge: &CognitiveMemoryBridge) -> SimardResult<()> {
    save_goal_board(board, bridge)?;
    bridge.store_episode(
        &board.durable_summary(),
        "goal-curator",
        Some(&json!({"active_count": board.active.len(), "backlog_count": board.backlog.len()})),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use serde_json::json;

    fn mock_bridge() -> CognitiveMemoryBridge {
        let transport =
            InMemoryBridgeTransport::new("test-goals", |method, _params| match method {
                "memory.search_facts" => Ok(json!({"facts": []})),
                "memory.store_fact" => Ok(json!({"id": "sem_g1"})),
                "memory.store_episode" => Ok(json!({"id": "epi_g1"})),
                _ => Err(crate::bridge::BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown method: {method}"),
                }),
            });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    fn sample_goal(id: &str, priority: u32) -> ActiveGoal {
        ActiveGoal {
            id: id.to_string(),
            description: format!("Goal {id}"),
            priority,
            status: GoalProgress::NotStarted,
            assigned_to: None,
        }
    }

    #[test]
    fn enforce_max_active_goals() {
        let mut board = GoalBoard::new();
        for i in 1..=MAX_ACTIVE_GOALS {
            add_active_goal(&mut board, sample_goal(&format!("g{i}"), i as u32)).unwrap();
        }
        let err = add_active_goal(&mut board, sample_goal("g-overflow", 1)).unwrap_err();
        assert!(err.to_string().contains("capacity"));
    }

    #[test]
    fn promote_backlog_to_active() {
        let mut board = GoalBoard::new();
        add_backlog_item(
            &mut board,
            BacklogItem {
                id: "bl-1".to_string(),
                description: "Research topic X".to_string(),
                source: "meeting".to_string(),
                score: 0.8,
            },
        )
        .unwrap();
        promote_to_active(&mut board, "bl-1", 2, Some("alice".to_string())).unwrap();
        assert_eq!(board.active.len(), 1);
        assert!(board.backlog.is_empty());
        assert_eq!(board.active[0].assigned_to.as_deref(), Some("alice"));
    }

    #[test]
    fn update_progress_and_archive() {
        let mut board = GoalBoard::new();
        add_active_goal(&mut board, sample_goal("g1", 1)).unwrap();
        update_goal_progress(&mut board, "g1", GoalProgress::Completed).unwrap();
        let archived = archive_completed(&mut board);
        assert_eq!(archived.len(), 1);
        assert!(board.active.is_empty());
    }

    #[test]
    fn load_empty_board_from_bridge() {
        let bridge = mock_bridge();
        let board = load_goal_board(&bridge).unwrap();
        assert!(board.active.is_empty());
        assert!(board.backlog.is_empty());
    }

    #[test]
    fn rejects_zero_priority() {
        let mut board = GoalBoard::new();
        let err = add_active_goal(
            &mut board,
            ActiveGoal {
                id: "bad".to_string(),
                description: "Zero priority".to_string(),
                priority: 0,
                status: GoalProgress::NotStarted,
                assigned_to: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("priority"));
    }

    #[test]
    fn rejects_progress_over_100() {
        let mut board = GoalBoard::new();
        add_active_goal(&mut board, sample_goal("g1", 1)).unwrap();
        let err = update_goal_progress(&mut board, "g1", GoalProgress::InProgress { percent: 200 })
            .unwrap_err();
        assert!(err.to_string().contains("100"));
    }
}
