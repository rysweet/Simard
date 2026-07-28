//! Board mutation, validation, persistence, and seeding operations.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{LazyLock, Mutex};

use serde_json::json;
use tracing::{debug, warn};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

use super::types::{
    ActiveGoal, BacklogItem, CARRYOVER_CONCEPT, GoalBoard, GoalCarryoverRecord, GoalProgress,
    MAX_ACTIVE_GOALS,
};

/// Process-local critical section for the merge-on-write pipeline in
/// [`save_goal_board`]. Serializes the read-merge-write window inside a
/// single Simard process so two concurrent in-process memory clients
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
// Parent-progress roll-up (issue #2405)
// ---------------------------------------------------------------------------

/// Roll a parent goal's progress up from its `children` (issue #2405), so the
/// board never shows a large goal parked at a stale percent while its slices
/// move. See `docs/reference/goal-decomposition.md`.
///
/// Returns:
/// - `None` when there are no children — the parent keeps its own
///   directly-tracked status (the signal is "keep own").
/// - `Some(Blocked(..))` when **any** child is `Blocked` — the block surfaces on
///   the parent so the operator and the brain see the umbrella is gated.
/// - `Some(Completed)` only when **every** child is `Completed`.
/// - `Some(InProgress { percent })` otherwise, where `percent` is the rounded
///   mean of the children's percents (`Completed` → 100, `InProgress { p }` →
///   `p`, and `NotStarted` / `Proposed` / `Paused` → 0).
pub fn rollup_parent_progress(children: &[ActiveGoal]) -> Option<GoalProgress> {
    if children.is_empty() {
        return None;
    }

    let blocked: Vec<String> = children
        .iter()
        .filter_map(|child| match &child.status {
            GoalProgress::Blocked(reason) => Some(reason.clone()),
            _ => None,
        })
        .collect();
    if !blocked.is_empty() {
        return Some(GoalProgress::Blocked(format!(
            "{} child goal(s) blocked: {}",
            blocked.len(),
            blocked.join("; ")
        )));
    }

    if children
        .iter()
        .all(|child| child.status == GoalProgress::Completed)
    {
        return Some(GoalProgress::Completed);
    }

    let sum: u32 = children
        .iter()
        .map(|child| match &child.status {
            GoalProgress::Completed => 100,
            GoalProgress::InProgress { percent } => *percent,
            GoalProgress::NotStarted | GoalProgress::Proposed | GoalProgress::Paused => 0,
            // `Blocked` is handled above; unreachable here but mapped to 0 for
            // totality.
            GoalProgress::Blocked(_) => 0,
        })
        .sum();
    let percent = (f64::from(sum) / children.len() as f64).round() as u32;
    Some(GoalProgress::InProgress { percent })
}

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

/// Resolve the path of the cross-process goal-board write lock.
///
/// Delegates to [`crate::state_root::goal_board_lock_path`] so the daemon, the
/// `simard goal` CLI, and the authoritative [`crate::goal_board_store`]
/// rendezvous on the one shared lock file `<state_root>/state/goal-board.lock`
/// (issue #2511).
#[cfg(unix)]
pub(super) fn board_lock_path() -> std::path::PathBuf {
    crate::state_root::goal_board_lock_path()
}

/// RAII guard holding an exclusive `flock` over [`board_lock_path`] for the
/// duration of a goal-board read-merge-write sequence.
///
/// [`save_goal_board`] already serializes *in-process* writers via
/// [`SAVE_GOAL_BOARD_MUTEX`] and merge-on-write (issue #1915), but that mutex
/// is process-local. The `simard goal add/remove` CLI runs in a *separate
/// process* from the OODA daemon, so the daemon's snapshot flush could land
/// between the CLI's snapshot read and its `store_fact`, silently clobbering
/// the just-added goal even though the CLI exited 0 (issue #2511). An advisory
/// `flock` on a shared file closes that cross-process window: the daemon's and
/// the CLI's read-merge-write sequences can no longer interleave.
///
/// Acquisition is **best-effort**: any filesystem error (unwritable state dir,
/// open/lock failure) is logged at `debug` and the caller proceeds *unlocked*
/// rather than failing the save. This preserves the existing fail-open
/// availability contract — the lock can only *prevent* the race, never
/// introduce a new way for goal persistence to error out. `flock` locks are
/// released automatically by the kernel on FD close or process death, so a
/// crashed holder never wedges the board.
#[cfg(unix)]
pub(super) struct BoardWriteLock {
    file: std::fs::File,
}

#[cfg(unix)]
impl BoardWriteLock {
    /// Best-effort acquire an exclusive (blocking) lock. Returns `None` (after
    /// logging at `debug`) when the lock file cannot be opened or locked.
    pub(super) fn acquire() -> Option<Self> {
        use std::os::unix::io::AsRawFd;

        let path = board_lock_path();
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            debug!(
                lock = "goal-board",
                error = %e,
                "BoardWriteLock: create lock dir failed; proceeding unlocked"
            );
            return None;
        }

        let file = match std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) => {
                debug!(
                    lock = "goal-board",
                    error = %e,
                    "BoardWriteLock: open lock file failed; proceeding unlocked"
                );
                return None;
            }
        };

        // Blocking exclusive lock. The guarded critical section (one snapshot
        // read + one merge + one store_fact) is short, so contention resolves
        // in milliseconds. flock treats independent open descriptions as
        // mutually exclusive even within one process, so this serializes the
        // daemon and CLI processes alike.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if ret != 0 {
            debug!(
                lock = "goal-board",
                error = %std::io::Error::last_os_error(),
                "BoardWriteLock: flock(LOCK_EX) failed; proceeding unlocked"
            );
            return None;
        }

        Some(Self { file })
    }
}

