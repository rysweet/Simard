# Reference: Goal board corruption guard APIs

Crate: `simard` · Modules: `simard::ooda_loop::orient`, `simard::ooda_loop::cycle`

Three `pub(crate)` functions implement the layered corruption defences
described in [Goal board corruption guards](../concepts/goal-board-corruption-guards.md).

---

## `filter_hallucinated_priorities`

```rust
pub(crate) fn filter_hallucinated_priorities(
    priorities: &mut Vec<Priority>,
    active_goals: &[ActiveGoal],
);
```

**Module:** `simard::ooda_loop::orient`

Mutates `priorities` in-place, retaining only entries whose `goal_id` is either:

- present in `active_goals` (matched by `goal_id == active_goal.id`), or
- a synthetic goal id (i.e. `goal_id.starts_with("__")`).

All other entries are dropped. For every dropped entry, a warning is written to
stderr:

```
[simard] OODA orient: dropping hallucinated goal_id '<id>' — not on active board
```

### When to call

Call this function after building the `Vec<Priority>` list from `active_goals`
(which may include LLM-adjusted urgencies from the orient brain) and **before**
appending synthetic priorities. The orient brain returns per-goal `OrientJudgment`
structs, not a priority list — priorities are constructed from `goals.active` in
`orient_with_brain` and then passed here. Synthetics are never sourced from the
LLM so they must not be passed through the filter.

The `orient_with_brain` function in `src/ooda_loop/orient.rs` calls this
automatically. Callers outside that function (tests, alternative orient paths)
must call it explicitly if they construct priorities from external input.

### Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `priorities` | `&mut Vec<Priority>` | Priority list from the orient brain. Modified in-place. |
| `active_goals` | `&[ActiveGoal]` | Current active board goals. Typically `state.active_goals.active.as_slice()`. |

### Example

```rust
let mut priorities = brain.judge_orientation(&ctx)?.priorities;
filter_hallucinated_priorities(&mut priorities, &state.active_goals.active);
// priorities now contains only validated entries
```

---

## `board_integrity_suspect`

```rust
pub(crate) fn board_integrity_suspect(
    board: &GoalBoard,
) -> Option<String>;
```

**Module:** `simard::ooda_loop::cycle`

Inspects every `ActiveGoal` in `board.active` and returns `Some(reason)` if the
board looks corrupt, or `None` if the board passes all checks.

### Heuristics

| Check | Condition | Example trigger |
|-------|-----------|-----------------|
| Short id | `goal.id.len() < 5` | `"g1"`, `"g12"`, `"g123"` |
| Placeholder description | `is_placeholder_description(&goal.description)` | `"Goal g1"`, `"GOAL abc"` |

The function returns on the **first** suspicious goal it finds; it does not
enumerate all problems.

### Return value

- `None` — board looks healthy; safe to use as cycle working state.
- `Some(reason)` — at least one goal triggered a heuristic; the `reason` string
  is a human-readable description suitable for log output.

### Example

```rust
if let Some(reason) = board_integrity_suspect(&board) {
    eprintln!("[simard] OODA start: rejecting board — {reason}");
    // fall through to seed_default_board
} else {
    state.active_goals = board;
}
```

---

## `is_placeholder_description`

```rust
pub(crate) fn is_placeholder_description(desc: &str) -> bool;
```

**Module:** `simard::ooda_loop::cycle`

Returns `true` when `desc` matches the placeholder pattern
`^\s*goal\s*[a-z0-9]{1,4}\s*$` (case-insensitive, leading/trailing whitespace
ignored; space between `"goal"` and the token is optional).

This is a pure helper used by `board_integrity_suspect`. It may be called
independently in tests or in additional validation sites.

### Match examples

| Input | Result |
|-------|--------|
| `"Goal g1"` | `true` |
| `"goal g1"` | `true` |
| `"GOAL abc"` | `true` |
| `"  goal g1  "` | `true` (trimmed) |
| `"goalg1"` | `true` (space optional) |
| `"Ship the v1 release"` | `false` |
| `"goal g1234"` | `false` (token is 5 chars, exceeds 4-char limit) |
| `""` | `false` |
| `"goal"` | `false` (no token after keyword) |

### Token length limit

The token after `"goal "` must be 1–4 characters (`[a-z0-9]{1,4}`). This
allows `g1`, `g12`, `g123` while excluding `g1234` (5 chars) and anything longer
— 5 characters is long enough to be a plausible real id in most naming schemes.

---

## Corruption guard in the cycle

The pre-cycle snapshot and curate-phase corruption check are not exposed as
standalone functions; they are implemented inline in `run_ooda_cycle`. The logic
is documented here for reference.

### Pre-cycle snapshot

Taken just before the Observe phase:

```rust
let pre_cycle_active_ids: HashSet<String> = state.active_goals.active
    .iter()
    .map(|g| g.id.clone())
    .collect();
```

### Curate-phase check

Run after `archive_completed` and `promote_from_backlog`, before `persist_board`:

```rust
let archived_ids: HashSet<&str> = archived.iter().map(|g| g.id.as_str()).collect();
let post_active_ids: HashSet<&str> = state.active_goals.active
    .iter()
    .map(|g| g.id.as_str())
    .collect();

let vanished: Vec<&str> = pre_cycle_active_ids.iter()
    .map(|s| s.as_str())
    .filter(|id| !post_active_ids.contains(*id) && !archived_ids.contains(*id))
    .collect();

if vanished.is_empty() {
    persist_board(&state.active_goals, &*bridges.memory)?;
} else {
    eprintln!("[simard] OODA curate: CORRUPTION DETECTED — {} goal(s) vanished without \
               archival: {}; skipping persist to protect board",
              vanished.len(), vanished.join(", "));
    // persist is skipped; last-known-good board remains on disk
}
```

A goal is considered **legitimately absent** only if it appears in `archived`
(i.e. it was moved by `archive_completed` during this cycle). Any goal that is
absent from both `active` and `archived` is treated as corruption.

---

## See also

- [Goal board corruption guards — concept](../concepts/goal-board-corruption-guards.md)
- [Goal board persistence](../concepts/goal-board-persistence.md)
- [OODA brain API](ooda-brain-api.md)
- [Goal board API](goal-board-api.md)
