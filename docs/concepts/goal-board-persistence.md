---
title: Goal board persistence — disk-first loading and stale assignment sweep
description: How Simard loads and saves the goal board across OODA cycles, and how stale engineer assignments are cleared automatically.
last_updated: 2026-05-08
owner: simard
doc_type: concept
related:
  - ../reference/goal-board-api.md
  - ../howto/recover-goal-board.md
  - ../architecture/overview.md
  - ../reference/subagent-tmux-tracking.md
---

# Goal board persistence — disk-first loading and stale assignment sweep

## The problem this solves

Before issue [#1574](https://github.com/rysweet/Simard/issues/1574), the OODA
cycle loaded the goal board exclusively from cognitive memory via
`search_facts("goal-board:snapshot", 1, 0.0)`. Every call to
`save_goal_board()` appended a new fact node in the graph. Over the lifetime of
a running daemon the graph accumulates many such nodes, and `LIMIT 1` is not
guaranteed to return the most-recently written one. This produced two classes of
failure:

- **Stale board**: the cycle started with an old snapshot, discarding goal
  progress, backlog additions, and engineer assignments written in the previous
  cycle.
- **Unpredictable board**: successive cycle starts could return different
  snapshots, making operator debugging very difficult.

Additionally, when an engineer's tmux session died (crash, kill, machine
restart), the goal's `assigned_to` field retained the dead session name.
The spawn logic would see the assignment and skip re-dispatching the goal,
permanently blocking progress until the operator manually cleared the field.

---

## How it works now

### Three-tier load strategy

`load_goal_board()` in `src/goal_curation/operations.rs` attempts three sources
in order, stopping at the first success:

| Tier | Source | Condition |
|------|--------|-----------|
| 1 | `$SIMARD_STATE_ROOT/goal_records.json` (disk) | File exists and parses as `GoalBoard` |
| 2 | `search_facts("goal-board:snapshot", 1, 0.0)` (cognitive memory) | Fact exists and parses as `GoalBoard` |
| 3 | `GoalBoard::new()` (empty board) | Both earlier sources absent or unreadable |

The disk file is the canonical authority. It is written by `save_goal_board()`
on every mutation — the same call that also writes to cognitive memory. Because
the disk write is a single `fs::write` to a fixed path, there is no ordering
ambiguity.

Cognitive memory remains the tier-2 fallback to support:
- First-ever daemon run before `goal_records.json` exists.
- Disaster recovery from a deleted state directory (as long as the cognitive
  memory process still holds the graph).

### Stale assignment sweep

`sweep_stale_assignments()` in `src/ooda_loop/cycle.rs` runs early in each
OODA cycle, after board load and before `seed_default_board`:

1. Calls `tmux list-sessions -F #{session_name}` to collect live session names
   into a `HashSet<String>`.
2. For each active goal whose `assigned_to` matches a session **not** in that
   set, clears `assigned_to = None` and resets `status = NotStarted`.
3. Skips the entire sweep if tmux is unavailable or returns an empty list —
   this prevents false-positive clearing when Simard is run outside a tmux
   environment (e.g., in CI or unit tests).

Once cleared, the goal re-enters the spawn path on the same cycle or the next
one, so engineer work resumes automatically without operator intervention.

---

## State root resolution

Both `load_goal_board()` and `save_goal_board()` resolve the state root
directory in the same way:

```
1. $SIMARD_STATE_ROOT env var (if set)
2. $HOME/.simard (fallback)
```

The `goal_records.json` file lives directly under the resolved root. The
directory is created automatically on first save.

---

## Cycle startup sequence

```
run_ooda_cycle_inner
├─ check .reseed_goals marker  ← if present: reset board to empty + skip load
├─ load_goal_board()           ← disk-first, 3-tier fallback (skipped if marker found)
│   └─ only applied if board.active is non-empty
├─ sweep_stale_assignments()   ← clear dead tmux sessions
├─ seed_default_board()        ← only if board still empty
├─ check_meeting_handoffs()
└─ [Observe → Orient → Decide → Act → Curate]
    └─ persist_board()         ← writes cognitive memory + disk
```

Two important notes on the load step:

1. **Reseed marker takes precedence.** If `$SIMARD_STATE_ROOT/.reseed_goals`
   exists at cycle start, the daemon resets the in-memory board to
   `GoalBoard::new()`, removes the marker file, and **skips `load_goal_board`
   entirely**. The three-tier load only runs when no marker is present.

2. **Empty boards are not applied.** The loaded board replaces the in-memory
   state only if `board.active.is_empty() == false`. An empty board — from
   tier-3 fallback or a disk file with no active goals — leaves the existing
   in-memory state untouched. An operator restoring a `goal_records.json`
   that contains no active goals will find that file ignored at load time.

---

## Guarantees and non-guarantees

**Guaranteed:**
- A daemon restarted after a clean shutdown always loads the board state from
  the previous cycle's end, not an arbitrary earlier snapshot.
- Goals assigned to dead tmux sessions are automatically unblocked within one
  OODA cycle.
- Load failures (corrupted JSON, permission errors) fall through gracefully to
  the next tier rather than crashing the daemon.

**Not guaranteed:**
- **Concurrent daemons**: if two Simard daemons share the same `SIMARD_STATE_ROOT`,
  writes from one can be overwritten by the other. Run one daemon per state root.
- **Atomic disk writes**: `save_goal_board()` uses `fs::write` which is not
  atomic. A partial write leaves a corrupted `goal_records.json`; the daemon
  falls back to cognitive memory and logs a parse error.
- **Assignment safety**: `sweep_stale_assignments()` uses session-name
  presence, not heartbeat or PID. A session that just started and has not yet
  announced itself may be cleared on the first cycle after its tmux session
  was created. This is unlikely in practice because `load_goal_board` runs
  before engineer dispatch in the same cycle.
