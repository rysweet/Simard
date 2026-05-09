---
title: Goal board persistence — cognitive-memory single source of truth
description: How Simard loads and saves the goal board across OODA cycles, dashboard handlers, meeting flows, and the engineer loop, with cognitive memory as the sole persistence target.
last_updated: 2026-05-09
owner: simard
doc_type: concept
related:
  - ../reference/goal-board-api.md
  - ../reference/cognitive-memory-bridge-helpers.md
  - ../reference/cognitive-memory-goal-store.md
  - ../howto/recover-goal-board.md
  - ../architecture/overview.md
  - ../reference/subagent-tmux-tracking.md
---

# Goal board persistence — cognitive-memory single source of truth

This document describes the post-issue-[#1590](https://github.com/rysweet/Simard/issues/1590)
state in which **cognitive memory is the sole persistence target** for the
goal board across the OODA cycle, dashboard handlers, meeting REPL flows,
the engineer loop, and `bootstrap`-assembled local sessions. The
persistence APIs (`load_goal_board`, `save_goal_board`, `persist_board`)
and the integrity guard live in `src/goal_curation/operations.rs`. The
adapter pattern that fronts them as a `GoalStore` for `RuntimePorts` is
the [`CognitiveMemoryGoalStore`](../reference/cognitive-memory-goal-store.md).

## The problem this solves

The goal board was historically persisted to **two** places: the
cognitive memory graph (under the `goal-board:snapshot` fact) and a
`goal_records.json` file in `$SIMARD_STATE_ROOT`. Different consumers read
from different places:

- The OODA cycle wrote to both, then read disk-first on the next cycle.
- The operator dashboard read `goal_records.json` directly from several
  handlers (`goals.rs`, `workboard.rs`, `current_work.rs`, `metrics.rs`)
  and wrote to it directly from the dashboard mutation paths.
- The meeting REPL goal-curation and improvement-curation flows read
  through `FileBackedGoalStore`, which targets `goal_records.json`.
- The engineer loop read its "next five goals" through the same
  `FileBackedGoalStore`.
- `bootstrap`-assembled local sessions held a
  `FileBackedGoalStore`-backed `Arc<dyn GoalStore>` in `RuntimePorts`.

This produced a class of subtle bugs:

- **Drift** — a dashboard write that succeeded on disk but was racing a
  daemon cycle could be silently overwritten by the daemon's next save.
- **Stale reads** — a consumer that read the disk file just after another
  consumer had updated only the cognitive memory snapshot saw outdated data.
- **Recovery confusion** — an operator restoring from a backup of one store
  did not know whether the other store was now ahead, behind, or in sync.

Issue #1590 collapses both stores into one: the
`goal-board:snapshot` fact in cognitive memory becomes the **single source
of truth**. After the migration, no consumer reads or writes
`goal_records.json` in production code paths. A one-shot bootstrap
migration imports any pre-existing disk file on first startup and deletes
it after a successful re-write into cognitive memory.

---

## How it works in the target state

### Single store, two access patterns

All consumers obtain a typed bridge — see
[Cognitive memory bridge helpers](../reference/cognitive-memory-bridge-helpers.md) —
and route through two functions in
[`src/goal_curation/operations.rs`](../reference/goal-board-api.md):

| Operation | Function | Bridge type |
|-----------|----------|-------------|
| Read | `load_goal_board(bridge.ops())` | `ReaderBridge` (cheap, never contends with daemon writer lock) |
| Write | `save_goal_board(&board, bridge.ops())` | `WriterBridge` (prefers daemon IPC when available, else takes the local writer lock; fails synchronously if no writer is obtainable) |

When the OODA daemon is running, **all** writes flow through the daemon's
IPC socket. Dashboard mutation handlers, meeting REPL flows, and any other
out-of-process writer connect to the same socket via
`launch_writer_bridge`'s tier 1, so writes are serialized by the daemon.
When no daemon is running, the writer bridge takes the local LadybugDB
writer lock directly.

### Bridge ladders

`launch_writer_bridge` resolves two writer-bearing tiers in order:

1. **Daemon IPC** — connect to `~/.simard/memory.sock` if it exists.
2. **Local writer** — open the LadybugDB store directly with the writer
   lock, after running the stale-lock reaper.

If both fail, `launch_writer_bridge` returns `Err` immediately. There is
no silent read-only fallback — callers learn synchronously whether they
got a writer.

`open_reader_bridge` resolves two tiers:

1. **Daemon IPC** — same socket as above.
2. **Local read-only opener** — never contends with the writer lock, so
   safe to call concurrently with a running daemon.

See the [helper reference](../reference/cognitive-memory-bridge-helpers.md)
for the full algorithm and stale-lock reaper details.

### Legacy migration on first startup

`load_goal_board` calls `migrate_legacy_disk_file_if_present(bridge)` as
its first step. When the legacy `$SIMARD_STATE_ROOT/goal_records.json`
file exists:

1. The file is read and parsed as a `GoalBoard`.
2. The board is written into cognitive memory via `bridge.store_fact`.
3. On successful store, the file is deleted from disk.

Any failure at any step is logged to stderr and is **non-fatal** — the
file is left in place for the next startup to retry. Once the file no
longer exists, the migration step costs only a single `metadata` syscall.

This means operators upgrading a host where `goal_records.json` already
contains live state never need to manually move data: the next daemon
startup picks it up and migrates it. After a single successful migration,
no production code path reads or writes the legacy file.

### Stale assignment sweep

`sweep_stale_assignments()` in `src/ooda_loop/cycle.rs` runs early in each
OODA cycle, after board load and before `seed_default_board`:

1. Calls `tmux list-sessions -F #{session_name}` to collect live session
   names into a `HashSet<String>`.
2. For each active goal whose `assigned_to` matches a session **not** in
   that set, clears `assigned_to = None` and resets
   `status = NotStarted`.
3. **Skips the entire sweep if tmux is unavailable or returns an empty
   list.** This prevents false-positive clearing when Simard is run
   outside a tmux environment (e.g., in CI or unit tests). The trade-off
   is that in non-tmux environments, `assigned_to` values can outlive a
   real engineer death indefinitely — see "Not guaranteed" below.

Once cleared in a tmux environment, the goal re-enters the spawn path on
the same cycle or the next one, so engineer work resumes automatically
without operator intervention.

---

## Consumer matrix

| Consumer | File | Bridge helper | Read fn | Write fn | Notes |
|----------|------|---------------|---------|----------|-------|
| OODA cycle | `src/ooda_loop/cycle.rs` | (uses daemon's own bridge) | `load_goal_board` | `persist_board` | Records an episode in addition to saving the snapshot |
| Dashboard goals API | `src/operator_commands_dashboard/goals.rs` | `launch_writer_bridge` (writes), `open_reader_bridge` (reads) | `load_goal_board` | `save_goal_board` (×6 mutation handlers) | The in-process Arc shortcut keeps mutation handlers from going through Unix-socket IPC against the same process |
| Dashboard workboard | `src/operator_commands_dashboard/workboard.rs` | `open_reader_bridge` | `load_goal_board` | — | Read-only |
| Dashboard current work | `src/operator_commands_dashboard/current_work.rs` | `open_reader_bridge` | `load_goal_board` | — | Read-only |
| Dashboard metrics panel | `src/operator_commands_dashboard/metrics.rs` | `open_reader_bridge` | `load_goal_board` | — | Reports `{ source: "cognitive-memory:goal-board:snapshot", count: N }` |
| Dashboard memory panel | `src/operator_commands_dashboard/memory.rs` | (n/a) | (n/a) | (n/a) | The `goal_records.json` artefact label is removed; only on-disk artefacts are listed here |
| Meeting goal curation | `src/operator_commands_meeting/goal_curation.rs` | `open_reader_bridge` (read-curation), `launch_writer_bridge` (mutation paths) | `load_goal_board` + `active_goals_as_records` | `save_goal_board` | Replaces `FileBackedGoalStore` |
| Meeting improvement curation | `src/operator_commands_meeting/improvement_curation.rs` | `launch_writer_bridge` | `load_goal_board` + `active_goals_as_records` | `save_goal_board` | Replaces `FileBackedGoalStore` |
| Engineer loop | `src/engineer_loop/mod.rs` | `open_reader_bridge` | `load_goal_board` + `active_goals_as_records` | — | Reads top 5 active goals as `GoalRecord`s |
| Meeting bridge acquisition | `src/operator_commands_meeting/meeting_session.rs` | `launch_writer_bridge` | — | — | `launch_real_meeting_bridge` is a thin wrapper around `launch_writer_bridge(&default_state_root())` |
| Bootstrap-assembled `RuntimePorts.goal_store` | `src/bootstrap/assembly.rs` | `CognitiveMemoryGoalStore` (which uses both helpers internally) | adapter `list` / `active_top_goals` | adapter `upsert` / `remove` | Replaces `FileBackedGoalStore::try_new(config.goal_store_path())` |

`FileBackedGoalStore` is no longer instantiated in any production
goal-board code path. It remains in `src/goals/store.rs` as a value type
used by `meeting_backend` and tests.

---

## State root resolution

Both helpers resolve the state root directory in the same way (delegating to
`memory_ipc::default_state_root()`):

```
1. $SIMARD_STATE_ROOT env var (if set)
2. $HOME/.simard/state (fallback)
```

The state root contains the LadybugDB cognitive memory store. After the
one-shot legacy migration completes on first startup, **it does not
contain `goal_records.json`.**

---

## Cycle startup sequence

```
run_ooda_cycle_inner
├─ check .reseed_goals marker  ← if present: reset board to empty + skip load
├─ load_goal_board(bridge)
│   ├─ migrate_legacy_disk_file_if_present(bridge)  ← one-shot bootstrap
│   └─ search_facts("goal-board:snapshot", 1, 0.0)  ← primary read
│       (failure → log + Ok(GoalBoard::new()))
│   └─ only applied if board.active is non-empty
├─ sweep_stale_assignments()   ← clear dead tmux sessions (no-op outside tmux)
├─ seed_default_board()        ← only if board still empty
├─ check_meeting_handoffs()
└─ [Observe → Orient → Decide → Act → Curate]
    └─ persist_board(bridge)   ← writes goal-board:snapshot fact + episode
        (rejects suspect boards via integrity guard before any write)
```

Three important notes on the load step:

1. **Reseed marker takes precedence.** If `$SIMARD_STATE_ROOT/.reseed_goals`
   exists at cycle start, the daemon resets the in-memory board to
   `GoalBoard::new()`, removes the marker file, and **skips `load_goal_board`
   entirely**.

2. **Empty boards are not applied.** The loaded board replaces the in-memory
   state only if `board.active.is_empty() == false`. An empty board leaves
   the existing in-memory state untouched.

3. **`load_goal_board` never raises an error.** Snapshot parse failures and
   bridge IPC errors are logged and degrade to `Ok(GoalBoard::new())`. The
   cycle that performs the read continues to run; the next `persist_board`
   writes a fresh snapshot.

---

## Guarantees and non-guarantees

**Guaranteed:**
- A daemon restarted after a clean shutdown always loads the board state
  from the previous cycle's end, because the same fact key
  (`goal-board:snapshot`) is written on every save and the cognitive memory
  graph orders fact revisions by recency.
- Writes from the dashboard, the meeting REPL, and the daemon never race
  against each other when the daemon is running, because they all flow
  through the same daemon IPC socket.
- `save_goal_board` rejects boards containing placeholder descriptions or
  suspiciously short IDs **before** any write, preventing LLM
  hallucinations during the Decide phase from silently overwriting the
  authoritative snapshot.
- `load_goal_board` never panics or raises an error from a corrupt
  snapshot — it logs and degrades to an empty board, leaving the in-memory
  state untouched (because the OODA cycle only applies non-empty loaded
  boards).

**Not guaranteed:**
- **Concurrent daemons**: if two Simard daemons share the same
  `SIMARD_STATE_ROOT`, the second one will fail to take the writer lock
  and exit. This is enforced by LadybugDB.
- **Bridge writes when no writer can be acquired**: `launch_writer_bridge`
  returns `Err` synchronously rather than returning a degraded bridge.
  Callers must handle the error — they cannot silently fall through to a
  read-only path. This is rare and indicates a stale lock the reaper
  could not free — see the
  [recovery how-to](../howto/recover-goal-board.md).
- **Assignment safety in tmux**: `sweep_stale_assignments()` uses
  session-name presence, not heartbeat or PID. A session that just started
  and has not yet announced itself may be cleared on the first cycle after
  its tmux session was created. This is unlikely in practice because
  `load_goal_board` runs before engineer dispatch in the same cycle.
- **Assignment safety outside tmux**: when tmux is not present (CI,
  unit tests, headless servers), the sweep is a no-op. `assigned_to`
  values can outlive a real engineer death indefinitely. Operators
  running Simard outside tmux should clear stale assignments via the
  `simard goals clear-assignment` CLI (see the recovery how-to) — or run
  Simard inside tmux.
