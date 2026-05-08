---
title: Goal board API reference
description: Rust API reference for goal board persistence, mutation, and adapter functions in src/goal_curation/operations.rs.
last_updated: 2026-05-08
owner: simard
doc_type: reference
status: design — partially implemented
related:
  - ../concepts/goal-board-persistence.md
  - ../howto/recover-goal-board.md
  - ../howto/inspect-durable-goal-register.md
  - ./cognitive-memory-bridge-helpers.md
---

# Goal board API reference

> **Status: design specification — partially implemented (issue [#1590](https://github.com/rysweet/Simard/issues/1590)).**
>
> The persistence functions (`load_goal_board`, `save_goal_board`,
> `persist_board`) and mutation helpers (`add_active_goal`,
> `enqueue_stewardship_issue`, `promote_to_active`, `update_goal_progress`,
> `clear_goal_assignment`, `archive_completed`) **exist today** in
> [`src/goal_curation/operations.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/operations.rs)
> and behave as documented below.
>
> The `active_goals_as_records` adapter is **not yet implemented** — it is part
> of the issue #1590 migration work and is documented here as the target
> design that the engineer-loop and meeting-curation consumers will use to
> obtain `Vec<GoalRecord>` from a `GoalBoard`.
>
> The bridge-acquisition helpers used in the examples
> (`launch_writer_bridge`, `open_reader_bridge`) are also part of the
> migration spec — see
> [Cognitive memory bridge helpers](./cognitive-memory-bridge-helpers.md).

This document covers the public functions in
`src/goal_curation/operations.rs` that load, save, mutate, and adapt the goal
board (`GoalBoard`) used by the OODA cycle, the dashboard, the meeting REPL,
and the engineer loop.

> **Single source of truth (target state).** Issue #1590 collapses the goal
> board onto the `goal-board:snapshot` fact in cognitive memory. After the
> migration, no consumer reads or writes `goal_records.json` directly. A
> one-time bootstrap migration in `load_goal_board` continues to import any
> legacy disk file and delete it after successful re-write into cognitive
> memory — see "Legacy migration" below.

---

## Persistence functions

### `load_goal_board`

```rust
pub fn load_goal_board(bridge: &dyn CognitiveMemoryOps) -> SimardResult<GoalBoard>
```

Loads the goal board from cognitive memory. The function is intentionally
**resilient** — every recoverable failure is logged to stderr and degrades to
an empty `GoalBoard` rather than propagating an `Err`. The cycle that
performs the read continues to run and the next `save_goal_board` writes a
fresh snapshot.

**Resolution order (in this order, each step optional):**

1. **Legacy bootstrap.** Calls `migrate_legacy_disk_file_if_present(bridge)`.
   If `$SIMARD_STATE_ROOT/goal_records.json` exists, the file is read,
   converted to a `GoalBoard`, written to cognitive memory via `store_fact`,
   and the file is deleted. Failures here are logged and non-fatal — the
   file is left in place for the next startup to retry. Once migrated, this
   step costs only a single `metadata` syscall.
2. **Read snapshot.** Calls `bridge.search_facts("goal-board:snapshot", 1, 0.0)`
   and parses the most-recent matching fact's payload as `GoalBoard`.

**Parameters**

| Name | Type | Description |
|------|------|-------------|
| `bridge` | `&dyn CognitiveMemoryOps` | Cognitive memory adapter — typically obtained via `open_reader_bridge` for read-only consumers (see [bridge helpers](./cognitive-memory-bridge-helpers.md)) or via the daemon's own in-process bridge for the OODA cycle |

**Return contract**

| Outcome | Behaviour |
|---------|-----------|
| Snapshot fact found and parses | `Ok(GoalBoard)` with the deserialized board |
| `search_facts` returns 0 results | `Ok(GoalBoard::new())` — empty board |
| `search_facts` returns a fact whose payload fails to parse | `Ok(GoalBoard::new())` + stderr warning `cognitive memory snapshot parse error (…) — returning empty board` |
| `bridge.search_facts` itself fails (IPC error, lock contention, …) | `Ok(GoalBoard::new())` + stderr warning `cognitive memory search_facts failed (…) — returning empty board` |
| Legacy migration encounters a corrupt or unreadable file | Logged, non-fatal; the function continues to step 2 |

The function returns `Err` **only** for unrecoverable internal panics
(none currently exist). The corruption-guard recovery path described in
[`docs/concepts/goal-board-corruption-guards.md`](../concepts/goal-board-corruption-guards.md)
is layered on top of this resilient read by the OODA cycle and the
integrity guard in `save_goal_board` — see below.

**Note on empty boards:** the OODA cycle only applies the loaded board to
in-memory state if `board.active.is_empty() == false`. Callers that need
unconditional replacement (the dashboard, for example) must apply the
returned board themselves.

**Example — read-only dashboard handler**

```rust
use simard::goal_curation::load_goal_board;
use simard::memory_ipc::open_reader_bridge;

let bridge = open_reader_bridge(&state_root)?;       // ReaderBridge
let board = load_goal_board(bridge.ops())?;          // .ops() → &dyn CognitiveMemoryOps
eprintln!("Loaded {} active goal(s)", board.active.len());
```

---

### `save_goal_board`

```rust
pub fn save_goal_board(
    board: &GoalBoard,
    bridge: &dyn CognitiveMemoryOps,
) -> SimardResult<()>
```

Persists the board to cognitive memory. Internally calls:

```rust
bridge.store_fact(
    "goal-board:snapshot",   // concept
    &serde_json::to_string(board)?,  // content
    1.0,                     // confidence
    &["goal-board".to_string()],  // tags
    "goal-curator",          // source_id
)?;
```

There is no disk write — cognitive memory is the sole authoritative store.
`persist_board` should be preferred for end-of-cycle saves: it calls
`save_goal_board` and additionally records a durable episode for
cross-session recall.

**Integrity guard (runs before any write)**

`save_goal_board` calls `board_integrity_suspect(board)` first. If that
returns `Some(reason)`, the function returns
`Err(SimardError::InvalidGoalRecord)` **without** invoking
`bridge.store_fact`. This blocks two classes of corruption:

- Active goals whose `id` is shorter than 5 characters (catches `g1`,
  `g12`, …).
- Active goals whose description matches the placeholder pattern
  `^\s*goal\s+[a-z0-9]{1,4}\s*$` (case-insensitive).

A board mutated by an LLM hallucination during the Decide phase that emits
goals like `Goal g1` is therefore rejected at write time and the previous
snapshot remains the authoritative state.

**Errors**

| Outcome | Behaviour |
|---------|-----------|
| `board_integrity_suspect` returns `Some(_)` | `Err(SimardError::InvalidGoalRecord { field: "board", reason: format!("refusing to persist suspect board: {reason}") })` — `store_fact` is not called |
| `serde_json::to_string(board)` fails | `Err(SimardError::InvalidGoalRecord { field: "board", reason: format!("failed to serialize goal board: {e}") })` |
| `bridge.store_fact` fails | The underlying `SimardError` is propagated via `?` |

The error is fatal for the caller: the in-memory mutation is not retained
anywhere else.

**Example — dashboard write handler**

```rust
use simard::goal_curation::{load_goal_board, save_goal_board};
use simard::memory_ipc::launch_writer_bridge;

let bridge = launch_writer_bridge(&state_root)?;     // WriterBridge or Err
let mut board = load_goal_board(bridge.ops())?;
board.active.retain(|g| g.id != deleted_id);
save_goal_board(&board, bridge.ops())?;
```

---

### `persist_board`

```rust
pub fn persist_board(board: &GoalBoard, bridge: &dyn CognitiveMemoryOps) -> SimardResult<()>
```

Calls `save_goal_board` and then `store_episode` with a human-readable
summary (active count, backlog count) so the board state appears in
cross-session memory recall.

Use this function at the end of an OODA cycle. Use `save_goal_board` for
intermediate saves where an episode record is not needed.

---

## Board mutation functions

### `add_active_goal`

```rust
pub fn add_active_goal(board: &mut GoalBoard, goal: ActiveGoal) -> SimardResult<()>
```

Appends an active goal. Fails if:
- `board.active.len() >= MAX_ACTIVE_GOALS` (capacity exceeded)
- A goal with the same `id` already exists in `board.active`

### `add_backlog_item`

```rust
pub fn add_backlog_item(board: &mut GoalBoard, item: BacklogItem) -> SimardResult<()>
```

Appends a backlog item. Fails if an item with the same `id` already exists.

### `enqueue_stewardship_issue`

```rust
pub fn enqueue_stewardship_issue(
    board: &mut GoalBoard,
    repo: &str,
    issue_number: u64,
    url: &str,
    signature: &str,
) -> SimardResult<()>
```

Idempotent. Derives a stable backlog ID using the format
`stewardship-<org>_<repo>-<number>` (forward slashes in the repository name
are replaced with underscores, e.g. `org/repo` → `stewardship-org_repo-42`),
then calls `add_backlog_item`. If a backlog item with that ID already exists
the call is a no-op. Default score: `DEFAULT_STEWARD_SCORE` (0.6).

### `promote_to_active`

```rust
pub fn promote_to_active(
    board: &mut GoalBoard,
    backlog_id: &str,
    priority: u32,
    assigned_to: Option<String>,
) -> SimardResult<()>
```

Removes a backlog item and inserts it as a `NotStarted` active goal with
the given priority. Fails if the board is at capacity or the backlog item
does not exist.

### `update_goal_progress`

```rust
pub fn update_goal_progress(
    board: &mut GoalBoard,
    goal_id: &str,
    progress: GoalProgress,
) -> SimardResult<()>
```

Updates the `status` field of an active goal. Fails if the goal is not found
or `progress` is `InProgress { percent }` with `percent > 100`.

### `clear_goal_assignment`

```rust
pub fn clear_goal_assignment(board: &mut GoalBoard, goal_id: &str) -> SimardResult<()>
```

Clears `assigned_to = None` and resets `status = NotStarted` for the named
goal. Used to unblock a goal whose engineer session has died.

Calling this function alone does not persist the change — call
`save_goal_board` or `persist_board` afterwards.

### `archive_completed`

```rust
pub fn archive_completed(board: &mut GoalBoard) -> Vec<ActiveGoal>
```

Removes all goals with `status == GoalProgress::Completed` from
`board.active` and returns them. Called at the end of each OODA Curate step.

---

## Seeding

### `seed_default_board`

```rust
pub fn seed_default_board(board: &mut GoalBoard) -> usize
```

If `board.active` is empty, inserts the five default starter goals defined in
`DEFAULT_SEED_GOALS`. Returns the number of goals added (0 or 5). Called
once per cycle after board load, before the Observe phase.

### `DEFAULT_SEED_GOALS`

```rust
pub const DEFAULT_SEED_GOALS: [(u32, &str, &str); 5]
```

The canonical list of starter goals. Each tuple is `(priority, slug_source, description)`.
Both `seed_default_board` (GoalBoard) and `seed_default_goals` (GoalStore) derive
their seeding from this constant.

### `DEFAULT_STEWARD_SCORE`

```rust
pub const DEFAULT_STEWARD_SCORE: f64 = 0.6
```

Default backlog score assigned to stewardship-filed issues by
`enqueue_stewardship_issue`.

---

## Adapters

### `active_goals_as_records` *(spec — not yet implemented)*

```rust
pub fn active_goals_as_records(board: &GoalBoard) -> Vec<GoalRecord>
```

Adapts a `GoalBoard` into the legacy `Vec<GoalRecord>` shape consumed by the
meeting REPL goal-curation flow, the meeting REPL improvement-curation flow,
and the engineer loop. Once implemented as part of issue #1590, this
adapter will be the single point of mapping between the cognitive-memory-backed
`GoalBoard` and the older `GoalRecord` value type that those subsystems
expect — replacing every existing `FileBackedGoalStore::try_new(...)`
call site.

**Field mapping** (from each `ActiveGoal` to its synthesized `GoalRecord`):

| `GoalRecord` field | Source on `ActiveGoal` | Notes |
|--------------------|------------------------|-------|
| `slug` | `slugify(active.id)` or `active.id` | If the id is already slug-shaped (lowercase, dashes, no whitespace) it passes through unchanged |
| `title` | `active.description` | Truncated to the first line, max 120 characters |
| `rationale` | `active.current_activity.unwrap_or_default()` | Empty string when no current activity is set |
| `status` | `Completed → GoalStatus::Completed`, all others → `GoalStatus::Active` | `NotStarted`, `InProgress`, and `Blocked` collapse to `Active` because the legacy `GoalRecord` has no equivalent variants |
| `priority` | `u8::try_from(active.priority).unwrap_or(u8::MAX)` | Saturates rather than panicking on overflow |
| `owner_identity` | `active.assigned_to.clone().unwrap_or_else(\|\| "unassigned".into())` | The literal string `"unassigned"` is used as a sentinel when no engineer is assigned |
| `source_session_id` | Sentinel `SessionId::parse("00000000-0000-0000-0000-000000000000")?` | The all-zeros UUID indicates "synthesized from goal-board snapshot, no originating session" |
| `updated_in` | `SessionPhase::Persistence` | Marks the record as having come from the persistence layer rather than a live phase |

Only goals from `board.active` are emitted. Backlog items are not adapted;
callers that need backlog data must read `board.backlog` directly.

**Example — engineer loop (target call site)**

```rust
use simard::goal_curation::{active_goals_as_records, load_goal_board};
use simard::memory_ipc::open_reader_bridge;

let bridge = open_reader_bridge(&state_root)?;
let board = load_goal_board(bridge.ops())?;
let next_five: Vec<GoalRecord> =
    active_goals_as_records(&board).into_iter().take(5).collect();
```

This will replace
`FileBackedGoalStore::try_new(state_root.join("goal_records.json"))?.active_top_goals(5)?`
at `src/engineer_loop/mod.rs:276`.

**Example — meeting REPL goal curation (target call site)**

```rust
let bridge = open_reader_bridge(&state_root)?;
let board = load_goal_board(bridge.ops())?;
let records = active_goals_as_records(&board);
present_records_to_curator(&records);
```

This will replace
`FileBackedGoalStore::try_new(state_root.join("goal_records.json"))?` at
`src/operator_commands_meeting/goal_curation.rs:58` and the analogous call
at `src/operator_commands_meeting/improvement_curation.rs:123`.

---

## `GoalProgress` variants

| Variant | Fields | Meaning |
|---------|--------|---------|
| `NotStarted` | — | Goal is queued; no engineer spawned yet |
| `InProgress` | `percent: u32` | Engineer session is active; 0–100 |
| `Completed` | — | Goal is done; will be archived at end of cycle |
| `Blocked` | `reason: String` | Cannot proceed; requires operator attention |

---

## Error variants

| `SimardError` variant | When raised |
|-----------------------|-------------|
| `InvalidGoalRecord { field, reason }` | Validation failure (empty field, priority 0, percent > 100, capacity exceeded, duplicate id, item not found), serialization failure, or integrity-guard rejection of a suspect board in `save_goal_board` |
| `BridgeTransportError { bridge, reason }` | A `bridge.store_fact` or `bridge.store_episode` call failed (propagated via `?` from `save_goal_board`/`persist_board`). `load_goal_board` does **not** raise this — it logs and degrades to an empty board instead |

There is no silent disk fallback for writes — when cognitive memory is
unavailable, `save_goal_board` fails and the in-memory mutation is lost.
For reads, the resilience contract (log + empty board) is documented above.

