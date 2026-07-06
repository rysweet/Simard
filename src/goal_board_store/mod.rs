//! Authoritative, durable goal-board store (issue #1).
//!
//! # Why this exists
//!
//! Before this module the goal board lived only as **searchable semantic
//! facts** in cognitive memory (`goal-board:snapshot`), read back via
//! `search_facts(...).max_by(node_id)` and rewritten every OODA cycle through a
//! *union-by-id* `merge_boards`. That architecture had three fatal properties
//! for a steerable daemon:
//!
//! 1. **Not read-your-writes.** A snapshot is one of many versioned facts; a
//!    stale replica or an out-of-order read can surface an old board.
//! 2. **Clobbering.** The daemon re-saves its *in-memory* board each cycle. An
//!    operator `goal remove` (or a meeting handoff) writes a new snapshot, but
//!    the daemon's next `merge_boards` unions its still-in-memory copy back on
//!    top — resurrecting the just-removed goal even though the CLI exited `0`.
//! 3. **No durable process state.** The no-progress breaker's per-goal counter
//!    lived only in `OodaState`, so the daemon's ~hourly process restart reset
//!    it to zero before it could ever reach the threshold.
//!
//! # The single authoritative store
//!
//! This module makes **one durable file** — `<state_root>/state/goal_board.json`
//! — the single source of truth. It is guarded by the same cross-process
//! advisory `flock` that already serialises board writes
//! ([`crate::state_root::goal_board_lock_path`], issue #2511) and mutated with
//! an **atomic read-modify-write** (temp file + `rename`). [`load`] returns
//! exactly the last committed state (read-your-writes), always. The cognitive
//! memory snapshot is demoted to a **derived cache** the daemon overwrites from
//! this file each cycle (see
//! [`crate::goal_curation::overwrite_memory_cache`]).
//!
//! The store also persists the [`NoProgressTracker`] so the breaker's counters
//! survive daemon restarts, and it reconciles operator edits against the daemon's
//! in-flight board **honouring tombstones** ([`reconcile`]) so a removed or
//! completed goal is never clobbered back onto the board.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::goal_curation::completion_gate::{
    CompletionEvidenceGate, CompletionVerdict, EvidenceSource,
};
use crate::goal_curation::{
    ActiveGoal, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS, NoProgressTracker, is_no_progress_marker,
};

#[cfg(test)]
mod tests;

/// On-disk schema version for [`PersistentGoalState`]. Bump on incompatible
/// layout changes so a loader can migrate; `#[serde(default)]` keeps older
/// files (which lacked the field) deserializable as version 0.
pub const STORE_VERSION: u32 = 1;

/// The complete durable goal-board state, serialised to `goal_board.json`.
///
/// Every field carries `#[serde(default)]` so a partially-written or older
/// file still deserialises into a usable value rather than failing the load.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistentGoalState {
    /// Schema version of the persisted file (see [`STORE_VERSION`]).
    #[serde(default)]
    pub version: u32,
    /// The authoritative goal board (active goals + scored backlog).
    #[serde(default)]
    pub board: GoalBoard,
    /// The no-progress breaker's per-goal consecutive-no-action counters,
    /// persisted so a livelock spanning a daemon restart is still bounded.
    #[serde(default)]
    pub no_progress: NoProgressTracker,
    /// The brain's total lived OODA cognition: a monotonic cycle counter
    /// persisted across daemon restarts (issue #1). Before this field the OODA
    /// cycle number lived only in `OodaState::cycle_count`, so every daemon
    /// restart (a frequent deploy) reset it to 1 and the dashboard perpetually
    /// showed "Cycle #1", erasing the sense of accumulated cognitive activity.
    /// The daemon seeds `OodaState::cycle_count` from this at startup and
    /// re-stamps it every `commit_cycle`, under the same `flock`, so the number
    /// reflects the brain's persistent memory rather than process uptime.
    #[serde(default)]
    pub cycle_count: u32,
}

/// Absolute path of the authoritative goal-board file for `state_root`.
pub fn store_path(state_root: &Path) -> PathBuf {
    state_root.join("state").join("goal_board.json")
}

// ---------------------------------------------------------------------------
// Cross-process advisory lock (shared with goal_curation::save_goal_board)
// ---------------------------------------------------------------------------

/// RAII guard holding an exclusive `flock` over
/// [`crate::state_root::goal_board_lock_path`] for the duration of an atomic
/// read-modify-write. Shares the **same physical lock file**
/// (`<state_root>/state/goal-board.lock`) as
/// [`crate::goal_curation::save_goal_board`] so a daemon cycle flush and a
/// concurrent `simard goal` CLI mutation can never interleave (issue #2511).
///
/// Acquisition is best-effort: any filesystem error is logged at `debug` and
/// the caller proceeds *unlocked* rather than failing the mutation — the lock
/// can only prevent the race, never introduce a new failure mode. `flock`
/// releases on FD close or process death, so a crashed holder never wedges the
/// board.
#[cfg(unix)]
struct StoreLock {
    file: std::fs::File,
}

