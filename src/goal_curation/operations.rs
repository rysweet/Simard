//! Board mutation, validation, persistence, and seeding operations.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{LazyLock, Mutex};

use serde_json::json;
use tracing::{debug, warn};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

use super::types::{ActiveGoal, BacklogItem, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS};

/// Process-local critical section for the merge-on-write pipeline in
/// [`save_goal_board`]. Serializes the read-merge-write window inside a
/// single Simard process so two concurrent in-process bridge clients
/// (daemon + dashboard, two engineer worktrees in one cargo build, …)
/// cannot both observe the same persisted snapshot and then each store a
/// stale-derived snapshot that drops the other writer's goals (issue
/// [#1915](https://github.com/rysweet/Simard/issues/1915)).
///
/// Cross-process races still fall back to the best-effort field-level
/// guarantees documented on `save_goal_board` (the LadybugDB flock at the
/// storage layer prevents simultaneous writes, but does not provide
/// snapshot isolation across separate read-then-write sequences).
static SAVE_GOAL_BOARD_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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
/// Thin delegating wrapper around [`crate::state_root::simard_state_root`]
/// so existing `use goal_curation::operations::simard_state_root` imports
/// keep compiling. There is exactly one resolution helper; this is the
/// migration-compat surface. Issue #1906.
pub fn simard_state_root() -> std::path::PathBuf {
    crate::state_root::simard_state_root()
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
/// 1. [`read_latest_snapshot`] — `bridge.search_facts("goal-board:snapshot", 64, 0.0)`
///    filtered by `concept == "goal-board:snapshot"`, `max_by(node_id)`, parsed.
/// 2. `GoalBoard::new()` — empty board when no snapshot exists or parsing fails.
pub fn load_goal_board(bridge: &dyn CognitiveMemoryOps) -> SimardResult<GoalBoard> {
    migrate_legacy_disk_file_if_present(bridge);

    // Primary read path: cognitive memory snapshot via the shared helper.
    // The helper returns `None` on bridge error, on zero results, or on a
    // payload parse failure — load_goal_board folds all three into the
    // legacy "empty board" fallback so callers see a stable contract.
    Ok(read_latest_snapshot(bridge).unwrap_or_default())
}

/// Read the most recent `goal-board:snapshot` fact from cognitive memory,
/// or `None` if no snapshot is available.
///
/// Shared by [`load_goal_board`] (initial read) and [`save_goal_board`]
/// (merge-on-write read). All failure modes (bridge error, empty result,
/// payload deserialization failure) return `None` with a `warn!` log line
/// that records the bridge operation and error kind only — never the
/// payload, never goal descriptions.
///
/// `search_facts` is called with `limit=64, min_confidence=0.0` so that
/// the merge read can see recent snapshots even when the fact log has
/// accumulated. Fact ids are uuid-v7 (see `new_id()` in
/// `cognitive_memory/mod.rs`), so the lexicographically-largest id is the
/// most recent snapshot.
pub(super) fn read_latest_snapshot(bridge: &dyn CognitiveMemoryOps) -> Option<GoalBoard> {
    let facts = match bridge.search_facts("goal-board:snapshot", 64, 0.0) {
        Ok(f) => f,
        Err(e) => {
            warn!(
                concept = "goal-board:snapshot",
                op = "search_facts",
                error_kind = %e,
                "read_latest_snapshot: cognitive memory read failed; returning None"
            );
            return None;
        }
    };
    let latest = facts
        .iter()
        .filter(|f| f.concept == "goal-board:snapshot")
        .max_by(|a, b| a.node_id.cmp(&b.node_id))?;
    match serde_json::from_str::<GoalBoard>(&latest.content) {
        Ok(board) => Some(board),
        Err(e) => {
            warn!(
                concept = "goal-board:snapshot",
                op = "deserialize",
                error_kind = %e,
                "read_latest_snapshot: snapshot payload parse failed; returning None"
            );
            None
        }
    }
}

/// Merge a persisted snapshot with an in-flight board to produce a new
/// merged board suitable for `store_fact`.
///
/// **Union by `id`.** Both `active` and `backlog` are unioned by `id`.
/// On id collision the in-flight side wins for all fields (description,
/// priority, status, assigned_to, current_activity, wip_refs). This
/// reflects that the caller has the most recent intent for the goals it
/// owns. Cross-set collisions (same id in `persisted.active` and
/// `in_flight.backlog`, or vice versa) resolve to the in-flight
/// classification — the goal/item appears exactly once in the merged
/// board, in whichever set the in-flight board placed it.
///
/// **Active capacity.** If the merged active set exceeds
/// [`MAX_ACTIVE_GOALS`] (= 5), it is truncated using a deterministic sort
/// key:
///
/// 1. `priority` ascending (lower numeric value = higher importance, kept first)
/// 2. In-flight-origin preferred over persisted-origin on tie
/// 3. `id` lexicographic ascending on tie
///
/// **Backlog capacity.** Backlog has no bound and is never truncated.
///
/// Pure function: never panics, never `unwrap`s, never `expect`s.
/// Iteration order is deterministic via `BTreeMap`/`BTreeSet`, so repeated
/// merges of identical inputs produce identical outputs.
///
/// See issue [#1915](https://github.com/rysweet/Simard/issues/1915) for
/// the race this prevents.
pub(super) fn merge_boards(persisted: GoalBoard, in_flight: GoalBoard) -> GoalBoard {
    // Collect all in-flight ids (both active and backlog) so that
    // cross-set collisions resolve to the in-flight classification.
    let in_flight_ids: BTreeSet<String> = in_flight
        .active
        .iter()
        .map(|g| g.id.clone())
        .chain(in_flight.backlog.iter().map(|b| b.id.clone()))
        .collect();

    // Active union. `BTreeMap` keyed on id gives deterministic iteration.
    // For each entry we also track whether the goal originated from the
    // in-flight board (true) or the persisted board (false) — used by the
    // capacity-truncation tiebreak.
    let mut active_map: BTreeMap<String, (ActiveGoal, bool)> = BTreeMap::new();
    for goal in persisted.active {
        // Skip persisted entries shadowed by an in-flight entry in *either*
        // set; in-flight classification wins on cross-set collisions.
        if in_flight_ids.contains(&goal.id) {
            continue;
        }
        active_map.insert(goal.id.clone(), (goal, false));
    }
    for goal in in_flight.active {
        active_map.insert(goal.id.clone(), (goal, true));
    }

    // Backlog union with the same rules.
    let mut backlog_map: BTreeMap<String, BacklogItem> = BTreeMap::new();
    for item in persisted.backlog {
        if in_flight_ids.contains(&item.id) {
            continue;
        }
        backlog_map.insert(item.id.clone(), item);
    }
    for item in in_flight.backlog {
        backlog_map.insert(item.id.clone(), item);
    }

    let mut active_with_origin: Vec<(ActiveGoal, bool)> = active_map.into_values().collect();
    let mut truncated_count = 0usize;
    if active_with_origin.len() > MAX_ACTIVE_GOALS {
        // Deterministic sort key: priority asc, in-flight (true) before
        // persisted (false), id lex asc. We invert the bool comparison
        // because `true > false` in Rust's default Ord — we want `true`
        // (in-flight) to come first in ascending order, hence reverse.
        active_with_origin.sort_by(|a, b| {
            a.0.priority
                .cmp(&b.0.priority)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.0.id.cmp(&b.0.id))
        });
        truncated_count = active_with_origin.len() - MAX_ACTIVE_GOALS;
        active_with_origin.truncate(MAX_ACTIVE_GOALS);
    }

    let active: Vec<ActiveGoal> = active_with_origin.into_iter().map(|(g, _)| g).collect();
    let backlog: Vec<BacklogItem> = backlog_map.into_values().collect();

    debug!(
        merge = "goal-board",
        merged_active = active.len(),
        merged_backlog = backlog.len(),
        truncated = truncated_count,
        "merge_boards: completed"
    );

    GoalBoard { active, backlog }
}

