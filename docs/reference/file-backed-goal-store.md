---
title: File-backed goal store with flock
description: Reference for FileBackedGoalStore — the production GoalStore implementation that persists GoalRecords to ~/.simard/state/goal_store.json with advisory flock locking for cross-process safety.
last_updated: 2026-06-03
owner: simard
doc_type: reference
related:
  - ../concepts/file-backed-goal-store-simplification.md
  - ./state-root-resolution.md
  - ./goal-board-api.md
  - ../howto/troubleshoot-goal-store.md
  - ./meeting-close-lifecycle.md
---

# File-backed goal store with flock

`FileBackedGoalStore` is the production `GoalStore` implementation wired
into `RuntimePorts.goal_store` via `bootstrap::assembly`. It persists
`Vec<GoalRecord>` to a single JSON file at the canonical path
`goal_store_path()` (default: `~/.simard/state/goal_store.json`) with
advisory `flock(2)` locking for cross-process consistency.

This replaces the `CognitiveMemoryGoalStore` design documented in
[cognitive-memory-goal-store.md](./cognitive-memory-goal-store.md)
(now superseded). The rationale for the change is in
[File-backed goal store simplification](../concepts/file-backed-goal-store-simplification.md).

---

## Canonical path

The goal store file path is resolved by a single centralized helper:

```rust
pub fn goal_store_path() -> PathBuf {
    crate::state_root::simard_state_root()
        .join("state")
        .join("goal_store.json")
}
```

This helper is re-exported from `crate::state_root` and used by
consumers that resolve the default goal store path:

- `bootstrap::assembly` — constructs `FileBackedGoalStore::try_new(goal_store_path())`
- `meeting_backend::closing` — constructs an ad-hoc `FileBackedGoalStore`
  for direct goal writes on meeting close
- `meeting_backend::mod` — `load_active_goal_titles()` reads from
  `goal_store_path()` (no longer hardcoded)
- `improvement_curation::run_improvement_curation_read_probe` — reads
  persisted goals via `FileBackedGoalStore::try_new(state_root.join("state").join("goal_store.json"))`
  using the operator-supplied `state_root` override (same relative path
  as `goal_store_path()` but rooted at the explicit state root, not the
  default `simard_state_root()`)

The resolved path follows the standard state-root resolution ladder
documented in [State-root resolution](./state-root-resolution.md):

```
1. $SIMARD_STATE_ROOT/state/goal_store.json  (if SIMARD_STATE_ROOT is set)
2. ~/.simard/state/goal_store.json            (default)
```

> **Automatic migration.** The previous path was
> `<state_root>/goal_records.json`. When `FileBackedGoalStore::try_new()`
> opens a path that does not yet exist, it checks for the legacy file at
> `<state_root>/goal_records.json` (computed via
> `path.parent().parent().join("goal_records.json")`). If the legacy
> file exists, it is **copied** (not renamed) to the new location and
> the parent `state/` directory is created if absent. This preserves
> existing goals on deploy without operator intervention. The copy is
> idempotent — if the new file already exists, the migration is skipped.
> A copy failure returns `SimardError::PersistentStoreIo` so the
> operator sees the error immediately rather than silently starting with
> an empty store.
>
> The legacy file is intentionally left in place after copying so that
> other code that may still read the old path is not broken.

---

## Locking protocol

`FileBackedGoalStore` uses advisory `flock(2)` on a dedicated lockfile
at `goal_store.json.lock` (sibling of the data file). This avoids the
invalidation problem that would occur if flock were placed on the data
file itself while using atomic temp-file + rename for persistence.

### Write path (`put`, `remove`)

```
1. Open/create goal_store.json.lock
2. flock(LOCK_EX)                       ← exclusive lock
3. Read goal_store.json from disk       ← reload for cross-process freshness
4. Deserialize into local Vec<GoalRecord>
5. Upsert/remove in local vec
6. Serialize local vec → temp file
7. fsync(temp file)
8. rename(temp file → goal_store.json)  ← atomic replace
9. Replace self.records with local vec  ← only after persist succeeds
10. flock(LOCK_UN) / drop lock fd
```

The in-memory cache (`self.records: Mutex<Vec<GoalRecord>>`) is updated
**only after** the persist succeeds (step 9). A failed persist leaves
the cache reflecting the last successful disk state, preventing
in-process consumers from seeing unpersisted data.

### Read path (`list`, `active_top_goals`)

```
1. Open/create goal_store.json.lock
2. flock(LOCK_SH)                       ← shared lock
3. Read goal_store.json from disk       ← reload for cross-process freshness
4. Deserialize into local Vec<GoalRecord>
5. Replace self.records with loaded vec
6. flock(LOCK_UN) / drop lock fd
7. Return clone of self.records
```

Shared locks allow concurrent readers; an exclusive writer blocks all
readers and other writers.