#[cfg(unix)]
impl Drop for BoardWriteLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        // Best-effort unlock; the kernel also releases the lock when `file`
        // drops (FD close) immediately after, so a failure here is benign.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// Returns `Some(reason)` if the board contains obviously corrupt or
/// placeholder goals that should not be accepted as valid loaded state.
///
/// The signature that actually correlates with fixture-leakage from
/// `tests_goal.rs` (issues #1923 / #1925) is the placeholder description
/// pattern `^goal [a-z0-9]{1,4}$` — `Goal alpha`, `Goal g1`, etc. The
/// previously-shipped "id shorter than 5 chars" heuristic was both too
/// permissive (real leaked fixtures used 5+ char ids like `alpha`,
/// `stuck-a`) and too aggressive (legitimate short operator-chosen ids
/// like `beta` tripped it). Description-based detection is retained as
/// the single source of truth.
pub fn board_integrity_suspect(board: &GoalBoard) -> Option<String> {
    for goal in &board.active {
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
    let s = desc.trim();
    if !s.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("goal")) {
        return false;
    }
    let rest = s[4..].trim();
    !rest.is_empty() && rest.len() <= 4 && rest.chars().all(|c| c.is_ascii_alphanumeric())
}

/// One-time migration: if a legacy `goal_records.json` exists on disk, read
/// it, store it in cognitive memory as the canonical snapshot, then delete
/// the file. Migration failures are logged and non-fatal — a corrupt or
/// unreadable file is left in place for operator inspection and the caller
/// proceeds to the cognitive-memory read path.
fn migrate_legacy_disk_file_if_present(memory: &dyn CognitiveMemoryOps) {
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
    if let Err(e) = memory.store_fact_with_caller_key(
        "goal-board:snapshot",
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
/// `goal-board:snapshot` fact via `memory.store_fact()` and read back via
/// `memory.search_facts()`.
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
/// 1. [`read_latest_snapshot`] — `memory.search_facts("goal-board:snapshot", 64, 0.0)`
///    filtered by `concept == "goal-board:snapshot"`, `max_by(node_id)`, parsed.
/// 2. `GoalBoard::new()` — empty board when no snapshot exists or parsing fails.
pub fn load_goal_board(memory: &dyn CognitiveMemoryOps) -> SimardResult<GoalBoard> {
    migrate_legacy_disk_file_if_present(memory);

    // Primary read path: cognitive memory snapshot via the shared helper.
    // The helper returns `None` on memory error, on zero results, or on a
    // payload parse failure — load_goal_board folds all three into the
    // legacy "empty board" fallback so callers see a stable contract.
    Ok(read_latest_snapshot(memory).unwrap_or_default())
}

/// Read the most recent `goal-board:snapshot` fact from cognitive memory,
/// or `None` if no snapshot is available.
///
/// Shared by [`load_goal_board`] (initial read) and [`save_goal_board`]
/// (merge-on-write read). All failure modes (memory error, empty result,
/// payload deserialization failure) return `None` with a `warn!` log line
/// that records the memory operation and error kind only — never the
/// payload, never goal descriptions.
///
/// `search_facts` is called with `limit=64, min_confidence=0.0` so that
/// the merge read can see recent snapshots even when the fact log has
/// accumulated. Fact ids are uuid-v7 (see `new_id()` in
/// `cognitive_memory/mod.rs`), so the lexicographically-largest id is the
/// most recent snapshot.
pub(super) fn read_latest_snapshot(memory: &dyn CognitiveMemoryOps) -> Option<GoalBoard> {
    let facts = match memory.search_facts("goal-board:snapshot", 64, 0.0) {
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
/// [`MAX_ACTIVE_GOALS`], it is truncated using a deterministic sort
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
/// 4. `memory.store_fact("goal-board:snapshot", &serde_json::to_string(&merged)?, 1.0, &["goal-board"], "goal-curator")`.
///    Fact metadata is constant — only the `GoalBoard` payload is merged.
///
/// **Best-effort guarantee.** No goal *added* on a disjoint subset
/// *disappears* in the common multi-client race. A tight
/// read-read-write-write interleaving across separate
/// `CognitiveMemoryOps` clients can still produce a snapshot that omits
/// the earlier writer's most recent fact; same-id concurrent edits
/// resolve field-level last-writer-wins. Callers needing strict
/// serializability must route through the daemon IPC socket.
pub fn save_goal_board(board: &GoalBoard, memory: &dyn CognitiveMemoryOps) -> SimardResult<()> {
    // NOTE: hermetic guard removed from this call-site. The env-var-based
    // `simard_state_root()` check raced with parallel tests that unset
    // SIMARD_STATE_ROOT (see CI failure on PR #2017). The
    // launch_writer_client guard now covers this path without env-var
    // dependency.

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

    // Cross-process serialization for the read-merge-write window (#2511).
    // Held for the remainder of the function so the daemon's snapshot flush
    // and a concurrent `simard goal` CLI mutation cannot interleave (the
    // in-process mutex above only covers threads of this process). Skipped
    // under `cfg(test)`: unit tests run single-process (already serialized by
    // the mutex) and resolve `simard_state_root()` from a parallel-mutated env
    // var, which would be non-deterministic at this call site (the reason the
    // hermetic guard was removed here, PR #2017). Production and integration
    // builds acquire the real lock.
    #[cfg(all(unix, not(test)))]
    let _board_lock = BoardWriteLock::acquire();

    // Step 2: read latest persisted snapshot (None on any failure).
    let persisted = read_latest_snapshot(memory);

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
    // Issue #2329: route the board snapshot through CallerKey dedup so each save
    // supersedes the prior board image instead of piling up a new revision every
    // cycle. The caller key and the concept are the same stable string.
    memory.store_fact_with_caller_key(
        "goal-board:snapshot",
        "goal-board:snapshot",
        &snapshot,
        1.0,
        &["goal-board".to_string()],
        "goal-curator",
    )?;
    Ok(())
}

/// Persist `board` with explicit removal of the goal ids in
/// `force_remove_ids`.
///
/// See `docs/reference/goal-board-api.md#save_goal_board_with_removals`
/// for the full contract. Introduced for issues
/// [#1923](https://github.com/rysweet/Simard/issues/1923) /
/// [#1925](https://github.com/rysweet/Simard/issues/1925) so an operator
/// can drop a known goal-id set from the persisted board without being
/// defeated by the merge-on-write resurrection failure mode that PR #1926
/// hit.
///
/// # Pipeline
///
/// 1. Test-only hermetic-state-root guard.
/// 2. Validate in-flight `board` via [`board_integrity_suspect`].
/// 3. Acquire [`SAVE_GOAL_BOARD_MUTEX`] so the read-merge-filter-write
///    window is serialized with [`save_goal_board`].
/// 4. Read the persisted snapshot.
/// 5. Merge persisted ⊕ in-flight (in-flight wins on collision).
/// 6. Filter goals (active + backlog) whose id is in `force_remove_ids`.
/// 7. Truncate `active` to [`MAX_ACTIVE_GOALS`] using the same
///    deterministic order [`merge_boards`] guarantees.
/// 8. `store_fact("goal-board:snapshot", merged_json, …)`.
///
/// # Idempotency
///
/// - Empty `force_remove_ids` → exactly equivalent to `save_goal_board`.
/// - Unknown ids → silent no-ops (no error).
/// - Duplicate ids in the slice → one removal each (de-duplicated).
pub fn save_goal_board_with_removals(
    board: &GoalBoard,
    force_remove_ids: &[String],
    memory: &dyn CognitiveMemoryOps,
) -> SimardResult<()> {
    // NOTE: hermetic guard removed — same reasoning as save_goal_board.
    // The launch_writer_client guard covers the hermetic property without
    // the racy simard_state_root() call.

    if let Some(reason) = board_integrity_suspect(board) {
        return Err(SimardError::InvalidGoalRecord {
            field: "board".to_string(),
            reason: format!("refusing to persist suspect board: {reason}"),
        });
    }

    let _critical = SAVE_GOAL_BOARD_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // Cross-process serialization for the read-merge-filter-write window
    // (#2511) — see the note in `save_goal_board`. Held for the remainder of
    // the function. Gated out of unit-test builds for the same reason.
    #[cfg(all(unix, not(test)))]
    let _board_lock = BoardWriteLock::acquire();

    let persisted = read_latest_snapshot(memory);
    let mut merged = match persisted {
        Some(p) => merge_boards(p, board.clone()),
        None => board.clone(),
    };

    if !force_remove_ids.is_empty() {
        let removals: BTreeSet<&str> = force_remove_ids.iter().map(String::as_str).collect();
        merged.active.retain(|g| !removals.contains(g.id.as_str()));
        merged.backlog.retain(|b| !removals.contains(b.id.as_str()));
    }

    // Re-apply the MAX_ACTIVE_GOALS cap as a defence in depth — removals
    // shrink, never grow, the active list, but a future caller passing a
    // pre-merged board larger than the cap should not produce a snapshot
    // that violates the invariant.
    if merged.active.len() > MAX_ACTIVE_GOALS {
        merged.active.truncate(MAX_ACTIVE_GOALS);
    }

    debug!(
        merge = "goal-board",
        in_flight_active = board.active.len(),
        in_flight_backlog = board.backlog.len(),
        force_removed = force_remove_ids.len(),
        merged_active = merged.active.len(),
        merged_backlog = merged.backlog.len(),
        "save_goal_board_with_removals: persisting filtered snapshot"
    );

    let snapshot = serde_json::to_string(&merged).map_err(|e| SimardError::InvalidGoalRecord {
        field: "board".to_string(),
        reason: format!("failed to serialize goal board: {e}"),
    })?;
    // Issue #2329: CallerKey dedup — supersede the prior board image.
    memory.store_fact_with_caller_key(
        "goal-board:snapshot",
        "goal-board:snapshot",
        &snapshot,
        1.0,
        &["goal-board".to_string()],
        "goal-curator",
    )?;
    Ok(())
}

/// Persist the board state and record an episode for recall.
pub fn persist_board(board: &GoalBoard, memory: &dyn CognitiveMemoryOps) -> SimardResult<()> {
    save_goal_board(board, memory)?;
    memory.store_episode(
        &board.durable_summary(),
        "goal-curator",
        Some(&json!({"active_count": board.active.len(), "backlog_count": board.backlog.len()})),
    )?;
    Ok(())
}

/// Overwrite the cognitive-memory board **cache** with `board` exactly,
/// bypassing the merge-on-write path.
///
/// Issue #1: with [`crate::goal_board_store`] now the single authoritative
/// source of truth, the cognitive-memory `goal-board:snapshot` fact is a
/// *derived cache* the daemon regenerates from the authoritative file each
/// cycle. [`save_goal_board`] performs a union merge-on-write (correct when
/// memory *was* the source of truth) which would resurrect goals the authority
/// dropped; this helper instead stores `board` verbatim via the CallerKey-dedup
/// fact so `read_latest_snapshot` returns exactly the authoritative board.
///
/// Best-effort by contract: the dashboard and recall read this cache, but the
/// authoritative file governs, so a cache-write failure is non-fatal.
pub fn overwrite_memory_cache(
    board: &GoalBoard,
    memory: &dyn CognitiveMemoryOps,
) -> SimardResult<()> {
    let snapshot = serde_json::to_string(board).map_err(|e| SimardError::InvalidGoalRecord {
        field: "board".to_string(),
        reason: format!("failed to serialize goal board: {e}"),
    })?;
    memory.store_fact_with_caller_key(
        "goal-board:snapshot",
        "goal-board:snapshot",
        &snapshot,
        1.0,
        &["goal-board".to_string()],
        "goal-curator",
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

/// Default backlog score for Overseer-created work.
pub const DEFAULT_STEWARD_SCORE: f64 = 0.6;

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
    let promoted_source =
        crate::goal_curation::labels::source_for_backlog(&board.backlog[position].source);
    let goal = ActiveGoal {
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: board.backlog[position].id.clone(),
        description: board.backlog[position].description.clone(),
        priority,
        status: GoalProgress::NotStarted,
        assigned_to,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
        labels: vec![promoted_source.to_string()],
    };
    // Centralized, fail-closed admission validation (issue #4930): every seam
    // that admits a record — `add_active_goal` (direct active add),
    // `add_backlog_item` (backlog admission), and this backlog→active promotion
    // — must run the same required-field/priority gate. Prior to this, promotion
    // only checked `validate_priority`, silently admitting into `board.active` a
    // goal with an empty id/description that the direct-add path rejects.
    // Validated BEFORE removing the item from the backlog so a rejected
    // promotion leaves the board untouched (no goal silently lost).
    validate_active_goal(&goal)?;
    board.backlog.remove(position);
    board.active.push(goal);
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

/// Map a [`GoalProgress`] variant to its effective percent (0–100) for
/// gate comparison. `Blocked` keeps the *current* percent since it does
/// not change progress numerically; callers pass `current` for that.
fn progress_to_percent(p: &GoalProgress, current: u32) -> u32 {
    match p {
        GoalProgress::Proposed => 0,
        GoalProgress::NotStarted => 0,
        GoalProgress::InProgress { percent } => *percent,
        GoalProgress::Blocked(_) => current,
        GoalProgress::Paused => current,
        GoalProgress::Completed => 100,
    }
}

/// Variant label for bypass reason strings.
fn variant_label(p: &GoalProgress) -> &'static str {
    match p {
        GoalProgress::Proposed => "proposed",
        GoalProgress::NotStarted => "not-started",
        GoalProgress::InProgress { .. } => "in-progress",
        GoalProgress::Blocked(_) => "blocked",
        GoalProgress::Paused => "paused",
        GoalProgress::Completed => "completed",
    }
}

/// Progress-evidence gatekeeper façade (issue #1967).
///
/// Routes every proposed mutation of [`ActiveGoal::status`] through a
/// [`ProgressEvidenceChecker`]. Behaviour summary:
///
/// * **Bypass** (no evidence consulted, no audit episode):
///   - Decreases and same-value `InProgress` writes
///   - `Blocked(_)` transitions (kept at prior percent)
///   - `NotStarted` resets
///
/// * **Otherwise** (proposal is an *increase* over `old_percent`):
///   1. Source `since` via the three-step fallback chain:
///      a. `goal.last_progress_update_at`
///      b. Most recent `"goal progress accepted: …<goal_id>…"` episode
///      c. [`progress_evidence::process_start`]
///   2. Call `checker.check(...)`.
///   3. On `Accept`: write through, stamp `last_progress_update_at =
///      Some(now)`, emit one `"goal progress accepted: …"` episode.
///   4. On `Reject`: keep prior percent, emit one
///      `"brain hallucination detected: …"` episode.
///
/// Both `Accept` and `Reject` are returned as `Ok(...)`. `Err` is
/// reserved for genuine failures (goal not found, underlying writer
/// fails, memory `store_episode` fails).
pub fn update_goal_progress_with_evidence(
    board: &mut GoalBoard,
    goal_id: &str,
    proposed: GoalProgress,
    checker: &dyn super::progress_evidence::ProgressEvidenceChecker,
    memory: &dyn CognitiveMemoryOps,
    now: chrono::DateTime<chrono::Utc>,
) -> SimardResult<super::progress_evidence::EvidenceDecision> {
    use super::progress_evidence::{EvidenceDecision, process_start};

    // ── Look up and snapshot the goal; reused for percent comparison and
    //    checker.check() below, avoiding a second linear scan. ─────────
    let goal_snapshot = board
        .active
        .iter()
        .find(|g| g.id == goal_id)
        .ok_or_else(|| SimardError::InvalidGoalRecord {
            field: "goal_id".to_string(),
            reason: format!("active goal '{goal_id}' not found"),
        })?
        .clone();
    let old_percent = progress_to_percent(&goal_snapshot.status, 0);
    let new_percent = progress_to_percent(&proposed, old_percent);
    let last_update_at_field = goal_snapshot.last_progress_update_at;

    // ── Bypass set ────────────────────────────────────────────────────
    let is_bypass = matches!(proposed, GoalProgress::Blocked(_))
        || matches!(proposed, GoalProgress::NotStarted)
        || matches!(proposed, GoalProgress::Proposed)
        || matches!(proposed, GoalProgress::Paused)
        || new_percent <= old_percent;

    if is_bypass {
        let bypass_reason = if matches!(proposed, GoalProgress::Blocked(_)) {
            "bypass: blocked".to_string()
        } else if matches!(proposed, GoalProgress::NotStarted) {
            "bypass: not-started".to_string()
        } else if matches!(proposed, GoalProgress::Proposed) {
            "bypass: proposed".to_string()
        } else if matches!(proposed, GoalProgress::Paused) {
            "bypass: paused".to_string()
        } else if new_percent < old_percent {
            "bypass: non-increase (decrease)".to_string()
        } else {
            format!("bypass: non-increase ({})", variant_label(&proposed))
        };
        update_goal_progress(board, goal_id, proposed)?;
        return Ok(EvidenceDecision::Accept {
            reason: bypass_reason,
        });
    }

    // ── Source `since` via three-step fallback ───────────────────────
    let mut since = if let Some(t) = last_update_at_field {
        t
    } else {
        // Memory scan: most recent "goal progress accepted: …<goal_id>…"
        // episode. We over-fetch a generous limit and filter to those
        // mentioning this goal id.
        let prefix = "goal progress accepted:";
        let hits = memory
            .search_episodes_starting_with(prefix, 64)
            .unwrap_or_default();
        let matched: Option<chrono::DateTime<chrono::Utc>> = hits
            .into_iter()
            .filter(|(content, _)| content.contains(goal_id))
            .map(|(_, at)| at)
            .max();
        matched.unwrap_or_else(process_start)
    };
    // Clamp `since` to `now` so the cached `process_start` fallback
    // never produces a `since` in the future when the caller's `now`
    // happens to predate the daemon process start (test fixtures with
    // historical timestamps; daylight-savings/NTP corrections in prod).
    if since > now {
        since = now;
    }

    // ── Consult checker ───────────────────────────────────────────────
    let decision = checker.check(&goal_snapshot, old_percent, new_percent, since);

    match decision {
        EvidenceDecision::Accept { reason } => {
            update_goal_progress(board, goal_id, proposed)?;
            // Stamp the timestamp for next time.
            if let Some(g) = board.active.iter_mut().find(|g| g.id == goal_id) {
                g.last_progress_update_at = Some(now);
            }
            let episode = format!(
                "goal progress accepted: {old_percent}%->{new_percent}% on {goal_id}\n  -- evidence: {reason}"
            );
            // Best-effort audit episode — memory failures propagate.
            let _ = memory.store_episode(
                &episode,
                "progress-evidence-gate",
                Some(&json!({
                    "kind": "progress_accepted",
                    "goal_id": goal_id,
                    "old_percent": old_percent,
                    "new_percent": new_percent,
                    "reason": reason,
                    "at": now.to_rfc3339(),
                })),
            )?;
            Ok(EvidenceDecision::Accept { reason })
        }
        EvidenceDecision::Reject { reason } => {
            // Do NOT mutate the board. Emit a hallucination alert.
            let episode = format!(
                "brain hallucination detected: rejected progress {old_percent}%->{new_percent}% on {goal_id}\n  -- reviewer rationale: {reason}"
            );
            memory.store_episode(
                &episode,
                "progress-evidence-gate",
                Some(&json!({
                    "kind": "progress_rejected",
                    "goal_id": goal_id,
                    "old_percent": old_percent,
                    "new_percent": new_percent,
                    "reason": reason,
                    "since": since.to_rfc3339(),
                    "at": now.to_rfc3339(),
                })),
            )?;
            Ok(EvidenceDecision::Reject { reason })
        }
    }
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
///
/// Standing/perpetual goals (issue #2580) are never removed: a standing goal
/// that reached a terminal-looking status is rolled into a fresh cycle in place
/// (see [`ActiveGoal::roll_to_new_cycle`]) and kept on the board, so perpetual
/// research / stewardship work is never silently dropped.
pub fn archive_completed(board: &mut GoalBoard) -> Vec<ActiveGoal> {
    let mut archived = Vec::new();
    board.active.retain_mut(|goal| {
        let dominated = goal.status.is_terminal();
        if !dominated {
            return true;
        }
        if goal.is_perpetual() {
            // Never terminate a standing goal — roll it to a fresh cycle.
            goal.roll_to_new_cycle();
            true
        } else {
            archived.push(goal.clone());
            false
        }
    });
    archived
}

// ---------------------------------------------------------------------------
// Goal carryover API (issue #2092)
// ---------------------------------------------------------------------------

/// Compute a SHA-256 hex digest of the serialized `GoalBoard`.
///
/// Used to detect board drift between the meeting close (write) and the
/// engineer startup (read). Deterministic because `GoalBoard` derives
/// `Serialize` with field-order stability and `serde_json::to_string`
/// produces a canonical representation.
pub fn board_snapshot_hash(board: &GoalBoard) -> String {
    use std::hash::{Hash, Hasher};
    // We use a lightweight approach: hash the JSON serialization.
    // This is deterministic because serde_json field order is stable for
    // structs (not maps), and GoalBoard uses Vec, not HashMap.
    let json = serde_json::to_string(board).unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    json.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Write a carryover record to cognitive memory after the meeting close
/// pipeline persists goal updates.
///
/// The record captures:
/// - which meeting produced the goals (`meeting_id`)
/// - a hash of the board snapshot for drift detection
/// - the ids and count of active goals for verification
///
/// The engineer loop reads this back via [`verify_goal_carryover`] and
/// fails loudly when the record is missing or the board has drifted.
pub fn write_goal_carryover(
    board: &GoalBoard,
    meeting_id: &str,
    memory: &dyn CognitiveMemoryOps,
) -> SimardResult<()> {
    let record = GoalCarryoverRecord {
        meeting_id: meeting_id.to_string(),
        handed_off_at: chrono::Utc::now(),
        board_snapshot_hash: board_snapshot_hash(board),
        active_goal_count: board.active.len(),
        active_goal_ids: board.active.iter().map(|g| g.id.clone()).collect(),
        acknowledged: false,
    };
    let payload = serde_json::to_string(&record).map_err(|e| SimardError::InvalidGoalRecord {
        field: "carryover".to_string(),
        reason: format!("failed to serialize carryover record: {e}"),
    })?;
    memory.store_fact(
        CARRYOVER_CONCEPT,
        &payload,
        1.0,
        &["goal-board".to_string(), "carryover".to_string()],
        "meeting-close",
    )?;
    debug!(
        meeting_id = meeting_id,
        active_goals = board.active.len(),
        "write_goal_carryover: carryover record written"
    );
    Ok(())
}

/// Read the most recent carryover record from cognitive memory, if any.
pub fn read_latest_carryover(
    memory: &dyn CognitiveMemoryOps,
) -> SimardResult<Option<GoalCarryoverRecord>> {
    let facts = memory.search_facts(CARRYOVER_CONCEPT, 64, 0.0)?;
    let latest = facts
        .iter()
        .filter(|f| f.concept == CARRYOVER_CONCEPT)
        .max_by(|a, b| a.node_id.cmp(&b.node_id));
    match latest {
        Some(fact) => match serde_json::from_str::<GoalCarryoverRecord>(&fact.content) {
            Ok(record) => Ok(Some(record)),
            Err(e) => {
                warn!(
                    concept = CARRYOVER_CONCEPT,
                    error_kind = %e,
                    "read_latest_carryover: parse failed"
                );
                Ok(None)
            }
        },
        None => Ok(None),
    }
}

/// Outcome of [`verify_goal_carryover`].
#[derive(Clone, Debug, PartialEq)]
pub enum CarryoverVerification {
    /// No carryover record exists — either the system has never had a
    /// meeting or the board was seeded without one. The engineer loop
    /// should proceed normally (first-run scenario).
    NoRecord,
    /// The carryover record exists and the current board matches the
    /// handed-off state.
    Verified {
        meeting_id: String,
        active_goal_count: usize,
    },
    /// The carryover record exists but the board has drifted — goals
    /// may have been lost. The engineer loop should surface this as a
    /// warning.
    Drifted {
        meeting_id: String,
        expected_hash: String,
        actual_hash: String,
        missing_goal_ids: Vec<String>,
    },
}

/// Verify that the engineer session's goal board matches the most recent
/// meeting carryover record.
///
/// Returns [`CarryoverVerification::NoRecord`] when no carryover exists
/// (first run or no meetings yet). Returns [`CarryoverVerification::Verified`]
/// when the board hash and goal ids match. Returns
/// [`CarryoverVerification::Drifted`] when goals may have been lost.
///
/// The engineer loop calls this on startup (spec line 665) so goal loss
/// surfaces as a clear warning rather than silent data disappearance.
pub fn verify_goal_carryover(
    board: &GoalBoard,
    memory: &dyn CognitiveMemoryOps,
) -> SimardResult<CarryoverVerification> {
    let record = match read_latest_carryover(memory)? {
        Some(r) => r,
        None => return Ok(CarryoverVerification::NoRecord),
    };

    let current_hash = board_snapshot_hash(board);
    let current_ids: std::collections::BTreeSet<&str> =
        board.active.iter().map(|g| g.id.as_str()).collect();

    let missing: Vec<String> = record
        .active_goal_ids
        .iter()
        .filter(|id| !current_ids.contains(id.as_str()))
        .cloned()
        .collect();

    if missing.is_empty() {
        Ok(CarryoverVerification::Verified {
            meeting_id: record.meeting_id,
            active_goal_count: board.active.len(),
        })
    } else {
        Ok(CarryoverVerification::Drifted {
            meeting_id: record.meeting_id,
            expected_hash: record.board_snapshot_hash,
            actual_hash: current_hash,
            missing_goal_ids: missing,
        })
    }
}

/// The 5 default starter goals shared by both `seed_default_board` (GoalBoard)
/// and `seed_default_goals` (GoalStore). Single source of truth.
///
/// Each tuple: (priority, title, description, target-repo slug). The repo slug
/// is `None` for goals that target the daemon's own repo ("Simard") and
/// `Some(slug)` for ecosystem-targeted goals (issue #2359, BUG 1).
pub const DEFAULT_SEED_GOALS: [(u32, &str, &str, Option<&str>); 5] = [
    (
        1,
        "Improve amplihack test coverage",
        "Increase test coverage across the amplihack ecosystem to catch regressions early",
        Some("amplihack-rs"),
    ),
    (
        2,
        "Enhance Simard meeting experience",
        "Improve the interactive meeting facilitator with better UX and richer handoffs",
        None,
    ),
    (
        3,
        "Improve cognitive memory persistence",
        "Harden memory consolidation and ensure durable recall across sessions",
        None,
    ),
    (
        4,
        "Fix broken features",
        "Analyze all Simard features against their specs and intended behavior. Identify features that are not working correctly (e.g., meeting REPL, any other broken functionality) and fix them. Prioritize by user impact. Start by auditing the Specs/ directory and comparing each spec against the actual implementation to find gaps and failures.",
        None,
    ),
    (
        5,
        "Self-serve dashboard improvement",
        "Use your own dashboard (localhost:8080) with Playwright to understand your operations and memory. Continuously improve the dashboard until it is very useful for understanding your internal state. The dashboard must not use jargon and must remain useful to humans too. Login by reading the code from ~/.simard/.dashkey. Playwright is installed (playwright==1.59.0 with Chromium browser).",
        None,
    ),
];

/// Seed the board with 5 default starter goals if it has no active goals.
/// Returns the number of goals added.
pub fn seed_default_board(board: &mut GoalBoard) -> usize {
    if !board.active.is_empty() {
        return 0;
    }

    for (priority, id_source, description, repo) in DEFAULT_SEED_GOALS {
        let id = crate::goals::goal_slug(id_source);
        board.active.push(ActiveGoal {
            parent_goal_id: None,
            priority_explicit: false,
            id,
            description: description.to_string(),
            priority,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            repo: repo.map(str::to_string),
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
            labels: vec![crate::goal_curation::labels::SOURCE_SEED.to_string()],
        });
    }

    DEFAULT_SEED_GOALS.len()
}

/// `DEFAULT_SEED_GOALS` projected as owned [`crate::identity::SeedGoal`] values.
///
/// This is the single shape shared by Simard's baked-in defaults and an
/// identity's declared seed goals (#3125), so the seeding site can treat both
/// uniformly. `title` is the tuple's `id_source` (the slug source).
pub fn default_seed_goals() -> Vec<crate::identity::SeedGoal> {
    DEFAULT_SEED_GOALS
        .iter()
        .map(|(priority, title, description, repo)| {
            crate::identity::SeedGoal::new(
                *priority,
                *title,
                *description,
                repo.map(str::to_string),
            )
        })
        .collect()
}

/// Resolve which seed goals to use at the OODA cold-start seeding site (#3125).
///
/// When the identity declares a non-empty `seed_goals` list it **overrides**
/// Simard's baked-in `DEFAULT_SEED_GOALS` (no merge); an empty list falls
/// through to [`default_seed_goals`], so Simard herself is unchanged.
pub fn resolve_seed_goals(
    identity_seed_goals: &[crate::identity::SeedGoal],
) -> Vec<crate::identity::SeedGoal> {
    if identity_seed_goals.is_empty() {
        default_seed_goals()
    } else {
        identity_seed_goals.to_vec()
    }
}

/// Seed the board from an explicit list of [`crate::identity::SeedGoal`] if it
/// has no active goals. Returns the number of goals added. Mirrors
/// [`seed_default_board`] exactly (same `SOURCE_SEED` label, same empty-board
/// guard), differing only in that the goals come from the resolved identity /
/// default list rather than the baked-in `DEFAULT_SEED_GOALS` tuple. A goal's
/// `repo` slug scopes it to the identity's target repo (#3125), never to
/// `rysweet/Simard`.
pub fn seed_board_from_seed_goals(
    board: &mut GoalBoard,
    goals: &[crate::identity::SeedGoal],
) -> usize {
    if !board.active.is_empty() {
        return 0;
    }

    for goal in goals {
        let id = crate::goals::goal_slug(&goal.title);
        board.active.push(ActiveGoal {
            parent_goal_id: None,
            priority_explicit: false,
            id,
            description: goal.description.clone(),
            priority: goal.priority,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            repo: goal.repo.clone(),
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
            labels: vec![crate::goal_curation::labels::SOURCE_SEED.to_string()],
        });
    }

    goals.len()
}

// ---------------------------------------------------------------------------
// GoalBoard -> Vec<GoalRecord> adapter
// ---------------------------------------------------------------------------

/// Sentinel `SessionId` used to populate `GoalRecord::source_session_id`
/// for records synthesised from the cognitive-memory-backed `GoalBoard`.
/// The board has no per-goal session provenance, so we mark these records
/// as originating from the "all-zeros" session so callers can distinguish
/// them from session-sourced goals.
///
/// Cached via `LazyLock` to avoid re-parsing the UUID string on every call.
static SENTINEL_SESSION_ID: LazyLock<crate::session::SessionId> = LazyLock::new(|| {
    crate::session::SessionId::parse("00000000-0000-0000-0000-000000000000")
        .expect("sentinel uuid must parse")
});

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
/// | `status`             | `Completed → GoalStatus::Completed`, `Proposed → GoalStatus::Proposed`, `Paused → GoalStatus::Paused`, others → `GoalStatus::Active` |
/// | `priority`           | `u8::try_from(active.priority).unwrap_or(u8::MAX)`                |
/// | `owner_identity`     | `active.assigned_to.clone().unwrap_or_else(\|\| "unassigned".into())` |
/// | `source_session_id`  | sentinel `00000000-0000-0000-0000-000000000000`                   |
/// | `updated_in`         | `SessionPhase::Persistence`                                       |
///
/// Backlog items are not emitted — only the active goals surface here, which
/// matches the legacy `FileBackedGoalStore::active_top_goals(...)` contract.
pub fn active_goals_as_records(board: &GoalBoard) -> Vec<crate::goals::GoalRecord> {
    board
        .active
        .iter()
        .map(|active| {
            let title_first_line = active.description.lines().next().unwrap_or("");
            let title: String = title_first_line.chars().take(120).collect();

            let status = match &active.status {
                GoalProgress::Proposed => crate::goals::GoalStatus::Proposed,
                GoalProgress::Paused => crate::goals::GoalStatus::Paused,
                GoalProgress::Completed => crate::goals::GoalStatus::Completed,
                _ => crate::goals::GoalStatus::Active,
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
                source_session_id: SENTINEL_SESSION_ID.clone(),
                updated_in: crate::session::SessionPhase::Persistence,
                evidence: Vec::new(),
                labels: active.labels.clone(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// GoalRecord -> board placement adapter (reverse of active_goals_as_records)
// ---------------------------------------------------------------------------

/// Where a persisted [`GoalRecord`] lands when mapped back onto the live board
/// (issue #2922). The inverse of [`active_goals_as_records`]: it renders an
/// overlay `goal-store:record` — a promoted creative-idea Proposed goal, a
/// meeting goal, a seed/runtime goal — in the SAME `ActiveGoal` / `BacklogItem`
/// shapes a snapshot goal uses, so the dashboard's live union renders overlay
/// and snapshot goals identically.
///
/// `Skip` drops terminal (`Completed`) records: they are not surfaced on the
/// live board in either bucket.
#[derive(Debug)]
// An `ActiveGoal` is materially larger than the other two variants; the doc
// contract (#2922) is that this enum stays unboxed so callers can move the
// payload out and `{:?}`-format it, so we accept the size disparity rather than
// box the variant.
#[allow(clippy::large_enum_variant)]
pub enum BoardPlacement {
    Active(ActiveGoal),
    Backlog(BacklogItem),
    Skip,
}

/// Map a persisted [`GoalRecord`] back into its live-board placement (issue
/// #2922) — the inverse of [`active_goals_as_records`].
///
/// Status routing mirrors the board's own active/backlog split:
///
/// | `GoalStatus` | Placement | Rendered progress |
/// |--------------|-----------|-------------------|
/// | `Active`     | active    | `InProgress { percent: 0 }` |
/// | `Proposed`   | backlog   | — |
/// | `Paused`     | backlog   | — |
/// | `Completed`  | skipped   | terminal |
///
/// A `GoalRecord` carries none of the snapshot-only rich fields, so they are
/// synthesized as `None` / `[]` / `false`. Pure struct mapping — panic-free on
/// arbitrary record text: overlay records carry untrusted, model-generated
/// content and a panic here would 500 the dashboard read path.
pub fn record_as_active_goal(record: &crate::goals::GoalRecord) -> BoardPlacement {
    match record.status {
        crate::goals::GoalStatus::Completed => BoardPlacement::Skip,
        crate::goals::GoalStatus::Active => {
            // The forward adapter writes the `"unassigned"` sentinel for a
            // goal with no assignee; map it back to `None` so the round-trip
            // is faithful.
            let assigned_to = match record.owner_identity.as_str() {
                "unassigned" => None,
                owner => Some(owner.to_string()),
            };
            BoardPlacement::Active(ActiveGoal {
                id: record.slug.clone(),
                description: record.title.clone(),
                priority: u32::from(record.priority),
                status: GoalProgress::InProgress { percent: 0 },
                assigned_to,
                repo: None,
                current_activity: None,
                wip_refs: Vec::new(),
                last_progress_update_at: None,
                parent_goal_id: None,
                priority_explicit: false,
                labels: record.labels.clone(),
            })
        }
        crate::goals::GoalStatus::Proposed | crate::goals::GoalStatus::Paused => {
            BoardPlacement::Backlog(BacklogItem {
                id: record.slug.clone(),
                description: record.title.clone(),
                source: super::labels::provenance_source_label(&record.labels).to_string(),
                score: backlog_score_for_priority(record.priority),
            })
        }
    }
}

/// Deterministic backlog score from a goal's priority so the proposed backlog
/// orders stably (issue #2922). Higher priority — a LOWER number, p1 is most
/// important — yields a higher score. The priority is clamped into `[1, 10]`
/// and mapped to `(0, 1]` via `(11 - p) / 10`, so p1 -> 1.0, p3 -> 0.8, and
/// p10+ -> 0.1. Always finite; never panics for any `u8`.
fn backlog_score_for_priority(priority: u8) -> f64 {
    let p = f64::from(priority.clamp(1, 10));
    (11.0 - p) / 10.0
}
