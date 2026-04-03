---
title: OODA Meeting Handoff Integration & Goal Seeding
description: Design for wiring meeting handoffs into the OODA daemon and seeding default goals — Issues #157 and #158.
last_updated: 2026-04-03
owner: simard
doc_type: architecture-decision
issues: ["#157", "#158"]
---

# OODA Meeting Handoff Integration & Goal Seeding

## Problem

Meeting decisions and action items are written to `target/meeting_handoffs/` as
JSON artifacts when meetings close (`meeting_repl.rs` → `write_meeting_handoff`).
Currently, these artifacts are consumed in two places:

1. **Engineer loop** (`engineer_loop.rs:188–205`): injects unprocessed handoff
   decisions and action items into the next engineer prompt as context.
2. **CLI `act-on-decisions`** (`operator_cli.rs:516–583`): creates GitHub issues
   from handoff decisions and action items, then marks the handoff processed.

Neither path converts meeting outcomes into OODA goals. The OODA daemon runs
independently but has no awareness of meeting artifacts, meaning decisions
agreed upon in meetings never become tracked goals unless manually created.

Additionally, when Simard starts fresh with an empty goal store, the OODA loop
has nothing to prioritize — it runs cycles with zero active goals, producing
no useful work.

## Solution

### 1. `check_meeting_handoffs` — ooda_loop.rs

Add a function at the start of `run_ooda_cycle` that:

1. Reads `target/meeting_handoffs/meeting_handoff.json` via
   `meeting_facilitator::load_meeting_handoff`.
2. Skips if no file or already `processed == true`.
3. For each `MeetingDecision`, creates an `ActiveGoal` on the `GoalBoard`:
   - `id`: derived from a slugified form of `decision.description` (lowercase, hyphens, truncated to 64 chars)
   - `description`: `"[meeting] {decision.description}"`
   - `priority`: based on position in the decisions list (earlier = higher)
   - `status`: `GoalProgress::NotStarted`
4. For each `ActionItem` with priority >= 2, creates a `BacklogItem`:
   - `id`: derived from a slugified form of `action_item.description`
   - `description`: `"[action] {action_item.description} (owner: {owner})"`
   - `source`: `"meeting:{topic}"`
   - `score`: mapped from action item priority (higher priority → higher score)
5. Marks the handoff as processed via `mark_meeting_handoff_processed`.
6. Logs the number of goals and backlog items created.

**Placement in cycle**: Before the Observe phase. Meeting handoffs are
pre-cycle inputs, not observations. They modify the goal board directly so
that the subsequent Observe → Orient → Decide → Act phases operate on the
updated board.

```
run_ooda_cycle:
    load_goal_board(memory)          // existing
    check_meeting_handoffs(state)    // NEW — converts handoffs to goals
    observe(state, bridges)          // existing
    orient(observation, goals)       // existing
    decide(priorities, config)       // existing
    act(actions, bridges, state)     // existing
    curate(state)                    // existing
```

**Deduplication**: Before adding a goal, check `state.active_goals.active`
for an existing goal with the same id. Skip if already present. This prevents
re-processing if the handoff was only partially processed in a previous cycle.

**Cap enforcement**: `GoalBoard` enforces `MAX_ACTIVE_GOALS = 5`. If the
board is full, excess meeting-derived goals go to the backlog instead.

### 2. `seed_default_goals` — goals.rs

Add a function that populates the goal store with 5 starter goals when it
is empty. Called once during OODA daemon initialization (before the first
cycle).

**Default goals** (reflecting Simard's core purpose):

| # | Title | Rationale | Priority |
|---|-------|-----------|----------|
| 1 | Keep top-5 goals honest and current | Goals must reflect actual priorities | 1 |
| 2 | Improve gym evaluation scores | Continuous quality measurement | 2 |
| 3 | Consolidate episodic memory into semantic | Prevent memory bloat, improve recall | 2 |
| 4 | Advance the highest-priority open issue | Ship code that matters | 1 |
| 5 | Review and curate the backlog | Keep backlog actionable, not stale | 3 |

**Implementation**: Uses `GoalStore::list()` to check emptiness, then
`GoalStore::put()` for each default. Constructs goals through the existing
`GoalStore` write path to ensure validation is applied consistently.

**Idempotency**: Only seeds when `list()` returns an empty vec. If any goals
exist (even completed ones), seeding is skipped. This prevents overwriting
user-curated goals on restart.

## Files Modified

| File | Change |
|------|--------|
| `src/ooda_loop.rs` | Add `check_meeting_handoffs(&mut OodaState)` function; call it at the top of `run_ooda_cycle` |
| `src/goals.rs` | Add `seed_default_goals(store: &dyn GoalStore, session_id, phase)` function |
| `src/ooda_loop.rs` | Import `meeting_facilitator::{load_meeting_handoff, mark_meeting_handoff_processed, default_handoff_dir}` |
| `src/goal_curation.rs` | No changes — existing `ActiveGoal`, `BacklogItem`, and board cap logic are sufficient |

## Integration Points

- **`meeting_facilitator.rs`**: Read-only consumer of `load_meeting_handoff`,
  `mark_meeting_handoff_processed`, `default_handoff_dir`, `MeetingHandoff`.
- **`goal_curation.rs`**: Uses `GoalBoard`, `ActiveGoal`, `BacklogItem`,
  `GoalProgress`, `MAX_ACTIVE_GOALS`, `promote_to_active`.
- **`goals.rs`**: Uses `GoalStore`, `GoalUpdate`, `GoalRecord`, `GoalStatus`.
- **`ooda_actions.rs`**: No changes — dispatched actions already handle
  `AdvanceGoal` for any active goal regardless of origin.

## Degradation Behavior (Pillar 11)

- If `target/meeting_handoffs/` doesn't exist or is unreadable,
  `check_meeting_handoffs` logs a warning and continues. The OODA cycle
  is not interrupted.
- If `seed_default_goals` fails to write to the store, the error is logged
  but the daemon proceeds with an empty board. The next cycle will retry
  loading from cognitive memory.

## Testing

- Unit test: `check_meeting_handoffs` with a temp dir containing a handoff
  JSON, verify goals are added to the board and handoff is marked processed.
- Unit test: `check_meeting_handoffs` with already-processed handoff, verify
  no-op.
- Unit test: `seed_default_goals` on empty store, verify 5 goals created.
- Unit test: `seed_default_goals` on non-empty store, verify no-op.
- Integration: Full OODA cycle with a handoff artifact, verify the meeting
  decision appears as a goal in the cycle report.
