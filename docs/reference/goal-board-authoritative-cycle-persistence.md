---
title: "Goal-board authoritative cycle persistence (#4287)"
description: >
  How the OODA cycle's end-of-cycle board write is routed through the
  authoritative, lock-serialized goal_board_store::commit_cycle instead of the
  non-authoritative goal_curation persist path, so direct callers of
  run_ooda_cycle can no longer diverge from the durable goal board.
last_updated: 2026-07-18
review_schedule: as-needed
owner: simard
doc_type: reference
status: planned
related:
  - ../concepts/authoritative-goal-board-store.md
  - ../concepts/goal-board-persistence.md
  - ./goal-board-api.md
  - ./goal-board-corruption-guard-api.md
  - ./durable-ooda-cycle-counter.md
  - ../../src/ooda_loop/cycle.rs
  - ../../src/goal_board_store/mod.rs
---

# Goal-board authoritative cycle persistence (#4287)

> **Status: planned (spec).** This document specifies routing the end-of-cycle
> persist block in
> [`src/ooda_loop/cycle.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs)
> through
> [`goal_board_store::commit_cycle`](https://github.com/rysweet/Simard/blob/main/src/goal_board_store/mod.rs),
> the single lock-serialized authority for the durable goal board. As of this
> writing `cycle.rs:865` still calls the non-authoritative
> `save_goal_board_with_removals`; `commit_cycle` exists in `goal_board_store` but
> is **not yet** called from `cycle.rs`. Closes
> [#4287](https://github.com/rysweet/Simard/issues/4287).

## The bug

`run_ooda_cycle` persisted the post-cycle board through the `goal_curation` path
(`persist_board` / `save_goal_board_with_removals`). That path is **not** the
authoritative store: it does not take the goal-board `StoreLock` and does not
run the tombstone reconciliation that the authoritative
[`goal_board_store`](../concepts/authoritative-goal-board-store.md) uses. Direct
callers of `run_ooda_cycle` (not just the daemon's primary path) therefore wrote
board state that could **diverge** from the authoritative
`<state_root>/state/goal_board.json` — an operator edit made *during* the cycle
could be clobbered, and archived/dropped goals could be resurrected from a stale
snapshot.

## The fix

The end-of-cycle persist block **will route** every caller — direct or daemon —
through the authoritative commit, replacing the `save_goal_board_with_removals`
call at `cycle.rs:865`:

```rust
/// Commit the daemon's post-cycle board authoritatively.
/// 1. Record `new_tombstones` (archived / completed / dropped this cycle).
/// 2. Under the StoreLock, re-read the current file (picking up any operator
///    edit made during the cycle), reconcile the in-flight board against it
///    honouring the full tombstone set, and persist the reconciled board, the
///    NoProgressTracker, and the monotonic cycle_count.
pub fn commit_cycle(
    state_root: &Path,
    in_flight: &GoalBoard,
    tracker: &NoProgressTracker,
    cycle_count: u32,
    new_tombstones: &[String],
) -> SimardResult<GoalBoard>;
```

`commit_cycle` **subsumes** the former `save_goal_board_with_removals` behavior:
the archived/dropped ids become the `new_tombstones` set, and the store's
`reconcile` step guarantees they cannot be resurrected by a merge-on-write from a
persisted snapshot (the original intent of issue #2264). Because the write holds
the `StoreLock` and re-reads the file inside the lock, a concurrent operator edit
is merged rather than clobbered.

### What is preserved

The change is scoped to the `cycle.rs` persist block only. It preserves:

- **The corruption guard.** If goals vanish from the board without archival, the
  cycle still *skips* persistence to protect the last-known-good on-disk state —
  the guard runs before `commit_cycle` is reached.
- **Episode classification.** The durable goal-archival event (issue #2327) is
  still classified and recorded with its `{importance, event_kind, goal_id,
  cycle, is_operational}` metadata, merged with board-count fields so neither
  signal is lost.
- **The monotonic cycle counter.** `commit_cycle` persists the brain's lived
  cycle number with a `max()` guard so a racing/rolled-back writer cannot rewind
  it (issue #1).

## Behavior

| Caller | Before | After |
|--------|--------|-------|
| Daemon primary path | authoritative store | authoritative store (unchanged) |
| Direct `run_ooda_cycle` caller | non-authoritative `goal_curation` write | authoritative `commit_cycle` (StoreLock + reconcile) |

All callers will observe a single durable source of truth. There is no unlocked
write path once the routing lands.

## Tests

A regression test in
[`src/goal_board_store/tests.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_board_store/tests.rs)
runs a cycle to completion through a direct caller and asserts the **on-disk**
store reflects the completed cycle — archived/dropped goals are tombstoned and do
not reappear, and a concurrent edit is reconciled rather than clobbered.

## See also

- [Authoritative goal-board store](../concepts/authoritative-goal-board-store.md)
- [Goal-board API](./goal-board-api.md)
- [Goal-board corruption-guard API](./goal-board-corruption-guard-api.md)
- [Durable OODA cycle counter](./durable-ooda-cycle-counter.md)
