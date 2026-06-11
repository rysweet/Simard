---
title: OODA Meeting Handoff Integration & Goal Seeding
description: Design for wiring meeting handoffs into the OODA daemon and seeding default goals — Issues #157 and #158.
last_updated: 2026-06-11
owner: simard
doc_type: architecture-decision
issues: ["#157", "#158", "#2269"]
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

### 1. `check_meeting_handoffs` — ooda_loop/curate.rs

A function at the start of `run_ooda_cycle` that **batch-processes up
to 10 unprocessed handoffs per cycle** (oldest-first FIFO):

1. Calls `find_unprocessed_handoffs(handoff_dir, 10)` to list up to 10
   unprocessed handoff files (timestamped `handoff-*.json` and the
   legacy `meeting_handoff.json`), sorted by filename ascending for
   deterministic FIFO ordering. The legacy file counts toward the
   limit.
2. For each handoff in the returned batch:
   a. Parses the JSON via `serde_json`. On parse failure: logs `warn!`,
      skips to the next file without aborting the batch.
   b. If `decisions.is_empty() && action_items.is_empty()`: marks the
      handoff processed immediately and skips ingestion. This handles
      legacy empty handoffs written before the write-side filter.
   c. Otherwise: for each `MeetingDecision`, creates an `ActiveGoal`
      on the `GoalBoard` (id from slugified description, priority from
      position, status `NotStarted`). For each `ActionItem` with
      priority ≥ 2, creates a `BacklogItem`.
   d. Calls `mark_handoff_processed_in_place` to flip `processed` to
      `true` on the specific file path.
3. Accumulates the total `created` count across all handoffs in the
   batch and returns it.
4. Logs the total count and batch size.

The batch limit of 10 prevents stalling a cycle on a large historical
backlog — the queue drains over successive cycles.

**Deduplication**: Before adding a goal, check `state.active_goals.active`
for an existing goal with the same id. Skip if already present. This prevents
re-processing if the handoff was only partially processed in a previous cycle.

**Cap enforcement**: `GoalBoard` enforces `MAX_ACTIVE_GOALS = 5`. If the
board is full, excess meeting-derived goals go to the backlog instead.

### 1a. Empty handoff filtering at write time

The write side (`write_handoff_with_explicit` in
`src/meeting_backend/persist/mod.rs`) skips writing to the handoff
directory when the meeting produced zero decisions **and** zero action
items. This prevents empty meetings from creating handoff files that
the OODA daemon would parse and immediately discard. The per-meeting
archival bundle (`~/.simard/meetings/`) is still written regardless.

### 1b. Handoff reaper

The OODA daemon's resource cleanup phase (end of each cycle) calls
`reap_processed_handoffs(handoff_dir, Duration::from_secs(7 * 86400))`
to delete processed handoff files older than 7 days. The reaper
requires both `processed == true` and `mtime > max_age` before
deleting. The well-known `meeting_handoff.json` is excluded from
reaping.

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
| `src/ooda_loop/curate.rs` | Replace single-handoff ingestion with `find_unprocessed_handoffs(dir, 10)` batch loop; accumulate `created` across iterations |
| `src/meeting_facilitator/handoff/persistence.rs` | Add `find_unprocessed_handoffs(dir, limit)` returning `Vec<PathBuf>` and `reap_processed_handoffs(dir, max_age)` |
| `src/meeting_facilitator/handoff/mod.rs` | Re-export `find_unprocessed_handoffs` and `reap_processed_handoffs` |
| `src/meeting_facilitator/mod.rs` | Re-export new public functions through the module facade |
| `src/meeting_backend/persist/mod.rs` | Add early-return guard in `write_handoff_with_explicit` skipping empty handoffs |
| `src/ooda_loop/cycle.rs` | Call `reap_processed_handoffs` in the resource cleanup block |
| `src/goals.rs` | Add `seed_default_goals(store: &dyn GoalStore, session_id, phase)` function (unchanged) |
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

- If `~/.simard/meeting_handoffs/` doesn't exist or is unreadable,
  `check_meeting_handoffs` logs a warning and continues. The OODA cycle
  is not interrupted.
- If an individual handoff file fails to parse within a batch, the error
  is logged at `warn!` level and processing continues with the remaining
  files. One bad file does not block the queue.
- If `reap_processed_handoffs` encounters a filesystem error (e.g., mtime
  unavailable), it logs a warning and skips the problematic file. The
  cleanup phase always completes.
- If `seed_default_goals` fails to write to the store, the error is logged
  but the daemon proceeds with an empty board. The next cycle will retry
  loading from cognitive memory.

## Testing

- Unit test: `check_meeting_handoffs` with a temp dir containing multiple
  handoff JSON files, verify batch processes all (up to 10) and goals are
  added to the board. Verify FIFO ordering preserved within the batch.
- Unit test: `check_meeting_handoffs` with empty handoffs (0 decisions,
  0 action items), verify they are marked processed without ingestion.
- Unit test: `check_meeting_handoffs` with already-processed handoff, verify
  no-op (excluded from `find_unprocessed_handoffs` results).
- Unit test: `write_handoff_with_explicit` with 0 decisions and 0 action
  items, verify no `handoff-*.json` created in the handoff directory.
- Unit test: `find_unprocessed_handoffs` returns files sorted oldest-first
  and respects the `limit` parameter.
- Unit test: `reap_processed_handoffs` deletes only files with
  `processed == true` AND mtime older than `max_age`.
- Unit test: `reap_processed_handoffs` skips `meeting_handoff.json`.
- Unit test: `reap_processed_handoffs` skips files with `processed == false`.
- Unit test: `seed_default_goals` on empty store, verify 5 goals created.
- Unit test: `seed_default_goals` on non-empty store, verify no-op.
- Integration: Full OODA cycle with a handoff artifact, verify the meeting
  decision appears as a goal in the cycle report.