### Cross-process safety

The lockfile protocol ensures that:

- Two OODA cycles running concurrently (e.g., a daemon and a manual
  `simard ooda run --cycles=1`) serialize their writes.
- A meeting close writing new goal records does not race with the
  daemon's curate phase persisting the board.
- Multiple readers (dashboard, engineer loop) never see a half-written
  JSON file.

The lock is advisory — processes that bypass `FileBackedGoalStore` and
write to the JSON file directly will not be protected.

### Platform notes

On Linux, `flock(2)` is used via `libc::flock()`. On non-Linux
platforms, the Rust `fs2` crate provides equivalent advisory locking.
The lockfile is never deleted; it persists as a zero-byte sentinel.

---

## GoalStore trait implementation

```rust
impl GoalStore for FileBackedGoalStore {
    fn list(&self) -> SimardResult<Vec<GoalRecord>>;
    fn put(&self, record: GoalRecord) -> SimardResult<()>;
    fn remove(&self, id: &str) -> SimardResult<()>;
    fn active_top_goals(&self, n: usize) -> SimardResult<Vec<GoalRecord>>;
}
```

| Method | Lock | Reloads from disk | Writes to disk |
|--------|------|-------------------|----------------|
| `list()` | Shared | Yes | No |
| `put(record)` | Exclusive | Yes | Yes |
| `remove(id)` | Exclusive | Yes | Yes |
| `active_top_goals(n)` | Shared | Yes | No |

`active_top_goals(n)` filters by `status == Active`, sorts by priority
(ascending), and returns the first `n` records.

---

## Wiring in `bootstrap::assembly`

```rust
// src/bootstrap/assembly.rs

let goal_store: Arc<dyn GoalStore> = Arc::new(
    FileBackedGoalStore::try_new(
        crate::state_root::goal_store_path(),
    )?,
);
```

`FileBackedGoalStore::try_new(path)` creates the parent directory if
absent (mode `0o700` on unix), but does **not** create the JSON file
itself — an empty file is treated as an empty store (`Vec::new()`), and
a missing file is also treated as empty. The first `put()` call creates
the file.

Before opening, `try_new()` runs the one-time legacy migration
described in [Canonical path § Automatic migration](#canonical-path).
If the new file does not exist but
`<state_root>/goal_records.json` does, the legacy file is copied to
the new path. This ensures a seamless upgrade on first boot after the
path change.

---

## Error handling

| Scenario | Behavior |
|----------|----------|
| File does not exist on read | Returns empty `Vec<GoalRecord>` |
| File contains invalid JSON | Returns `SimardError::GoalStoreParseFailed` |
| Lockfile cannot be created | Returns `SimardError::GoalStoreLockFailed` |
| Write fails (ENOSPC, EACCES) | Returns `SimardError::GoalStorePersistFailed`; in-memory cache is **not** updated |
| Rename fails after temp write | Returns error; temp file is cleaned up |
| Legacy migration copy fails | Returns `SimardError::PersistentStoreIo` with `action: "migrate-legacy-goals"` — the store does not open |

No error is silently swallowed. All errors propagate to callers.

---

## Relationship to GoalBoard and cognitive memory

`FileBackedGoalStore` stores `Vec<GoalRecord>` — flat records with
`id`, `title`, `status`, `priority`, and metadata fields. This is
**not** the `GoalBoard` type (which has `active: Vec<ActiveGoal>` and
`backlog: Vec<BacklogItem>`).

The OODA cycle's in-memory `GoalBoard` remains the authoritative
structure for cycle-level operations. `GoalBoard` is persisted to
cognitive memory via `save_goal_board()` as before. The file-backed
`GoalStore` is a complementary query/index layer used by:

- Meeting close (direct goal record writes)
- Meeting backend (`load_active_goal_titles()`)
- Bootstrap-assembled `RuntimePorts` consumers
- Engineer loop (reads top-5 goals)
- Improvement-curation read probe (reads persisted goals for the
  audit-only `improvement-curation read` command)

The two stores may diverge briefly (a goal record written by meeting
close has not yet been ingested into the `GoalBoard` by the curate
phase). This is expected and documented in
[Meeting close lifecycle](./meeting-close-lifecycle.md).

---

## See also

- [File-backed goal store simplification (concept)](../concepts/file-backed-goal-store-simplification.md)
  — why this replaced `CognitiveMemoryGoalStore`.
- [State-root resolution](./state-root-resolution.md) — path resolution.
- [Goal board API reference](./goal-board-api.md) — the cognitive-memory
  `GoalBoard` persistence used by the OODA cycle.
- [How to troubleshoot the goal store](../howto/troubleshoot-goal-store.md)
  — operator playbook for common issues.
- [Cognitive-memory goal store adapter](./cognitive-memory-goal-store.md)
  — the superseded design.
