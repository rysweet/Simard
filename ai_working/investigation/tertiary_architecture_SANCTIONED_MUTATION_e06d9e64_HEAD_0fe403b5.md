# Tertiary (Architect) — Sanctioned Board Mutation & Close Command for `goal:steward-ci-github-actions-health-across-all-gov-e06d9e64`

HEAD: `0fe403b5` · Role: TERTIARY / architecture · Scope: **track/link only**.
Deliverable = the *sanctioned* durable mutator, the standing/`is_perpetual`
disposition, the **exact** close command, and a live-loop `BoardWriteLock`
safety verdict. Do **not** fix CI bugs, do **not** redesign the gap-scan.

All claims below are re-verified against source at this HEAD (prior dives
`CONSOLIDATED_e06d9e64_GAP_CLOSURE.md` and `..._MINIMAL_DURABLE_..._7cb152ff.md`
reconfirmed; two corrections noted in §6).

---

## 1. The two storage layers — and which one the predicate reads

There is **not** one board store; there are two, and the sanctioned mutator must
touch both. Getting this wrong is the whole reason the gap recurred.

| Layer | Written by | Read by | Lock |
|---|---|---|---|
| **Authoritative file store** `goal_board_store` | CLI via `with_board` → `goal_board_store::mutate` (`operator_cli/goal.rs:200`) | `goal_board_store::load` | its own store lock (`mutate`) |
| **Memory snapshot** fact `goal-board:snapshot` | `save_goal_board` / `overwrite_memory_cache` (`operations.rs:549,760`) | **Overseer gap-scan** `BoardGoalCurator::load` → `load_goal_board(mem)` (`wiring.rs:675-680`) | `BoardWriteLock` (flock) |

**The predicate reads the memory snapshot, not the file store.** `detect_workstream_gaps`
(`sensor.rs:288`) is fed by `BoardGoalCurator::workstream_gaps` (`wiring.rs:750`)
whose `self.load()` calls `load_goal_board(self.mem)` — i.e. the
`goal-board:snapshot` cognitive-memory fact. **Any durable close MUST supersede
that fact**, or the Overseer keeps re-flagging even after the file store changes.

## 2. Coverage predicate (single condition to flip) — reconfirmed verbatim

