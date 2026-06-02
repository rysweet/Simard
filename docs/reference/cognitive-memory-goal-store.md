---
title: Cognitive-memory goal store adapter (superseded)
description: Design reference for the CognitiveMemoryGoalStore adapter — superseded by the file-backed GoalStore with flock (issue #2182). Retained for historical context.
last_updated: 2026-06-02
owner: simard
doc_type: reference
related:
  - ./file-backed-goal-store.md
  - ./cognitive-memory-bridge-helpers.md
  - ./goal-board-api.md
  - ../concepts/goal-board-persistence.md
  - ../concepts/file-backed-goal-store-simplification.md
---

# Cognitive-memory goal store adapter (superseded)

> **Status: superseded by issue
> [#2182](https://github.com/rysweet/Simard/issues/2182).** The
> `CognitiveMemoryGoalStore` design described below was replaced by
> `FileBackedGoalStore` with advisory flock locking. See
> [File-backed goal store reference](./file-backed-goal-store.md) for
> the current production implementation and
> [File-backed goal store simplification](../concepts/file-backed-goal-store-simplification.md)
> for the rationale.
>
> This document is retained for historical context. The code in
> `src/goals/cognitive_memory_store.rs` remains in the codebase for
> test use but is no longer wired into `bootstrap::assembly`.

`CognitiveMemoryGoalStore` was designed to implement the `GoalStore`
trait against the goal-board snapshot in cognitive memory. It would have
been the production `Arc<dyn GoalStore>` constructed in
`bootstrap::assembly` and wired into `RuntimePorts` for local-session
execution paths — goal-curation runs, improvement-curation runs, meeting
probes, and any other consumer that reaches `RuntimePorts.goal_store`.

---

> **Everything below this line is historical context.** The design was
> never shipped; issue #2182 chose the simpler file-backed approach
> instead. Read [File-backed goal store](./file-backed-goal-store.md)
> for the current production implementation.

---

## Why this was proposed

Before issue #2182, `FileBackedGoalStore` persisted goals to
`goal_records.json` under `$SIMARD_STATE_ROOT`. Issue
[#1590](https://github.com/rysweet/Simard/issues/1590) and the merged
follow-up PRs migrated every other consumer onto cognitive memory but
left `bootstrap::assembly` still calling
`FileBackedGoalStore::try_new(config.goal_store_path())`. That created a
half-migration: the OODA daemon, the dashboard handlers, the meeting
flows, and the engineer loop all read and wrote through cognitive memory,
while `bootstrap`-assembled local sessions read from a file that nothing
else wrote to (and wrote to a file that nothing else read from).

`CognitiveMemoryGoalStore` was intended to close that gap. Had it
landed, `goal_records.json` would no longer have been read or written by
any production code path through `RuntimePorts`, and cognitive memory
would have become the single source of truth for the goal board.
Instead, issue #2182 retained and improved the file-backed approach with
flock locking — see [File-backed goal store
simplification](../concepts/file-backed-goal-store-simplification.md)
for the rationale.

## Location and shape

```rust
// src/goals/cognitive_memory_store.rs (design — never shipped)

pub struct CognitiveMemoryGoalStore {
    state_root: PathBuf,
}

impl CognitiveMemoryGoalStore {
    pub fn new(state_root: PathBuf) -> Self { Self { state_root } }
}

impl GoalStore for CognitiveMemoryGoalStore {
    fn list(&self) -> SimardResult<Vec<GoalRecord>> { /* read */ }
    fn upsert(&self, record: GoalRecord) -> SimardResult<()> { /* write */ }
    fn remove(&self, id: &str) -> SimardResult<()> { /* write */ }
    fn active_top_goals(&self, n: usize) -> SimardResult<Vec<GoalRecord>> { /* read */ }
    // … remaining trait methods …
}
```

Each method would have opened a fresh bridge for the duration of one
call and let it drop afterwards. There would have been no long-lived
bridge held inside the adapter because:

- The planned in-process Arc shortcut (tier 0 of `launch_writer_bridge`)
  would have made per-call acquisition cheap inside the daemon process.
- Holding a `WriterBridge` across awaits would have either serialized all
  callers behind a `Mutex` or risked lock contention with the daemon.

### Read methods

```rust
fn list(&self) -> SimardResult<Vec<GoalRecord>> {
    let bridge = open_reader_bridge(&self.state_root)?;
    let board = load_goal_board(bridge.ops())?;
    Ok(active_goals_as_records(&board))
}
```

Read methods would have used `open_reader_bridge` because they do not
need the writer lock and should not contend with the daemon. The
`active_goals_as_records` adapter (defined in
`goal_curation::operations`) would have projected the goal board's
`active` slot into the same `GoalRecord` shape that
`FileBackedGoalStore` returned, so callers would have seen no
behavioural change.

### Write methods

```rust
fn upsert(&self, record: GoalRecord) -> SimardResult<()> {
    let bridge = launch_writer_bridge(&self.state_root)?;
    let mut board = load_goal_board(bridge.ops())?;
    apply_upsert(&mut board, record);
    save_goal_board(&board, bridge.ops())?;
    Ok(())
}
```

Write methods would have used `launch_writer_bridge`. With the planned
in-process Arc shortcut this would have been a single `OnceLock::get`
plus an `Arc::clone` when the daemon was registered — no IPC round-trip
and no lock acquisition. Outside the daemon process the helper would
have fallen back to IPC or direct open as documented in [Cognitive
memory bridge helpers](./cognitive-memory-bridge-helpers.md).

The launcher's planned strict no-silent-degradation contract would have
meant writer-method errors (database lock contention, IPC connect
failure with no daemon available) propagated to the caller as
`SimardError` rather than being swallowed — preserving the same
error-surfacing properties as the dashboard mutation handlers.

## Planned wiring in `bootstrap::assembly` (not shipped)

Had the migration proceeded, the wiring change would have been:

```rust
// Before (as of issue #1590)
let goal_store = Arc::new(FileBackedGoalStore::try_new(
    config.goal_store_path(),
)?);

// After (proposed but superseded by #2182)
let goal_store = Arc::new(CognitiveMemoryGoalStore::new(
    config.state_root_path().to_path_buf(),
));
```

`config.state_root_path()` was the canonical
`$SIMARD_STATE_ROOT`-resolved path that `default_state_root` already
returned to the bridge helpers, so the adapter and the rest of the
runtime would have agreed on which DB they addressed.

Issue #2182 instead kept `FileBackedGoalStore` in `bootstrap::assembly`
with the new flock-protected path (`goal_store_path()`). See
[File-backed goal store](./file-backed-goal-store.md) for current wiring.

## Test consequence: improvement_curation read probe (historical)

`tests/improvement_curation.rs` currently holds an ignored test:

```rust
#[ignore = "Probe round-trip needs bootstrap/assembly.rs migration to write \
            goals through cognitive memory (follow-up to #1590)"]
#[test]
fn improvement_curation_read_probe_surfaces_persisted_review_decisions_without_mutating_state() {
    /* … */
}
```

The test asserts that an improvement-curation read probe sees the same
review decisions that an earlier improvement-curation **write** run
persisted, given that both runs operate against the same `state_root`.
While `RuntimePorts.goal_store` was `FileBackedGoalStore`-backed and the
write run wrote to cognitive memory, the read probe loaded an empty list
from `goal_records.json` and the assertion failed — hence the ignore.

Had `CognitiveMemoryGoalStore` landed and replaced
`FileBackedGoalStore` in `bootstrap::assembly`, both runs would have
shared cognitive memory through `RuntimePorts`. The `#[ignore]` attribute
would have been removed and the test would have run in the standard
suite. Issue #2182 instead resolves this by ensuring the file-backed
store is the authoritative write path for both runs.

## What `FileBackedGoalStore` is used for

`FileBackedGoalStore` is the **current production implementation** after
issue #2182. It is wired into `bootstrap::assembly` with flock locking
and lives at `$SIMARD_STATE_ROOT/state/goal_store.json`. Consumers
include:

1. `bootstrap::assembly` — constructs the `Arc<dyn GoalStore>` for
   `RuntimePorts.goal_store`.
2. `meeting_backend::closing` — constructs an ad-hoc instance for
   direct goal writes on meeting close.
3. `meeting_backend::mod` — `load_active_goal_titles()` reads via
   `goal_store_path()`.
4. Tests in `src/goals/` and elsewhere that exercise the `GoalStore`
   trait.

See [File-backed goal store reference](./file-backed-goal-store.md) for
the full API and locking protocol.

## Related reading

- [Cognitive memory bridge helpers](./cognitive-memory-bridge-helpers.md)
  — the lower-level helpers that this adapter wraps.
- [Goal board API reference](./goal-board-api.md) — `load_goal_board`,
  `save_goal_board`, and `active_goals_as_records`.
- [Goal board persistence — concept](../concepts/goal-board-persistence.md)
  — the full lifecycle this adapter participates in.
