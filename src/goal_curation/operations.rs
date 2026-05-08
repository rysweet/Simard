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

/// Resolve the Simard state root directory.
///
/// Priority: `$SIMARD_STATE_ROOT` env var → `$HOME/.simard` → `/home/azureuser/.simard`.
pub fn simard_state_root() -> std::path::PathBuf {
    if let Ok(v) = std::env::var("SIMARD_STATE_ROOT") {
        return std::path::PathBuf::from(v);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".into());
    std::path::PathBuf::from(home).join(".simard")
}

/// Load a goal board using a three-tier fallback strategy:
///
/// 1. `$SIMARD_STATE_ROOT/goal_records.json` (or `~/.simard/goal_records.json`) — primary
///    source of truth, written by `save_goal_board()` on every save.
/// 2. `search_facts("goal-board:snapshot", 1, 0.0)` from cognitive memory — fallback for
///    the first-ever run (no disk file yet) and disaster recovery.
/// 3. `GoalBoard::new()` — empty board if both sources are absent or unreadable.
///
/// The disk file is always at least as recent as the last cognitive memory write, and
/// avoids the unordered `LIMIT 1` retrieval issue in cognitive memory (issue #1574).
pub fn load_goal_board(bridge: &dyn CognitiveMemoryOps) -> SimardResult<GoalBoard> {
    // Tier 1: disk file — primary source of truth.
    let goal_path = simard_state_root().join("goal_records.json");
    match std::fs::read_to_string(&goal_path) {
        Ok(content) => match serde_json::from_str::<GoalBoard>(&content) {
            Ok(board) => return Ok(board),
            Err(e) => {
                eprintln!(
                    "[simard] load_goal_board: goal_records.json parse error ({e}) — falling back to cognitive memory"
                );
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // File not yet created — fall through to cognitive memory.
        }
        Err(e) => {
            eprintln!(
                "[simard] load_goal_board: failed to read goal_records.json ({e}) — falling back to cognitive memory"
            );
        }
    }

    // Tier 2: cognitive memory snapshot.  Errors here (bridge unavailable,
    // timeout, etc.) are non-fatal: log and fall through to the empty board.
    let facts = match bridge.search_facts("goal-board:snapshot", 1, 0.0) {
        Ok(f) => f,
        Err(e) => {
            eprintln!(
                "[simard] load_goal_board: cognitive memory search_facts failed ({e}) — returning empty board"
            );
            vec![]
        }
    };
    if let Some(fact) = facts.first() {
        match serde_json::from_str::<GoalBoard>(&fact.content) {
            Ok(board) => return Ok(board),
            Err(e) => {
                eprintln!(
                    "[simard] load_goal_board: cognitive memory snapshot parse error ({e}) — returning empty board"
                );
            }
        }
    }

    // Tier 3: empty board.
    Ok(GoalBoard::new())
}

/// Save the current board state as a semantic fact in cognitive memory
/// **and** to `goal_records.json` on disk.
///
/// The on-disk write ensures that the next OODA cycle start (which loads
/// from cognitive memory OR disk) always sees the latest board state,
/// even when cognitive memory `search_facts` returns a stale snapshot
/// due to unordered `LIMIT 1` retrieval across multiple fact nodes.
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

    // Also write to disk so intermediate saves (e.g. stale subordinate
    // clearing during Act phase) are durable across cycle boundaries.
    let state_root = simard_state_root();
    let goal_path = state_root.join("goal_records.json");
    if let Err(e) = std::fs::create_dir_all(&state_root) {
        eprintln!("[simard] save_goal_board: failed to create state dir: {e}");
    }
    if let Ok(pretty) = serde_json::to_string_pretty(board)
        && let Err(e) = std::fs::write(&goal_path, pretty)
    {
        eprintln!("[simard] save_goal_board: failed to write goal_records.json: {e}");
    }

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

/// Default backlog score for stewardship-filed issues (issue #1167).
pub const DEFAULT_STEWARD_SCORE: f64 = 0.6;

/// Enqueue a stewardship-filed (or matched) GitHub issue onto the backlog
/// (issue #1167).
///
/// Idempotent: if a backlog item with the same stewardship id already exists
/// (same repo + issue number), this is a no-op and returns `Ok(())`.
pub fn enqueue_stewardship_issue(
    board: &mut GoalBoard,
    repo: &str,
    issue_number: u64,
    url: &str,
    signature: &str,
) -> SimardResult<()> {
    let id = format!("stewardship-{}-{}", repo.replace('/', "_"), issue_number);
    if board.backlog.iter().any(|b| b.id == id) {
        return Ok(());
    }
    let item = BacklogItem {
        id,
        description: format!(
            "Investigate stewardship-filed failure (signature {signature}) — {url}"
        ),
        source: format!("stewardship:{repo}#{issue_number}"),
        score: DEFAULT_STEWARD_SCORE,
    };
    add_backlog_item(board, item)
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
        current_activity: None,
        wip_refs: vec![],
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

/// Clear the assignment of an active goal, resetting it to `NotStarted` so
/// it can be re-dispatched on the next OODA cycle.
///
/// Used when a subordinate is detected as dead or stale with no artifacts —
/// clearing `assigned_to` allows `dispatch_advance_goal` to re-enter the
/// session-based spawn path rather than the subordinate heartbeat path.
pub fn clear_goal_assignment(board: &mut GoalBoard, goal_id: &str) -> SimardResult<()> {
    let goal = board
        .active
        .iter_mut()
        .find(|g| g.id == goal_id)
        .ok_or_else(|| SimardError::InvalidGoalRecord {
            field: "goal_id".to_string(),
            reason: format!("active goal '{goal_id}' not found"),
        })?;
    goal.assigned_to = None;
    goal.status = GoalProgress::NotStarted;
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
        "Fix broken features",
        "Analyze all Simard features against their specs and intended behavior. Identify features that are not working correctly (e.g., meeting REPL, any other broken functionality) and fix them. Prioritize by user impact. Start by auditing the Specs/ directory and comparing each spec against the actual implementation to find gaps and failures.",
    ),
    (
        5,
        "Self-serve dashboard improvement",
        "Use your own dashboard (localhost:8080) with Playwright to understand your operations and memory. Continuously improve the dashboard until it is very useful for understanding your internal state. The dashboard must not use jargon and must remain useful to humans too. Login by reading the code from ~/.simard/.dashkey. Playwright is installed (playwright==1.59.0 with Chromium browser).",
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
            current_activity: None,
            wip_refs: vec![],
        });
    }

    DEFAULT_SEED_GOALS.len()
}