`goal_has_active_workstream` (`sensor.rs:377-384`):
```rust
fn goal_has_active_workstream(goal: &ActiveGoal) -> bool {
    if goal.assigned_to.is_some() { return true; }          // A) TRANSIENT (swept each cycle)
    goal.wip_refs.iter()
        .any(|w| matches!(w.kind.as_str(), "pr" | "branch" | "session" | "engineer")) // B) DURABLE
}
```
Emission gate (`sensor.rs:298-309`): active goal, `!Blocked`, `priority <= 2`,
`!goal_has_active_workstream`, and `goal:<id> ∉ coverage`. `coverage` is `issue:`
strings only (`wiring.rs`), never `goal:` — so the **board entry is the sole
lever**. A merged PR (#4181) and issue-kind wip_ref (#4172) cover nothing.

`detect_workstream_gaps` iterates **only `board.active`** (`sensor.rs:298`).
Removing the goal from `active` → permanently out of scope. That is the
smallest durable flip.

## 3. The sanctioned mutator (exact code path)

**`simard goal complete <id>` is the sanctioned close.** Trace:

`handle_complete` (`goal.rs:657`) →
- `with_board(...)` (`goal.rs:192-218`): `goal_board_store::mutate` on the
  **authoritative store under its store lock**, retain-filtering the id out of
  `active`+`backlog` (`goal.rs:674-677`), then **`overwrite_memory_cache(&committed)`**
  (`goal.rs:214`) — a **blind** `store_fact_with_caller_key("goal-board:snapshot",…)`
  (`operations.rs:760-770`) that **supersedes** the fact the Overseer reads.
- `tombstone(&[id])` (`goal.rs:242` → `ooda_loop::tombstone_goals`) so no
  re-seed / recall / meeting handoff resurrects it.

**Why the blind cache overwrite matters (correction to the merge story):**
`save_goal_board` is **merge-on-write** — persisted ⊕ in-flight, in-flight wins
on *collision by id* but a **removed** id is re-merged back from the persisted
snapshot (`operations.rs:592-603`). So *removals* need either
`save_goal_board_with_removals` (`operations.rs:664`, filters `force_remove_ids`
post-merge) **or** the CLI's blind `overwrite_memory_cache`. `handle_complete`
uses the latter → the removed goal is **not** resurrected. This is precisely why
hand-mutating a board in memory and calling plain `save_goal_board` would fail to
close the gap, and why the CLI is the sanctioned path.

## 4. standing / `is_perpetual` disposition

There is **no boolean field**. `ActiveGoal::is_perpetual()` (`types.rs:292`) is
**derived from the description** via `description_marks_standing` (the `[standing]`
marker / "standing goal" phrases, `types.rs:64-108`). Consequences:

- **This board entry is NOT marked standing** (verified prior: `standing/is_perpetual = None`;
  description carries no `[standing]` marker). Therefore `handle_complete`'s
  `is_perpetual()` guard (`goal.rs:668-673`) is **false** → it takes the
  **remove + tombstone** branch, not the reopen branch. `simard goal complete`
  **will succeed** and durably close it.
- If it *were* standing, `complete` **refuses** to terminate and calls
  `roll_to_new_cycle()` (`goal.rs:671`, `types.rs:308`) — reopen, no removal, no
  tombstone. A standing goal then **cannot** be closed by `complete` at all, and
  the predicate has **no standing exemption** (only `Blocked` is exempt,
  `sensor.rs:300`), so it would oscillate uncovered forever unless given a
  durable `pr|branch|session|engineer` wip_ref.
- There is **no CLI to mark an existing goal standing** — only `goal add
  --standing` (`goal.rs:588`) prepends the marker at creation.

**Disposition ruling (reconciled with the ambiguity resolution):** the concrete
stewardship deliverable **PR #4181 merged**, and the entry is **not** flagged
perpetual on the board. Treat it as a **one-shot delivered** goal → **complete +
tombstone**. This matches the CONSOLIDATED row-2 "dedupe (mark-done)" verdict. A
perpetual-steward disposition is *only* correct if the operator re-creates it via
`goal add --standing` **and** attaches an `engineer`-kind wip_ref — otherwise a
standing entry with no durable wip_ref re-oscillates (systemic, tracked via #4126;
out of scope here).

## 5. Exact close command + `BoardWriteLock` / live-loop safety

**Command (durable, one-shot):**
```
simard goal complete steward-ci-github-actions-health-across-all-gov-e06d9e64
```

**Live-loop safety verdict: SAFE to run against a live OODA/Overseer loop.**
- The CLI mutation serializes on the authoritative `goal_board_store::mutate`
  store lock; the daemon's snapshot flush serializes on `BoardWriteLock` (flock
  `LOCK_EX`, `operations.rs:181-241`, active on `#[cfg(all(unix, not(test)))]`)
  plus the process-local `SAVE_GOAL_BOARD_MUTEX`. Neither the daemon nor the CLI
  can interleave a read-merge-write window (the #2511 fix).
- The one residual race is **ordering**, not corruption: if the daemon flushes a
  `save_goal_board` snapshot *after* the CLI's `overwrite_memory_cache`, the
  daemon's in-flight board (which still contains the goal) could merge it back
  into the memory fact — **but** the tombstone + authoritative-store removal make
  the next daemon `load()`/reconcile drop it again. Practically the goal stays
  gone; worst case it re-flags for at most one tick, then the tombstone wins.
- **`BoardWriteLock` contention is NOT a blocker** — the flock is held only for
  the microsecond read-merge-write window, not across recipe execution.
- **Hand-editing `~/.simard/state/goal_board.json` is UNSAFE**: it bypasses both
  the store lock and `BoardWriteLock`, and the next merge-on-write resurrects the
  edit. Never do it under a live loop.

## 6. Corrections to prior dives (source-accurate at `0fe403b5`)

1. `WipRef`'s value field is **`ref_id`** (`types.rs:129`), not `reference` as the
   `7cb152ff` note wrote. A standing-path wip_ref is
   `WipRef { kind: "engineer", ref_id: "<branch/PR>", label: "…", url: None }`.
2. The sanctioned close does **not** go through `save_goal_board`/`BoardWriteLock`
   directly — it goes through `goal_board_store::mutate` (authoritative store
   lock) **+** `overwrite_memory_cache` (blind supersede of the snapshot fact).
   `save_goal_board_with_removals` (`operations.rs:664`) is the equivalent
   removal-safe mutator for the memory-snapshot layer if a programmatic path is
   preferred over the CLI. Both are sanctioned; `simard goal complete` is the
   operator-facing one.

## 7. Verification recipe (definition of done)

1. Run `simard goal complete steward-ci-github-actions-health-across-all-gov-e06d9e64`.
2. `BoardGoalCurator::workstream_gaps(&[])` (or the Observe pass) must **not**
   return `goal:steward-ci-github-actions-health-across-all-gov-e06d9e64`.
3. Next Overseer tick: `workstream_gaps_detected` for this signature = 0.
4. Regression contracts unchanged:
   `tests_gap_scan.rs::ignores_goal_covered_by_pr_assignment_or_coverage_set`
   and `::detects_uncovered_p1_goal_and_unaddressed_anomaly`.
5. Confirm the tombstone persists (goal not resurrected across a simulated
   reconcile tick).

## 8. One-line answer

Sanctioned durable close = **`simard goal complete steward-ci-github-actions-health-across-all-gov-e06d9e64`**
— it removes the entry from the authoritative store, **blind-supersedes** the
`goal-board:snapshot` memory fact the gap-scan reads (via `overwrite_memory_cache`,
avoiding merge-resurrection), and tombstones it; the entry is **not** `is_perpetual`,
so `complete` removes rather than reopens; runs **safely** against a live loop under
the store lock + `BoardWriteLock`, and hand-editing the JSON is the only unsafe path.
