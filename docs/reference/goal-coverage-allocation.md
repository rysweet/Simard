---
title: Goal coverage allocation API reference
description: Rust API reference for the per-cycle coverage allocator that guarantees every incomplete active goal has exactly one live engineer, with the AIMD scaler retained as a safety cap.
last_updated: 2026-06-22
owner: simard
doc_type: reference
status: reference
related:
  - ./adaptive-scaling-api.md
  - ./goal-target-repo-routing.md
  - ./spawn-agent-for-goal.md
  - ../concepts/adaptive-scaling.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ../howto/unblock-stuck-ooda-goals.md
---

# Goal coverage allocation API reference

> **Issue [#2359](https://github.com/rysweet/Simard/issues/2359).** Goal
> **coverage** is now a first-class allocation rule: each OODA cycle, every
> active goal that is **not-started or in-progress** is guaranteed exactly one
> live engineer — subject to the AIMD safety cap.

Module: `simard::ooda_loop::coverage`

## The problem

The Act phase caps the number of engineers spawned per cycle via the AIMD
[`AdaptiveScaler`](./adaptive-scaling-api.md)'s `max_concurrent_actions`. With
several active goals and prior in-flight engineers occupying slots, the Decide
phase — which orders work by **urgency** — could spend every slot adding
parallelism to goals that *already* had an engineer, leaving other INCOMPLETE
goals with **no** engineer for many cycles.

The operator expectation is simple: *every active goal that is not complete
should have an engineer actively working it.* Coverage makes that the primary
allocation rule, while preserving the AIMD cap as a rate-limit safety valve.

---

## Definitions

| Term | Meaning |
|------|---------|
| **Incomplete goal** | An active goal whose `status` is `NotStarted` or `InProgress { .. }`. `Proposed` (not yet accepted), `Paused` (operator hold), `Blocked(_)` (operator/safeguard hold), and `Completed` (done) are **excluded**. |
| **Covered goal** | A goal that already has a live engineer: either `assigned_to` names a live subordinate **or** [`find_live_engineer_for_goal`](./goal-target-repo-routing.md#interaction-with-in-flight-de-duplication) finds a live worktree pursuing it. |
| **Uncovered incomplete goal** | An incomplete goal with no live engineer. These are the goals coverage spawns for. |
| **Cap** | The current `max_concurrent_actions` from `scaler.current_max()` (or the static `OodaConfig` value when scaling is `fixed`). |

> **Why only `NotStarted`/`InProgress`?** Coverage spawns only for goals that
> are accepted, unblocked, active work. `Proposed` goals are not yet accepted
> onto the active board. `Paused` and `Blocked(_)` goals are deliberately parked
> by the operator (or the OODA safeguard) — spawning an engineer would fight
> that decision. `Completed` goals are done. So `Proposed`, `Paused`,
> `Blocked(_)`, and `Completed` are all excluded; only `NotStarted` and
> `InProgress` are candidates.

---

## Public API

### `ensure_goal_coverage`

```rust
pub fn ensure_goal_coverage(
    state: &OodaState,
    planned: &mut Vec<PlannedAction>,
    cap: usize,
) -> CoverageReport
```

Guarantees coverage by giving every incomplete goal that lacks a live engineer
exactly one `AdvanceGoal` action this cycle — ordered by priority and bounded by
`cap`.

**Algorithm:**

1. **Find incomplete goals.** Filter `state.active_goals.active` to goals that
   are incomplete (status `NotStarted` or `InProgress` — `Proposed`, `Paused`,
   `Blocked`, and `Completed` excluded).
2. **Split by live engineer.** A goal with a live engineer (`assigned_to`, via
   the existing in-flight detection) is already **covered**; any Decide-produced
   `AdvanceGoal` for it is *extra parallelism*. A goal **without** a live engineer
   **needs coverage**.
3. **Sort by priority.** Order the needs-coverage goals by `ActiveGoal.priority`
   (lower number = higher priority), ascending. Coverage explicitly sorts by
   **priority**, not the urgency ordering the Decide phase uses, so the most
   important goals are covered first.
4. **One action per needs-coverage goal.** Reuse that goal's Decide-produced
   spawn when one was planned (so coverage and Decide never produce two actions
   for the same goal — never double-spawn), otherwise synthesize one. Reusing the
   planned spawn — rather than dropping it and prepending a separate action — is
   what keeps an unassigned goal's own spawn from being evicted.
5. **Order, then cap.** Place the priority-ordered coverage actions ahead of all
   extra-parallelism and non-goal actions, then truncate the combined list to
   `cap`. Coverage therefore wins every contested slot, and because a goal's own
   spawn *is* its coverage action, a higher-priority goal's spawn is never evicted
   by a lower-priority goal's coverage. Extra parallelism for already-covered
   goals is dropped first.

**Returns** a `CoverageReport` (see below). `covered`/`deferred` are computed from
the post-cap survivors, so the report never counts a goal whose action the cap
dropped.

> **Coverage precedes parallelism.** Because coverage actions are ordered ahead of
> extra parallelism and the list is then truncated to `cap`, a slot is never spent
> on a *second* engineer for an already-covered goal while any incomplete goal
> remains uncovered.

> **The cap is a hard safety ceiling.** `ensure_goal_coverage` never emits more
> than `cap` total actions. If the uncovered set exceeds `cap`, it covers the
> top `cap` (highest priority) this cycle and defers the rest. Subsequent
> cycles cover the remainder as slots free up. The AIMD protection against CPU,
> memory, and 429 pressure is fully preserved — coverage cannot DoS the host.

### `CoverageReport`

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct CoverageReport {
    /// Number of incomplete goals that ended this cycle with an engineer
    /// (already-covered + newly covered this cycle).
    pub covered: usize,
    /// Total incomplete goals this cycle.
    pub incomplete: usize,
    /// Uncovered incomplete goals that could not be covered because the
    /// cap was reached. Covered on a subsequent cycle.
    pub deferred: usize,
}

impl CoverageReport {
    /// Operator log line: "covered N/M incomplete goals, deferred K due to cap".
    pub fn log_line(&self) -> String;
}
```

---

## Cycle integration

`ensure_goal_coverage` runs in the OODA cycle **between Decide and Act**, after
the Decide brain has produced its urgency-ordered actions and before `act`
dispatches them. The existing `planned_actions` binding in `run_ooda_cycle`
(`src/ooda_loop/cycle.rs`) changes from `let` to `let mut` so coverage can
reorder and bound it:

```rust
// --- Decide --- (existing branch; now bound with `let mut`)
let mut planned_actions = match clients.decide_brain.as_ref() {
    Some(brain) => decide_with_brain(&priorities, config, brain.as_ref())?,
    None => decide(&priorities, config)?,
};

// --- Coverage (issue #2359) ---
let cap = config
    .scaler
    .as_ref()
    .map(|s| s.current_max() as usize)        // reuse the AIMD value; no extra adjust()
    .unwrap_or(config.max_concurrent_actions as usize);
let report = ensure_goal_coverage(state, &mut planned_actions, cap);
eprintln!("[simard] OODA cycle: coverage — {}", report.log_line());

// --- Act (now dispatches the planned spawn-path `AdvanceGoal` actions
//     concurrently, bounded by this same `cap` — see
//     reference/concurrent-engineer-dispatch.md) ---
let outcomes = act(&planned_actions, clients, state, cap)?;
```

**Cap source.** Coverage reads `scaler.current_max()` — the value the Decide
phase's `scaler.adjust()` already settled on this cycle. Coverage does **not**
call `adjust()` again, so it neither double-counts pressure nor perturbs the
AIMD state machine.

**Log line.** Every cycle emits exactly one coverage line:

```
[simard] OODA cycle: coverage — covered 4/5 incomplete goals, deferred 1 due to cap
```

When everything fits under the cap, `deferred` is `0`:

```
[simard] OODA cycle: coverage — covered 3/3 incomplete goals, deferred 0 due to cap
```

---

## Worked examples

Assume five active goals and `cap = max_concurrent_actions`.

### Example 1 — all goals fit

| Goal | Priority | Status | Live engineer? |
|------|----------|--------|----------------|
| `g-a` | 1 | InProgress | yes |
| `g-b` | 2 | NotStarted | no |
| `g-c` | 3 | NotStarted | no |

`cap = 5`. Uncovered incomplete: `g-b`, `g-c`. Coverage adds an `AdvanceGoal`
for `g-b` and `g-c` ahead of any extra parallelism; total ≤ 5. Result: all three
covered. `covered 3/3 incomplete goals, deferred 0 due to cap`.

### Example 2 — cap forces a defer

| Goal | Priority | Status | Live engineer? |
|------|----------|--------|----------------|
| `g-a` | 1 | InProgress | yes |
| `g-b` | 2 | NotStarted | no |
| `g-c` | 3 | NotStarted | no |
| `g-d` | 4 | NotStarted | no |

`cap = 2` (AIMD has scaled down under pressure). Uncovered incomplete (priority
order): `g-b`, `g-c`, `g-d`. Coverage can spawn at most 2 → covers `g-b` and
`g-c`; `g-d` deferred. Next cycle covers `g-d` once a slot frees.
`covered 3/4 incomplete goals, deferred 1 due to cap`.

### Example 3 — only NotStarted/InProgress are covered

| Goal | Priority | Status | Live engineer? |
|------|----------|--------|----------------|
| `g-a` | 1 | Completed | no |
| `g-b` | 2 | Blocked("operator hold") | no |
| `g-c` | 3 | Paused | no |
| `g-d` | 4 | Proposed | no |
| `g-e` | 5 | NotStarted | no |

Only `g-e` is incomplete-and-uncovered. `g-a` (Completed), `g-b` (Blocked),
`g-c` (Paused), and `g-d` (Proposed) are all excluded. `covered 1/1 incomplete
goals, deferred 0 due to cap`.

### Example 4 — never double-spawn

A goal with `assigned_to = Some("eng-…")` naming a **live** subordinate, or a
live worktree found by `find_live_engineer_for_goal`, is counted as covered and
gets **no** new action — even if the Decide phase also emitted one. Coverage
de-dups against both live engineers and already-planned actions.

---

## Invariants

- **Cap is never exceeded.** `planned_actions.len() <= cap` after
  `ensure_goal_coverage` returns.
- **No double-spawn.** At most one action (and at most one live engineer) per
  `goal_id`, enforced by reusing the existing in-flight detection.
- **Coverage ≥ parallelism.** No slot is spent on a second engineer for an
  already-covered goal while any incomplete goal is uncovered and a slot is
  available.
- **Priority order.** Uncovered goals are covered strictly in ascending
  `priority` order; deferral always falls on the lowest-priority overflow.
- **Idempotent under coverage.** If every incomplete goal is already covered,
  `ensure_goal_coverage` adds nothing and reports `deferred = 0`.

---

## Related reading

- [Maximum safe parallelism](./maximum-safe-parallelism.md) — how coverage,
  the AIMD cap, and goal decomposition combine to fill spare machine capacity
  with concurrent engineers on distinct work items.
- [Concurrent engineer dispatch](./concurrent-engineer-dispatch.md) — how the
  Act phase dispatches the spawn-path `AdvanceGoal` actions coverage plans
  **concurrently**, each with its own LLM session, bounded by this same cap.
- [Adaptive scaling API](./adaptive-scaling-api.md) — the AIMD scaler that
  supplies the safety cap.
- [Goal target-repo routing](./goal-target-repo-routing.md) — the companion
  #2359 fix that routes each engineer to the correct repo.
- [How OODA spawns engineer agents](../howto/spawn-engineers-from-ooda-daemon.md)
  — the dispatch path coverage feeds into.
- [How to unblock stuck OODA goals](../howto/unblock-stuck-ooda-goals.md) —
  why `Blocked` goals are intentionally excluded from coverage.