#[cfg(unix)]
impl StoreLock {
    fn acquire(state_root: &Path) -> Option<Self> {
        use std::os::unix::io::AsRawFd;

        let path = crate::state_root::goal_board_lock_path_in(state_root);
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            tracing::debug!(lock = "goal-board", error = %e, "StoreLock: create dir failed; unlocked");
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
                tracing::debug!(lock = "goal-board", error = %e, "StoreLock: open failed; unlocked");
                return None;
            }
        };
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if ret != 0 {
            tracing::debug!(
                lock = "goal-board",
                error = %std::io::Error::last_os_error(),
                "StoreLock: flock(LOCK_EX) failed; unlocked"
            );
            return None;
        }
        Some(Self { file })
    }
}

#[cfg(unix)]
impl Drop for StoreLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// Acquire the cross-process store lock. A no-op on non-unix targets (the
/// project ships unix-only, but keeping the guard `cfg`-gated avoids a build
/// break on doc/lint passes for other targets).
#[cfg(unix)]
fn lock(state_root: &Path) -> Option<StoreLock> {
    StoreLock::acquire(state_root)
}

#[cfg(not(unix))]
fn lock(_state_root: &Path) -> Option<()> {
    None
}

// ---------------------------------------------------------------------------
// Read / write primitives
// ---------------------------------------------------------------------------

/// Read the persisted state without taking the lock. Returns the default
/// (empty) state when the file is absent, unreadable, or corrupt — a missing or
/// damaged file must never crash the daemon; it starts from an empty board.
fn read_unlocked(state_root: &Path) -> PersistentGoalState {
    let path = store_path(state_root);
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "goal_board_store: corrupt file; starting from empty state"
            );
            PersistentGoalState::default()
        }),
        Err(_) => PersistentGoalState::default(),
    }
}

/// Atomically write `state` to the store file (temp file + `rename`), creating
/// the parent directory if needed. The `rename` is atomic on the same
/// filesystem, so a concurrent reader sees either the old or the new file
/// whole — never a torn write.
fn write_atomic(state_root: &Path, state: &PersistentGoalState) -> SimardResult<()> {
    let path = store_path(state_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SimardError::ArtifactIo {
            path: parent.to_path_buf(),
            reason: format!("creating goal-board store dir: {e}"),
        })?;
    }
    let json = serde_json::to_string_pretty(state).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("serialising goal-board store: {e}"),
    })?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json.as_bytes()).map_err(|e| SimardError::ArtifactIo {
        path: tmp.clone(),
        reason: format!("writing goal-board temp file: {e}"),
    })?;
    std::fs::rename(&tmp, &path).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("renaming goal-board temp file into place: {e}"),
    })?;
    Ok(())
}

/// Load the authoritative state (read-your-writes). Takes the shared lock for
/// the duration of the read so a partially-committed peer write is never
/// observed.
pub fn load(state_root: &Path) -> PersistentGoalState {
    let _guard = lock(state_root);
    read_unlocked(state_root)
}

