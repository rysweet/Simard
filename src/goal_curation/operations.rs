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

/// Returns `Some(reason)` if the board contains obviously corrupt or
/// placeholder goals that should not be accepted as valid loaded state.
///
/// Heuristics:
/// - Goal id shorter than 5 chars (catches `g1`, `g12`, `g123`, `g1234`)
/// - Description matches the placeholder pattern `^goal [a-z0-9]{1,4}$` (case-insensitive)
pub fn board_integrity_suspect(board: &GoalBoard) -> Option<String> {
    for goal in &board.active {
        if goal.id.len() < 5 {
            return Some(format!(
                "goal '{}' has suspiciously short id (len {})",
                goal.id,
                goal.id.len()
            ));
        }
        if is_placeholder_description(&goal.description) {
            return Some(format!(
                "goal '{}' has placeholder description '{}'",
                goal.id, goal.description
            ));
        }
    }
    None
}

/// Returns `true` when `desc` matches the placeholder pattern
/// `^\s*goal\s+[a-z0-9]{1,4}\s*$` (case-insensitive).
///
/// Matches strings like `Goal g1`, `goal g1`, `GOAL abc`.
pub fn is_placeholder_description(desc: &str) -> bool {
    let s = desc.trim().to_lowercase();
    if let Some(rest) = s.strip_prefix("goal") {
        let rest = rest.trim();
        !rest.is_empty() && rest.len() <= 4 && rest.chars().all(|c| c.is_ascii_alphanumeric())
    } else {
        false
    }
}

/// One-time migration: if a legacy `goal_records.json` exists on disk, read
/// it, store it in cognitive memory as the canonical snapshot, then delete
/// the file. Migration failures are logged and non-fatal — a corrupt or
/// unreadable file is left in place for operator inspection and the caller
/// proceeds to the cognitive-memory read path.
fn migrate_legacy_disk_file_if_present(bridge: &dyn CognitiveMemoryOps) {
    let goal_path = simard_state_root().join("goal_records.json");
    if !goal_path.exists() {
        return;
    }
    let content = match std::fs::read_to_string(&goal_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "[simard] load_goal_board: legacy goal_records.json read failed ({e}) — \
                 leaving file in place, falling through to cognitive memory"
            );
            return;
        }
    };
    let board: GoalBoard = match serde_json::from_str(&content) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "[simard] load_goal_board: legacy goal_records.json parse error ({e}) — \
                 leaving corrupt file in place for inspection, falling through to cognitive memory"
            );
            return;
        }
    };
    let snapshot = match serde_json::to_string(&board) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "[simard] load_goal_board: legacy migration serialize failed ({e}) — \
                 leaving file in place"
            );
            return;
        }
    };
    if let Err(e) = bridge.store_fact(
        "goal-board:snapshot",
        &snapshot,
        1.0,
        &["goal-board".to_string()],
        "goal-curator",
    ) {
        eprintln!(
            "[simard] load_goal_board: legacy migration store_fact failed ({e}) — \
             leaving file in place; next startup will retry"
        );
        return;
    }
    if let Err(e) = std::fs::remove_file(&goal_path) {
        eprintln!(
            "[simard] load_goal_board: legacy migration remove_file failed ({e}) — \
             snapshot stored but file remains; next startup will retry deletion"
        );
    }
}

/// Load the goal board from cognitive memory.
///
/// Cognitive memory is the single source of truth: the board is stored as a
/// `goal-board:snapshot` fact via `bridge.store_fact()` and read back via
/// `bridge.search_facts()`.
///
/// On every call this also performs an idempotent one-time migration: if a
/// legacy `goal_records.json` file exists on disk (from before the move to
/// memory-only persistence), it is loaded, written into cognitive memory,
/// and removed. The gate is `path.exists()`, so once migrated subsequent
/// calls pay only one cheap `metadata` syscall. Migration failures are
/// logged and non-fatal — the function never panics or propagates an
/// `Err` from migration.
///
/// Resolution order after migration:
/// 1. `bridge.search_facts("goal-board:snapshot", 1, 0.0)` → parsed board
/// 2. `GoalBoard::new()` — empty board when no snapshot exists or parsing fails
pub fn load_goal_board(bridge: &dyn CognitiveMemoryOps) -> SimardResult<GoalBoard> {
    migrate_legacy_disk_file_if_present(bridge);

    // Primary read path: cognitive memory snapshot. Bridge errors are
    // non-fatal — log and fall through to the empty board.
    //
    // NOTE: store_fact is append-only at the trait level (no UPSERT or
    // DELETE), so multiple `save_goal_board` calls accumulate facts. We
    // ask for several and pick the lexicographically-largest id — fact
    // ids are uuid-v7 (`new_id()` in cognitive_memory/mod.rs:276) which
    // are time-ordered, so the largest id is the most recent snapshot.
    let facts = match bridge.search_facts("goal-board:snapshot", 64, 0.0) {
        Ok(f) => f,
        Err(e) => {
            eprintln!(
                "[simard] load_goal_board: cognitive memory search_facts failed ({e}) — returning empty board"
            );
            vec![]
        }
    };
    let latest = facts
        .iter()
        .filter(|f| f.concept == "goal-board:snapshot")
        .max_by(|a, b| a.node_id.cmp(&b.node_id));
    if let Some(fact) = latest {
        match serde_json::from_str::<GoalBoard>(&fact.content) {
            Ok(board) => return Ok(board),
            Err(e) => {
                eprintln!(
                    "[simard] load_goal_board: cognitive memory snapshot parse error ({e}) — returning empty board"
                );
            }
        }
    }

    Ok(GoalBoard::new())
}