/// Save the current board state to cognitive memory as the single source of
/// truth, using **merge-on-write** semantics to prevent concurrent
/// `CognitiveMemoryOps` clients from silently clobbering each other's
/// goals (issue [#1915](https://github.com/rysweet/Simard/issues/1915)).
///
/// Pipeline:
/// 1. Run [`board_integrity_suspect`] on the in-flight board. Returning
///    `Some(_)` short-circuits with `SimardError::InvalidGoalRecord`
///    before any read or write — the persisted snapshot is inductively
///    guard-clean (every prior write went through this same guard).
/// 2. Call [`read_latest_snapshot`] to re-read the latest persisted
///    `goal-board:snapshot` fact. On error / empty / parse failure
///    (already logged inside the helper), the merge step is skipped and
///    the in-flight board is persisted unchanged — preserving write
///    availability when the read path is temporarily unhealthy.
/// 3. Call [`merge_boards`] to union by `id` (in-flight wins on collision)
///    and truncate the active set to [`MAX_ACTIVE_GOALS`] using the
///    deterministic sort key documented on `merge_boards`.
/// 4. `bridge.store_fact("goal-board:snapshot", &serde_json::to_string(&merged)?, 1.0, &["goal-board"], "goal-curator")`.
///    Fact metadata is constant — only the `GoalBoard` payload is merged.
///
/// **Best-effort guarantee.** No goal *added* on a disjoint subset
/// *disappears* in the common multi-client race. A tight
/// read-read-write-write interleaving across separate
/// `CognitiveMemoryOps` clients can still produce a snapshot that omits
/// the earlier writer's most recent fact; same-id concurrent edits
/// resolve field-level last-writer-wins. Callers needing strict
/// serializability must route through the daemon IPC socket.
pub fn save_goal_board(board: &GoalBoard, bridge: &dyn CognitiveMemoryOps) -> SimardResult<()> {
    // Step 1: guard the in-flight board. Persisted snapshot is inductively
    // guard-clean (every prior write went through this same check), so the
    // merged board does not need re-guarding — re-guarding would risk
    // erroneously rejecting valid persisted goals that an LLM later
    // contaminated locally on the in-flight side.
    if let Some(reason) = board_integrity_suspect(board) {
        return Err(SimardError::InvalidGoalRecord {
            field: "board".to_string(),
            reason: format!("refusing to persist suspect board: {reason}"),
        });
    }

    // Acquire the process-local merge-on-write critical section so two
    // in-process callers serialize their read-merge-write windows. Without
    // this, two threads can both read an empty (or stale) snapshot, each
    // merge it with their own in-flight board, and each store a snapshot
    // that lacks the other writer's goals — the original #1915 failure.
    // Mutex poisoning is treated as recoverable: we take the inner guard
    // and proceed, because a poisoned mutex still serialises us correctly.
    let _critical = SAVE_GOAL_BOARD_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // Step 2: read latest persisted snapshot (None on any failure).
    let persisted = read_latest_snapshot(bridge);

    // Step 3: merge in-flight on top of persisted. On read failure /
    // empty store, persist the in-flight board unchanged.
    let (merged, persisted_active, persisted_backlog) = match persisted {
        Some(p) => {
            let pa = p.active.len();
            let pb = p.backlog.len();
            (merge_boards(p, board.clone()), pa, pb)
        }
        None => (board.clone(), 0, 0),
    };

    debug!(
        merge = "goal-board",
        persisted_active = persisted_active,
        persisted_backlog = persisted_backlog,
        in_flight_active = board.active.len(),
        in_flight_backlog = board.backlog.len(),
        merged_active = merged.active.len(),
        merged_backlog = merged.backlog.len(),
        "save_goal_board: persisting merged snapshot"
    );

    // Step 4: serialize and store.
    let snapshot = serde_json::to_string(&merged).map_err(|e| SimardError::InvalidGoalRecord {
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