/// Atomically read-modify-write the authoritative state under the shared lock.
///
/// Reads the current file, hands a mutable reference to `f`, stamps the current
/// [`STORE_VERSION`], and writes the result atomically. The whole sequence runs
/// while the cross-process `flock` is held, so two processes (daemon + CLI)
/// serialise their read-modify-write windows and cannot lose each other's
/// updates. Returns whatever `f` returns.
pub fn mutate<R>(
    state_root: &Path,
    f: impl FnOnce(&mut PersistentGoalState) -> R,
) -> SimardResult<R> {
    let _guard = lock(state_root);
    let mut state = read_unlocked(state_root);
    state.version = STORE_VERSION;
    let out = f(&mut state);
    write_atomic(state_root, &state)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tombstone-aware reconciliation
// ---------------------------------------------------------------------------

/// Drop every active goal and backlog item whose id is tombstoned. Defensive
/// filter applied at load and commit so a tombstoned goal can never survive on
/// the board even if some other path (default seeding, memory recall, a meeting
/// handoff) tried to re-introduce it.
#[must_use]
pub fn filter_tombstoned(mut board: GoalBoard, tombstones: &HashSet<String>) -> GoalBoard {
    board.active.retain(|g| !tombstones.contains(&g.id));
    board.backlog.retain(|b| !tombstones.contains(&b.id));
    board
}

/// Self-heal a stale no-progress hard-block on a standing/perpetual goal
/// (issue #2589).
///
/// A standing/perpetual goal is inherently bursty and is **exempt** from the
/// no-progress breaker at runtime (see
/// [`crate::ooda_loop::no_progress::apply_no_progress_breaker`]). But a goal
/// parked by an *older* daemon build — before that exemption existed — can load
/// carrying the `[OODA-SAFEGUARD]` sentinel [`GoalProgress::Blocked`] reason,
/// leaving a continuous research goal stuck "needs human review" and requiring a
/// manual `simard goal unblock`. Applied at load and at the top of every cycle,
/// this clears exactly that stale block back to [`GoalProgress::NotStarted`] —
/// the canonical actionable, re-dispatchable state — so the goal is available
/// again next cycle with no operator intervention. A standing goal must be
/// continuous and self-sustaining.
///
/// It is deliberately narrow: it heals a goal **only** when it
/// [`is_perpetual`](ActiveGoal::is_perpetual) **and** its block reason is a
/// [`is_no_progress_marker`] sentinel. A normal goal's no-progress block (a
/// legitimate human-review request) is preserved, and any operator / scope /
/// dependency / brain-failure block on a standing goal is left untouched.
/// Idempotent — a goal already cleared is a no-op.
#[must_use]
pub fn heal_stale_no_progress_blocks(mut board: GoalBoard) -> GoalBoard {
    for goal in &mut board.active {
        let stale_no_progress_block = goal.is_perpetual()
            && matches!(&goal.status, GoalProgress::Blocked(reason) if is_no_progress_marker(reason));
        if stale_no_progress_block {
            tracing::info!(
                target: "simard::ooda",
                goal = %goal.id,
                "no-progress breaker: self-healing stale [OODA-SAFEGUARD] block on \
                 standing/perpetual goal — restoring to not-started (a standing goal \
                 must never require a manual unblock)",
            );
            goal.status = GoalProgress::NotStarted;
        }
    }
    board
}

/// Reconcile the daemon's `in_flight` board with the currently-`persisted`
/// board, honouring `tombstones`. This is the anti-clobber merge that makes
/// operator edits **stick**:
///
/// * A goal present only in `persisted` (an operator `goal add` the daemon has
///   not yet observed) is **kept**.
/// * A goal present in both is taken from `in_flight` field-for-field (the
///   daemon holds the most recent progress/status for goals it is driving).
/// * A **tombstoned** id is removed even if `in_flight` still carries it (the
///   operator removed/completed it — never resurrect it).
///
/// The active set is truncated to [`MAX_ACTIVE_GOALS`] with a deterministic key
/// (priority ascending, then id) so the result is stable. The backlog is
/// unioned by id and never truncated.
#[must_use]
pub fn reconcile(
    persisted: &GoalBoard,
    in_flight: &GoalBoard,
    tombstones: &HashSet<String>,
) -> GoalBoard {
    use std::collections::BTreeMap;

    let mut active: BTreeMap<String, ActiveGoal> = BTreeMap::new();
    for g in &persisted.active {
        active.insert(g.id.clone(), g.clone());
    }
    // in_flight wins on collision.
    for g in &in_flight.active {
        active.insert(g.id.clone(), g.clone());
    }

    let mut backlog: BTreeMap<String, crate::goal_curation::BacklogItem> = BTreeMap::new();
    for b in &persisted.backlog {
        backlog.insert(b.id.clone(), b.clone());
    }
    for b in &in_flight.backlog {
        backlog.insert(b.id.clone(), b.clone());
    }

    // Tombstones dominate both sets.
    active.retain(|id, _| !tombstones.contains(id));
    backlog.retain(|id, _| !tombstones.contains(id));
    // A goal cannot be both active and in the backlog; active wins.
    backlog.retain(|id, _| !active.contains_key(id));

    let mut active: Vec<ActiveGoal> = active.into_values().collect();
    if active.len() > MAX_ACTIVE_GOALS {
        active.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.id.cmp(&b.id)));
        active.truncate(MAX_ACTIVE_GOALS);
    }

    GoalBoard {
        active,
        backlog: backlog.into_values().collect(),
    }
}

// ---------------------------------------------------------------------------
// Daemon-facing operations
// ---------------------------------------------------------------------------

