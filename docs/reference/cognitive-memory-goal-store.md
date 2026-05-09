---
title: Cognitive-memory goal store adapter
description: Design reference for CognitiveMemoryGoalStore — the planned GoalStore-trait implementation backed by cognitive memory through the bridge helpers, used by RuntimePorts in bootstrap/assembly.
last_updated: 2026-05-09
owner: simard
doc_type: reference
related:
  - ./cognitive-memory-bridge-helpers.md
  - ./goal-board-api.md
  - ../concepts/goal-board-persistence.md
---

# Cognitive-memory goal store adapter

> **Status: design — not yet implemented.** This document describes the
> `CognitiveMemoryGoalStore` adapter and the
> `bootstrap::assembly`-level wiring that the issue
> [#1590](https://github.com/rysweet/Simard/issues/1590) follow-up
> regression-fix work will introduce. On `main` today,
> `bootstrap::assembly` constructs an
> `Arc<FileBackedGoalStore>` for `RuntimePorts.goal_store`, and the
> ignored `improvement_curation_read_probe_…` test in
> `tests/improvement_curation.rs` documents this gap. Update this document
> to drop the "design" banner and the "planned" qualifiers when the
> adapter lands.

`CognitiveMemoryGoalStore` will implement the `GoalStore` trait against
the goal-board snapshot in cognitive memory. It is the production
`Arc<dyn GoalStore>` constructed in `bootstrap::assembly` and wired into
`RuntimePorts` for local-session execution paths — goal-curation runs,
improvement-curation runs, meeting probes, and any other consumer that
reaches `RuntimePorts.goal_store`.

## Why this exists

`FileBackedGoalStore` is the current production implementation. It
persists goals to `goal_records.json` under `$SIMARD_STATE_ROOT`. Issue
[#1590](https://github.com/rysweet/Simard/issues/1590) and the merged
follow-up PRs migrated every other consumer onto cognitive memory but
left `bootstrap::assembly` still calling
`FileBackedGoalStore::try_new(config.goal_store_path())`. That created a
half-migration: the OODA daemon, the dashboard handlers, the meeting
flows, and the engineer loop all read and write through cognitive memory,
while `bootstrap`-assembled local sessions read from a file that nothing
else writes to (and write to a file that nothing else reads from).

`CognitiveMemoryGoalStore` closes the gap. After it lands and replaces
the `FileBackedGoalStore` instantiation in `bootstrap::assembly`,
`goal_records.json` is no longer read or written by any production code
path that goes through `RuntimePorts`; cognitive memory is the single
source of truth for the goal board.

## Location and shape

```rust
// src/goals/cognitive_memory_store.rs (planned)

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

Each method opens a fresh bridge for the duration of one call and lets
it drop afterwards. There is no long-lived bridge held inside the
adapter because:

- The planned in-process Arc shortcut (tier 0 of `launch_writer_bridge`)
  makes per-call acquisition cheap inside the daemon process.
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

Read methods use `open_reader_bridge` because they do not need the
writer lock and should not contend with the daemon. The
`active_goals_as_records` adapter (defined in
`goal_curation::operations`) projects the goal board's `active` slot into
the same `GoalRecord` shape that `FileBackedGoalStore` previously
returned, so callers see no behavioural change.

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

Write methods use `launch_writer_bridge`. With the planned in-process Arc
shortcut this is a single `OnceLock::get` plus an `Arc::clone` when the
daemon is registered — no IPC round-trip and no lock acquisition.
Outside the daemon process the helper falls back to IPC or direct open
as documented in [Cognitive memory bridge
helpers](./cognitive-memory-bridge-helpers.md).

The launcher's planned strict no-silent-degradation contract means
writer-method errors (database lock contention, IPC connect failure with
no daemon available) propagate to the caller as `SimardError` rather
than being swallowed — preserving the same error-surfacing properties as
the dashboard mutation handlers.

## Wiring in `bootstrap::assembly`

```rust
// Before (current main)
let goal_store = Arc::new(FileBackedGoalStore::try_new(
    config.goal_store_path(),
)?);

// After (post-follow-up)
let goal_store = Arc::new(CognitiveMemoryGoalStore::new(
    config.state_root_path().to_path_buf(),
));
```

`config.state_root_path()` is the canonical
`$SIMARD_STATE_ROOT`-resolved path that `default_state_root` already
returns to the bridge helpers, so the adapter and the rest of the
runtime agree on which DB they are addressing.

`config.goal_store_path()` (which returns
`<state_root>/goal_records.json`) is no longer called from
`bootstrap::assembly` after the migration. The accessor remains on
`BootstrapConfig` for the `FileBackedGoalStore` value type still
constructed by the `meeting_backend` setup path.

## Test consequence: improvement_curation read probe

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
While `RuntimePorts.goal_store` is `FileBackedGoalStore`-backed and the
write run writes to cognitive memory, the read probe loads an empty list
from `goal_records.json` and the assertion fails — hence the ignore.

After `CognitiveMemoryGoalStore` lands and replaces
`FileBackedGoalStore` in `bootstrap::assembly`, both runs share
cognitive memory through `RuntimePorts`. The `#[ignore]` attribute is
removed and the test runs in the standard suite; `cargo test --test
improvement_curation` passes the probe round-trip.

## What `FileBackedGoalStore` is still used for

After the bootstrap migration, `FileBackedGoalStore` remains in
`src/goals/store.rs` for one verified production-shaped consumer plus
test fixtures:

1. `src/meeting_backend/mod.rs` constructs a `FileBackedGoalStore` when
   the meeting backend wires up its own goal store from a local file
   path. This path is independent of `RuntimePorts.goal_store` and is
   not affected by the bootstrap migration. Whether this should also
   migrate is **out of scope** for the issue-#1590 follow-up.
2. Tests in `src/goals/` and elsewhere that exercise the `GoalStore`
   trait independently of cognitive memory.

The `goal_records.json` file itself remains as a historical artefact
that older operators may have under `$SIMARD_STATE_ROOT`; after the
follow-up migration, no production code path inside
`RuntimePorts.goal_store` reads or writes it.

## Related reading

- [Cognitive memory bridge helpers](./cognitive-memory-bridge-helpers.md)
  — the lower-level helpers that this adapter wraps.
- [Goal board API reference](./goal-board-api.md) — `load_goal_board`,
  `save_goal_board`, and `active_goals_as_records`.
- [Goal board persistence — concept](../concepts/goal-board-persistence.md)
  — the full lifecycle this adapter participates in.
