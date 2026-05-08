---
title: Goal board API reference
description: Rust API reference for goal board persistence and mutation functions in src/goal_curation/operations.rs.
last_updated: 2026-05-08
owner: simard
doc_type: reference
related:
  - ../concepts/goal-board-persistence.md
  - ../howto/recover-goal-board.md
  - ../howto/inspect-durable-goal-register.md
---

# Goal board API reference

This document covers the public functions in
`src/goal_curation/operations.rs` that load, save, and mutate the goal board
(`GoalBoard`) used by the OODA cycle.

---

## Persistence functions

### `load_goal_board`

```rust
pub fn load_goal_board(bridge: &dyn CognitiveMemoryOps) -> SimardResult<GoalBoard>
```

Loads the goal board using a three-tier fallback strategy:

| Priority | Source | Notes |
|----------|--------|-------|
| 1 (primary) | `$SIMARD_STATE_ROOT/goal_records.json` | Written by `save_goal_board` on every save |
| 2 (fallback) | `search_facts("goal-board:snapshot", 1, 0.0)` | Cognitive memory; used before disk file exists |
| 3 (last resort) | `GoalBoard::new()` | Empty board if both sources fail |

**Parameters**

| Name | Type | Description |
|------|------|-------------|
| `bridge` | `&dyn CognitiveMemoryOps` | Cognitive memory adapter (tier-2 source) |

**Return value**

Returns `Ok(GoalBoard)` in the common case. Disk read and parse errors (tiers
1) fall through silently to tier 2 and are logged to stderr. Cognitive memory
bridge failures at tier 2 are propagated as `Err` via the `?` operator — the
function does not fall through to tier 3 if `search_facts` itself fails.
Tier-3 empty board is returned only when `search_facts` succeeds but finds no
matching snapshot.

**Note on empty boards:** the calling cycle only applies the loaded board to
in-memory state if `board.active.is_empty() == false`. Callers that need
unconditional replacement must apply the returned board themselves.

**Example**

```rust
use simard::goal_curation::load_goal_board;

let board = load_goal_board(&*bridges.memory)?;
eprintln!("Loaded {} active goal(s)", board.active.len());
```

---

### `save_goal_board`

```rust
pub fn save_goal_board(board: &GoalBoard, bridge: &dyn CognitiveMemoryOps) -> SimardResult<()>
```

Saves the board to two destinations in the following order:

1. Cognitive memory (`store_fact("goal-board:snapshot", …, tags=["goal-board"])`)
2. `$SIMARD_STATE_ROOT/goal_records.json` — pretty-printed JSON, best-effort
   (disk write failure is logged but not propagated)

Prefer `persist_board` for end-of-cycle saves — it calls `save_goal_board`
and additionally records a durable episode for cross-session recall.

**Errors**

Returns `Err` only if the cognitive memory write fails. Disk write failures
are silently logged.

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
| `InvalidGoalRecord { field, reason }` | Validation failed (empty field, priority 0, percent > 100, capacity exceeded, duplicate id, item not found) |

Disk I/O errors from tier 1 of `load_goal_board` are not propagated — they
fall through to tier 2 and are logged to stderr. Cognitive memory bridge
failures (tier 2) **are** propagated as `Err`.