/// Load the authoritative state for the daemon, migrating from the legacy
/// cognitive-memory snapshot on first run.
///
/// If `goal_board.json` already exists it is the source of truth and returned
/// verbatim (read-your-writes). Otherwise the current cognitive-memory board
/// snapshot is read, tombstone-filtered, and written into the new authoritative
/// file so **no live goals are lost** when the daemon first adopts the store.
/// The tracker starts empty on migration (there is no prior persisted counter).
pub fn load_or_migrate(
    state_root: &Path,
    memory: &dyn CognitiveMemoryOps,
) -> SimardResult<PersistentGoalState> {
    if store_path(state_root).exists() {
        return Ok(load(state_root));
    }
    let legacy = crate::goal_curation::load_goal_board(memory).unwrap_or_default();
    let tombstones = crate::ooda_loop::load_tombstones(state_root);
    let filtered = filter_tombstoned(legacy, &tombstones);
    let migrated_active = filtered.active.len();
    mutate(state_root, |s| {
        s.board = filtered.clone();
        s.no_progress = NoProgressTracker::new();
    })?;
    tracing::info!(
        target: "simard::ooda",
        active = migrated_active,
        "goal_board_store: migrated cognitive-memory snapshot into authoritative goal_board.json"
    );
    Ok(load(state_root))
}

/// Commit the daemon's post-cycle board authoritatively.
///
/// Steps, all durable:
/// 1. Record `new_tombstones` (goals archived / completed / dropped this cycle)
///    so no path can recreate them.
/// 2. Under the store lock, re-read the current file (picking up any operator
///    edit made *during* the cycle), [`reconcile`] the daemon's `in_flight`
///    board against it honouring the full tombstone set, and persist the
///    reconciled board, the `tracker`, and the monotonic `cycle_count` (the
///    brain's lived OODA cycle number — issue #1).
///
/// Returns the reconciled board that was persisted.
pub fn commit_cycle(
    state_root: &Path,
    in_flight: &GoalBoard,
    tracker: &NoProgressTracker,
    cycle_count: u32,
    new_tombstones: &[String],
) -> SimardResult<GoalBoard> {
    if !new_tombstones.is_empty() {
        crate::ooda_loop::tombstone_goals(state_root, new_tombstones)?;
    }
    let tombstones = crate::ooda_loop::load_tombstones(state_root);
    let in_flight = in_flight.clone();
    let tracker = tracker.clone();
    mutate(state_root, move |s| {
        let reconciled = reconcile(&s.board, &in_flight, &tombstones);
        s.board = reconciled.clone();
        s.no_progress = tracker;
        // Persist the brain's lived cycle count with a monotonic guard so a
        // stale/lower value (a racing writer, or a rolled-back `OodaState`)
        // can never rewind the durable counter (issue #1).
        s.cycle_count = s.cycle_count.max(cycle_count);
        reconciled
    })
}

// ---------------------------------------------------------------------------
// Done-gate: runs every cycle, cross-repo aware
// ---------------------------------------------------------------------------

/// A single completion decision made by the every-cycle done-gate sweep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoneDecision {
    /// The goal id evaluated.
    pub goal_id: String,
    /// Whether hard completion evidence was present (the goal was marked done).
    pub completed: bool,
}

/// Evaluate **every** active goal against the completion-evidence gate and mark
/// those with hard evidence `Completed`. Unlike the legacy archive path (which
/// only re-checks goals *already* claiming completion), this runs each cycle
/// over the whole active board, so a goal whose objective was finished
/// out-of-band — e.g. a merged PR or closed issue on **another** governed repo —
/// is detected and auto-completed instead of being re-litigated forever.
///
/// The gate is cross-repo aware: [`crate::goal_curation::GhCliEvidenceSource`]
/// resolves each goal's `repo` slug, so a merged PR / closed issue on any
/// governed repo counts as evidence. Every decision is logged so operators can
/// see, in the daemon log, exactly which goals the gate acted on each cycle.
///
/// Returns the ids that were newly marked `Completed` (for the caller to
/// tombstone and archive).
pub fn sweep_done_goals(board: &mut GoalBoard, evidence: &dyn EvidenceSource) -> Vec<String> {
    let gate = CompletionEvidenceGate::new(evidence);
    let mut completed = Vec::new();
    for goal in board.active.iter_mut() {
        if matches!(goal.status, GoalProgress::Completed) {
            continue;
        }
        match gate.evaluate(goal) {
            CompletionVerdict::Complete(_) => {
                tracing::warn!(
                    target: "simard::ooda",
                    goal = %goal.id,
                    repo = goal.repo.as_deref().unwrap_or("Simard"),
                    "done-gate: cross-repo completion evidence present — marking goal DONE",
                );
                goal.status = GoalProgress::Completed;
                completed.push(goal.id.clone());
            }
            CompletionVerdict::Blocked { .. } => {
                tracing::debug!(
                    target: "simard::ooda",
                    goal = %goal.id,
                    "done-gate: no completion evidence yet — leaving active",
                );
            }
        }
    }
    completed
}
