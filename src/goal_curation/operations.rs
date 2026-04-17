//! Board mutation, validation, persistence, and seeding operations.

use serde_json::json;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

use super::types::{ActiveGoal, BacklogItem, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS};

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// Load a goal board from cognitive memory. Searches for the latest board
/// snapshot stored as a semantic fact and falls back to an empty board.
pub fn load_goal_board(bridge: &dyn CognitiveMemoryOps) -> SimardResult<GoalBoard> {
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
pub fn save_goal_board(board: &GoalBoard, bridge: &dyn CognitiveMemoryOps) -> SimardResult<()> {
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

/// Persist the board state and record an episode for recall.
pub fn persist_board(board: &GoalBoard, bridge: &dyn CognitiveMemoryOps) -> SimardResult<()> {
    save_goal_board(board, bridge)?;
    bridge.store_episode(
        &board.durable_summary(),
        "goal-curator",
        Some(&json!({"active_count": board.active.len(), "backlog_count": board.backlog.len()})),
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Board mutations
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Seeding
// ---------------------------------------------------------------------------

/// The 5 default starter goals shared by both `seed_default_board` (GoalBoard)
/// and `seed_default_goals` (GoalStore). Single source of truth.
///
/// Each tuple: (priority, title, description).
pub const DEFAULT_SEED_GOALS: [(u32, &str, &str); 5] = [
    (
        1,
        "Improve amplihack test coverage",
        "Increase test coverage across the amplihack ecosystem to catch regressions early",
    ),
    (
        2,
        "Enhance Simard meeting experience",
        "Improve the interactive meeting facilitator with better UX and richer handoffs",
    ),
    (
        3,
        "Improve cognitive memory persistence",
        "Harden memory consolidation and ensure durable recall across sessions",
    ),
    (
        4,
        "Add more gym benchmark scenarios",
        "Expand the gym evaluation suite with diverse scenarios for broader coverage",
    ),
    (
        5,
        "Explore developer ideas from tracked researchers",
        "Monitor tracked researchers and incorporate promising ideas into the roadmap",
    ),
];

/// Seed the board with 5 default starter goals if it has no active goals.
/// Returns the number of goals added.
pub fn seed_default_board(board: &mut GoalBoard) -> usize {
    if !board.active.is_empty() {
        return 0;
    }

    for (priority, id_source, description) in DEFAULT_SEED_GOALS {
        let id = crate::goals::goal_slug(id_source);
        board.active.push(ActiveGoal {
            id,
            description: description.to_string(),
            priority,
            status: GoalProgress::NotStarted,
            assigned_to: None,
        });
    }

    DEFAULT_SEED_GOALS.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_goal(id: &str, priority: u32) -> ActiveGoal {
        ActiveGoal {
            id: id.to_string(),
            description: format!("Goal {id}"),
            priority,
            status: GoalProgress::NotStarted,
            assigned_to: None,
        }
    }

    fn make_backlog(id: &str) -> BacklogItem {
        BacklogItem {
            id: id.to_string(),
            description: format!("Backlog {id}"),
            source: "test".to_string(),
            score: 0.0,
        }
    }

    #[test]
    fn add_active_goal_succeeds_and_rejects_duplicate() {
        let mut board = GoalBoard::new();
        assert!(add_active_goal(&mut board, make_goal("g1", 1)).is_ok());
        assert_eq!(board.active.len(), 1);
        assert!(add_active_goal(&mut board, make_goal("g1", 2)).is_err());
    }

    #[test]
    fn add_active_goal_rejects_at_capacity() {
        let mut board = GoalBoard::new();
        for i in 0..MAX_ACTIVE_GOALS {
            add_active_goal(&mut board, make_goal(&format!("g{i}"), (i + 1) as u32)).unwrap();
        }
        let result = add_active_goal(&mut board, make_goal("overflow", 1));
        assert!(result.is_err());
    }

    #[test]
    fn add_active_goal_rejects_zero_priority() {
        let mut board = GoalBoard::new();
        let result = add_active_goal(&mut board, make_goal("g1", 0));
        assert!(result.is_err());
    }

    #[test]
    fn add_backlog_item_succeeds_and_rejects_duplicate() {
        let mut board = GoalBoard::new();
        assert!(add_backlog_item(&mut board, make_backlog("b1")).is_ok());
        assert_eq!(board.backlog.len(), 1);
        assert!(add_backlog_item(&mut board, make_backlog("b1")).is_err());
    }

    #[test]
    fn promote_to_active_moves_item() {
        let mut board = GoalBoard::new();
        add_backlog_item(&mut board, make_backlog("b1")).unwrap();
        promote_to_active(&mut board, "b1", 1, None).unwrap();
        assert!(board.backlog.is_empty());
        assert_eq!(board.active.len(), 1);
        assert_eq!(board.active[0].id, "b1");
        assert!(matches!(board.active[0].status, GoalProgress::NotStarted));
    }

    #[test]
    fn promote_to_active_not_found() {
        let mut board = GoalBoard::new();
        assert!(promote_to_active(&mut board, "nonexistent", 1, None).is_err());
    }

    #[test]
    fn update_goal_progress_and_archive_completed() {
        let mut board = GoalBoard::new();
        add_active_goal(&mut board, make_goal("g1", 1)).unwrap();
        add_active_goal(&mut board, make_goal("g2", 2)).unwrap();
        update_goal_progress(&mut board, "g1", GoalProgress::Completed).unwrap();
        let archived = archive_completed(&mut board);
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, "g1");
        assert_eq!(board.active.len(), 1);
    }

    #[test]
    fn update_goal_progress_rejects_over_100_percent() {
        let mut board = GoalBoard::new();
        add_active_goal(&mut board, make_goal("g1", 1)).unwrap();
        let result =
            update_goal_progress(&mut board, "g1", GoalProgress::InProgress { percent: 101 });
        assert!(result.is_err());
    }

    #[test]
    fn seed_default_board_populates_empty_board() {
        let mut board = GoalBoard::new();
        let count = seed_default_board(&mut board);
        assert_eq!(count, DEFAULT_SEED_GOALS.len());
        assert_eq!(board.active.len(), DEFAULT_SEED_GOALS.len());
    }

    #[test]
    fn seed_default_board_skips_non_empty() {
        let mut board = GoalBoard::new();
        add_active_goal(&mut board, make_goal("existing", 1)).unwrap();
        let count = seed_default_board(&mut board);
        assert_eq!(count, 0);
        assert_eq!(board.active.len(), 1);
    }
}