/// Save the current board state to cognitive memory as the single source of
/// truth.
///
/// Rejects suspect boards (placeholder descriptions, suspiciously short
/// ids) with `SimardError::InvalidGoalRecord` before any persistence
/// attempt — `bridge.store_fact` is not called when the integrity guard
/// fires.
pub fn save_goal_board(board: &GoalBoard, bridge: &dyn CognitiveMemoryOps) -> SimardResult<()> {
    if let Some(reason) = board_integrity_suspect(board) {
        return Err(SimardError::InvalidGoalRecord {
            field: "board".to_string(),
            reason: format!("refusing to persist suspect board: {reason}"),
        });
    }
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

// ---------------------------------------------------------------------------
// GoalBoard -> Vec<GoalRecord> adapter
// ---------------------------------------------------------------------------

/// Sentinel `SessionId` used to populate `GoalRecord::source_session_id`
/// for records synthesised from the cognitive-memory-backed `GoalBoard`.
/// The board has no per-goal session provenance, so we mark these records
/// as originating from the "all-zeros" session so callers can distinguish
/// them from session-sourced goals.
fn sentinel_source_session_id() -> crate::session::SessionId {
    crate::session::SessionId::parse("00000000-0000-0000-0000-000000000000")
        .expect("sentinel uuid must parse")
}

/// Adapt the cognitive-memory `GoalBoard` into the flat
/// `Vec<crate::goals::GoalRecord>` shape that the engineer loop and meeting
/// curation paths consumed from `FileBackedGoalStore` before issue #1590.
///
/// Mapping (per spec section A3):
/// | Field                | Source                                                            |
/// |----------------------|-------------------------------------------------------------------|
/// | `slug`               | `goal_slug(active.id)` (preserves slug-shaped ids unchanged)      |
/// | `title`              | `active.description` (first line, truncated to 120 chars)         |
/// | `rationale`          | `active.current_activity.unwrap_or_default()`                     |
/// | `status`             | `Completed → GoalStatus::Completed`, all others → `GoalStatus::Active` |
/// | `priority`           | `u8::try_from(active.priority).unwrap_or(u8::MAX)`                |
/// | `owner_identity`     | `active.assigned_to.clone().unwrap_or_else(\|\| "unassigned".into())` |
/// | `source_session_id`  | sentinel `00000000-0000-0000-0000-000000000000`                   |
/// | `updated_in`         | `SessionPhase::Persistence`                                       |
///
/// Backlog items are not emitted — only the active goals surface here, which
/// matches the legacy `FileBackedGoalStore::active_top_goals(...)` contract.
pub fn active_goals_as_records(board: &GoalBoard) -> Vec<crate::goals::GoalRecord> {
    let sentinel = sentinel_source_session_id();
    board
        .active
        .iter()
        .map(|active| {
            let title_first_line = active.description.lines().next().unwrap_or("");
            let title: String = title_first_line.chars().take(120).collect();

            let status = if matches!(active.status, GoalProgress::Completed) {
                crate::goals::GoalStatus::Completed
            } else {
                crate::goals::GoalStatus::Active
            };

            let priority = u8::try_from(active.priority).unwrap_or(u8::MAX);
            let owner_identity = active
                .assigned_to
                .clone()
                .unwrap_or_else(|| "unassigned".to_string());

            crate::goals::GoalRecord {
                slug: crate::goals::goal_slug(&active.id),
                title,
                rationale: active.current_activity.clone().unwrap_or_default(),
                status,
                priority,
                owner_identity,
                source_session_id: sentinel.clone(),
                updated_in: crate::session::SessionPhase::Persistence,
            }
        })
        .collect()
}
