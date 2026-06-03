---
title: File-backed goal store simplification
description: Why Simard replaced the CognitiveMemoryGoalStore IPC adapter with a plain JSON file and flock locking for the GoalStore trait.
last_updated: 2026-06-02
owner: simard
doc_type: concept
related:
  - ../reference/file-backed-goal-store.md
  - ../reference/cognitive-memory-goal-store.md
  - goal-board-persistence.md
  - ../reference/state-root-resolution.md
---

# File-backed goal store simplification

Issue [#2182](https://github.com/rysweet/Simard/issues/2182) replaces
`CognitiveMemoryGoalStore` with `FileBackedGoalStore` (flock-backed) as
the production `GoalStore` implementation in `bootstrap::assembly`.

## The problem

`CognitiveMemoryGoalStore` was designed to close the gap between
`FileBackedGoalStore` (which persisted to `goal_records.json`) and the
cognitive-memory graph (the `goal-board:snapshot` fact). The adapter would
route every `GoalStore` trait call through the cognitive-memory bridge
helpers, opening per-call reader or writer bridges.

In practice, this introduced three problems:

1. **IPC complexity for a simple data need.** The `GoalStore` trait
   serves consumers that need a flat list of goal records — meeting
   backend, engineer loop, bootstrap-assembled sessions. These consumers
   do not need the full cognitive-memory graph, the bridge resolution
   ladder, or the daemon IPC socket. Routing a `list()` call through
   `open_reader_bridge → search_facts → filter → parse` is
   disproportionate to reading a JSON file.

2. **Failure modes inherited from the bridge stack.** When the daemon is
   not running and the local writer lock is contended, `GoalStore` write
   calls fail with opaque bridge errors that callers cannot distinguish
   from genuine goal-store problems. The bridge's stale-lock reaper,
   read-only fallback removal, and socket-connect timeout all become
   failure surfaces for simple goal queries.

3. **Two sources of truth remained.** The `GoalBoard` (cognitive memory)
   and `Vec<GoalRecord>` (file) stored overlapping but not identical
   data. `CognitiveMemoryGoalStore` eliminated the file but did not
   eliminate the schema mismatch — the adapter had to project
   `GoalBoard.active` into `Vec<GoalRecord>` on every read, and the
   two shapes could diverge in fields, ordering, and semantics.

## The solution

Replace the IPC adapter with the existing `FileBackedGoalStore`,
hardened with:

- **Advisory flock on a dedicated lockfile** for cross-process
  consistency. Shared locks on reads, exclusive locks on writes.
- **Reload-from-disk on every access** so a second process's writes
  are immediately visible (no stale in-memory cache).
- **Atomic persist** via temp-file + fsync + rename — the same
  hardening pattern used for meeting handoff writes.
- **Centralized path helper** (`state_root::goal_store_path()`) so
  every consumer resolves to the same file regardless of how
  `SIMARD_STATE_ROOT` or `BootstrapConfig` is configured.

The OODA cycle's `GoalBoard` in cognitive memory remains the
authoritative structure for cycle-level operations. The file-backed
`GoalStore` is a complementary query/index layer. The two stores may
diverge briefly — a goal record written by meeting close has not yet
been ingested into the `GoalBoard` by the curate phase — and this
divergence is expected and bounded (resolved on the next OODA cycle).

## What this does not change

- **Cognitive memory is still the OODA cycle's persistence layer.** The
  `goal-board:snapshot` fact, `load_goal_board`, `save_goal_board`,
  `persist_board`, and the corruption guards are unchanged.
- **The `GoalBoard` type** (`active: Vec<ActiveGoal>`,
  `backlog: Vec<BacklogItem>`) is unchanged.
- **Dashboard, OODA curate, and engineer-loop code** that reads through
  the cognitive-memory bridge is unchanged.

## What this removes

- `CognitiveMemoryGoalStore` in `src/goals/cognitive_memory_store.rs` is
  no longer the production implementation. It remains in the codebase for
  test use but is not wired into `bootstrap::assembly`.
- The IPC socket is no longer needed for goal store operations. It
  remains in use for other cognitive-memory consumers (dashboard, OODA
  cycle, meeting REPL).
- `BootstrapConfig::goal_store_path()` now returns
  `<state_root>/state/goal_store.json` instead of
  `<state_root>/goal_records.json`.

## Trade-offs

| Gain | Cost |
|------|------|
| Simpler failure modes — file I/O errors only | Two stores (file + cognitive memory) can diverge briefly |
| No IPC dependency for goal queries | File-backed store does not benefit from daemon IPC serialization |
| Every consumer resolves one path | Operators must not manually edit the JSON file (use CLI or GoalStore API) |
| flock provides cross-process safety | Advisory locks are not enforced on processes that bypass FileBackedGoalStore |

## See also

- [File-backed goal store reference](../reference/file-backed-goal-store.md)
  — the API, locking protocol, and wiring details.
- [Goal board persistence](./goal-board-persistence.md) — the
  cognitive-memory persistence layer that continues unchanged.
- [Cognitive-memory goal store adapter](../reference/cognitive-memory-goal-store.md)
  — the superseded design.
