---
title: Cognitive-memory goal store adapter
description: Reference for CognitiveMemoryGoalStore — the GoalStore-trait implementation backed by cognitive memory through the bridge helpers, used by RuntimePorts in bootstrap/assembly.
last_updated: 2026-05-09
owner: simard
doc_type: reference
related:
  - ./cognitive-memory-bridge-helpers.md
  - ./goal-board-api.md
  - ../concepts/goal-board-persistence.md
---

# Cognitive-memory goal store adapter

`CognitiveMemoryGoalStore` implements the `GoalStore` trait against the
goal-board snapshot in cognitive memory. It is the production
`Arc<dyn GoalStore>` constructed in `src/bootstrap/assembly.rs` and wired
into `RuntimePorts` for local-session execution paths — goal-curation
runs, improvement-curation runs, meeting probes, and any other consumer
that reaches `RuntimePorts.goal_store`.

## Why this exists

`FileBackedGoalStore` was the previous production implementation. It
persisted goals to `goal_records.json` under `$SIMARD_STATE_ROOT`. Issue
[#1590](https://github.com/rysweet/Simard/issues/1590) and the follow-up
PRs migrated every other consumer onto cognitive memory but left
`src/bootstrap/assembly.rs:99` still calling
`FileBackedGoalStore::try_new(config.goal_store_path())`. That created a
half-migration: the OODA daemon, the dashboard handlers, the meeting
flows, and the engineer loop all read from cognitive memory, while
`bootstrap`-assembled local sessions read from a file that nothing else
wrote to.

`CognitiveMemoryGoalStore` closes the gap. After it lands,
`goal_records.json` is no longer read or written by any production code
path; cognitive memory is the single source of truth.

## Location and shape

```rust
// src/goals/cognitive_memory_store.rs

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

Each method opens a fresh bridge for the duration of one call and lets it
drop afterwards. There is no long-lived bridge held inside the adapter
because:

- The daemon's in-process Arc shortcut (tier 0 of `launch_writer_bridge`)
  makes per-call acquisition cheap.
- Holding a `WriterBridge` across awaits would either serialize all
  callers behind a `Mutex` or risk lock contention with the daemon.

### Read methods

```rust
fn list(&self) -> SimardResult<Vec<GoalRecord>> {
    let bridge = open_reader_bridge(&self.state_root)?;
    let board = load_goal_board(bridge.ops())?;
    Ok(active_goals_as_records(&board))
}
```

Read methods use `open_reader_bridge` because they do not need the writer
lock and should not contend with the daemon. The `active_goals_as_records`
adapter (defined in `src/goal_curation/operations.rs`) projects the goal
board's `active` slot into the same `GoalRecord` shape that
`FileBackedGoalStore` previously returned, so callers see no behavioural
change.

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

Write methods use `launch_writer_bridge`. With the in-process Arc shortcut
this is a single `OnceLock::get` + `Box::new(Arc::clone(...))` when the
daemon is registered — no IPC round-trip and no lock acquisition. Outside
the daemon process the helper falls back to IPC or direct open as
documented in [Cognitive memory bridge
helpers](./cognitive-memory-bridge-helpers.md).

The launcher's strict no-silent-degradation contract means writer-method
errors (database lock contention, IPC connect failure with no daemon
available) propagate to the caller as `SimardError` rather than being
swallowed — preserving the same error-surfacing properties as the
dashboard mutation handlers.

## Wiring in `bootstrap/assembly.rs`

```rust
// Before
let goal_store = Arc::new(FileBackedGoalStore::try_new(
    config.goal_store_path(),
)?);

// After
let goal_store = Arc::new(CognitiveMemoryGoalStore::new(
    config.state_root().to_path_buf(),
));
```

`config.state_root()` is the canonical `$SIMARD_STATE_ROOT`-resolved path
that `default_state_root` already returns to the bridge helpers, so the
adapter and the rest of the runtime agree on which DB they are addressing.

`config.goal_store_path()` (which previously returned
`<state_root>/goal_records.json`) is no longer called from
`assembly.rs`. The method remains on `BootstrapConfig` for the
`FileBackedGoalStore` value type still used by `meeting_backend` test
fixtures, but production no longer calls it.

## Test consequence: improvement_curation read probe

`tests/improvement_curation.rs` previously held a single
`#[ignore]`d test:

```rust
#[ignore = "blocked on issue #1590 — RuntimePorts.goal_store still uses FileBackedGoalStore"]
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

After `CognitiveMemoryGoalStore` lands, both runs share cognitive memory
through `RuntimePorts`. The `#[ignore]` attribute is removed and the test
runs in the standard suite; `cargo test --test improvement_curation`
passes.

## What `FileBackedGoalStore` is still used for

`FileBackedGoalStore` remains in `src/goals/store.rs` for two narrow
reasons:

1. `src/meeting_backend/mod.rs:149-169` constructs one when a
   `meeting_backend` operates without a state root and needs an in-memory
   goal-store-shaped value. This is a test-helper code path; production
   meeting flows go through cognitive memory.
2. Existing tests in `src/goals/` and `src/engineer_loop/` that exercise
   the `GoalStore` trait independently of cognitive memory.

Neither path persists `goal_records.json` to disk in production. The file
itself remains as a historical artefact that older operators may have
under `$SIMARD_STATE_ROOT`; nothing reads it.

## Related reading

- [Cognitive memory bridge helpers](./cognitive-memory-bridge-helpers.md) —
  the lower-level helpers that this adapter wraps.
- [Goal board API reference](./goal-board-api.md) — `load_goal_board`,
  `save_goal_board`, and `active_goals_as_records`.
- [Goal board persistence — concept](../concepts/goal-board-persistence.md) —
  the full lifecycle this adapter participates in.
