---
title: "Overseer recurrence dead-band escalation API"
description: >
  Public surface of the Overseer's recurrence dead-band escalation: a lower
  escalation floor for a PERPETUAL, no-progress goal whose SAME root cause keeps
  re-parking. When such a goal's goal-scoped `RootCause.recurrence` — recalled
  from cognitive memory — reaches `PERPETUAL_RECURRENCE_ESCALATION_THRESHOLD`
  (2), the Overseer escalates the root cause once instead of blindly
  re-unblocking it every cycle, closing the [2,3) dead-band below the general
  `RECURRENCE_ESCALATION_THRESHOLD` (3). Covers the new `decide_blocked_goal`
  branch, the decision table, the reachable goal-scoped recurrence source, the
  unchanged `Intervention::EscalateBlockedGoal` /
  `OperatorNotification::goal_blocked_with_why` / `blocked_goal_gate` dedup path,
  the loop-termination guarantee, constants, and the test map.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: reference
status: design — not yet implemented
related:
  - ../concepts/overseer-root-cause-why.md
  - ../howto/configure-overseer-recurrence-escalation.md
  - ./overseer-root-cause-why-api.md
  - ./overseer-goal-board-health-api.md
  - ./overseer-operator-notifications.md
  - ./overseer-memory-recall-api.md
---

# Overseer recurrence dead-band escalation API reference

> **Status: design — not yet implemented** (issue
> [#4124](https://github.com/rysweet/Simard/issues/4124)). This document is the
> **binding design specification** for the recurrence dead-band escalation — it
> describes the feature we are about to build, not code that already ships. The
> new `PERPETUAL_RECURRENCE_ESCALATION_THRESHOLD` constant and the one new
> decision branch in `decide_blocked_goal` are the **contract the implementing
> PR will add** to `src/overseer/`; the implementation and this documentation
> land in the **same pull request**. The change is **purely additive**: it adds
> one compile-time constant and one decision branch. `decide_blocked_goal`'s
> signature is **unchanged**, no public type, enum variant, or notification
> constructor is renamed or removed, and every existing Overseer test keeps
> passing.

Module: `simard::overseer`.
Primary source: `src/overseer/mod.rs` (`decide_blocked_goal` and its GoalHygiene
call-site). Constants live in `src/overseer/root_cause.rs`.
Tests: `src/overseer/tests_goal_health.rs`, `src/overseer/tests_root_cause.rs`.

For the conceptual model of root-cause recurrence and the antipattern this
eliminates, see
[Overseer root-cause ("WHY") principle](../concepts/overseer-root-cause-why.md).
For operator configuration and end-to-end verification, see
[Configure and verify Overseer recurrence escalation](../howto/configure-overseer-recurrence-escalation.md).

## The problem this closes: the [2,3) recurrence dead-band

A blocked goal's root-cause recurrence is tracked by a **single, goal-scoped
counter**, `RootCause.recurrence`, folded into `Problem.why` from cognitive
memory recall. Before this feature the recurrence counter drove escalation
through exactly one floor:

| Floor constant | Value | Effect |
| -------------- | ----- | ------ |
| `RECURRENCE_ESCALATION_THRESHOLD` (`root_cause.rs`) | `3` | Any recurring cause at `recurrence >= 3` escalates to the operator. |

Below that floor, a **perpetual, no-progress** goal fell into the false-park
self-heal branch (`perpetual && is_no_progress_marker`) and was
**re-`UnblockGoal`-ed every cycle**. So a genuinely-blocked perpetual goal whose
cause had already recurred **twice** (`recurrence == 2`) sat in the **`[2, 3)`
dead-band**: recalled twice from cognitive memory, yet still re-unblocked,
re-parked, and re-emitting the **identical** signature on the next cycle — the
operator's explicitly-rejected "unblock it every cycle" antipattern, and a
silent, non-terminating loop. This is exactly the recurring observation

`overseer-obs:goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca`

reported in issue #4124 ("recurring signature **seen 2** in cognitive memory").

The fix adds a **lower escalation floor for the re-park loop only**: a
**perpetual, no-progress** goal whose goal-scoped `recurrence` reaches
`PERPETUAL_RECURRENCE_ESCALATION_THRESHOLD` (2) escalates its root cause **once**
instead of self-healing again. The general `RECURRENCE_ESCALATION_THRESHOLD` (3)
still governs every other recurring cause. Two floors, **one reachable counter**
— no second lane, no evidence fold.

### Why this counter is reachable (correctness)

`recurrence` is **goal-scoped**, which is what makes escalation actually fire for
#4124's scenario:

- **Write:** `Overseer::record_occurrence` stores each acted occurrence with
  `signature = entry.key` = `goal:blocked:{goal_id}` (`mod.rs`, `StoredOccurrence`).
- **Read:** `recall_occurrences` filters on exactly that `goal:blocked:{goal_id}`
  key, and `root_cause::analyze` folds the recalled count into
  `RootCause.recurrence`.

Because both sides key on the same goal-scoped string, each re-park that the
Overseer self-heals is recorded, and the next recall raises `recurrence`. So
`recurrence` **reliably climbs to 2** for a re-parking perpetual goal — this is
the corroboration issue #4124 describes ("seen 2 in cognitive memory"), and it is
the counter this feature keys on.

> **Design note — why not the episodic `Signal::RecurringSignature` lane.** An
> earlier draft proposed a *second* counter folded from
> `Signal::RecurringSignature { occurrences }` in the GoalHygiene problem's
> `evidence`. That fold is **unreachable**: `orient()` merges evidence only on an
> exact `dedup_key` match, and a `RecurringSignature`'s key
> (`sanitize_recalled(signature)` = `overseer-obs:goal:blocked:{goal_id}|…`) never
> equals the GoalHygiene key (`goal:blocked:{goal_id}`), so the signal becomes its
> own `ProcessHealth` problem and never reaches this decision. That approach is
> **not** implemented. `RECURRING_SIGNATURE_THRESHOLD` and
> `Signal::RecurringSignature` remain the episodic-detection mechanism used
> elsewhere; they are **not** part of this routing decision.

## `decide_blocked_goal`

The routing function's **signature is unchanged** — the feature adds one branch,
not a parameter:

```rust
/// Route a blocked goal to the right stewardship action.
///
/// Escalation precedence (first match wins):
///  1. General recurrence fast path — `recurrence >= RECURRENCE_ESCALATION_THRESHOLD`
///     (3): any structurally-recurring root cause escalates immediately.
///  2. Dead-band close (#4124) — `perpetual && is_no_progress_marker(&reason)
///     && recurrence >= PERPETUAL_RECURRENCE_ESCALATION_THRESHOLD` (2): a
///     perpetual, no-progress goal whose SAME cause has already recurred at the
///     detection floor is escalated ONCE instead of being re-unblocked forever.
///  3. First/second-time perpetual false-park self-heal —
///     `perpetual && is_no_progress_marker` (reached only when recurrence < 2).
///  4. Needs-human-review escalation.
///  5. Otherwise surface in the periodic Report and leave the block untouched.
fn decide_blocked_goal(
    goal_id: String,
    reason: String,
    perpetual: bool,
    needs_review: bool,
    recurrence: u32,   // goal-scoped RootCause.recurrence (from why), recalled from memory
    why: String,
) -> Intervention;
```

The new branch is inserted **immediately before** the existing
`perpetual && is_no_progress_marker` self-heal branch:

```rust
    if recurrence >= RECURRENCE_ESCALATION_THRESHOLD {          // 3 — unchanged
        return Intervention::EscalateBlockedGoal { goal_id, reason, why };
    }
    // #4124: a perpetual, no-progress re-park loop whose cause has already
    // recurred at the detection floor is NOT re-unblocked again — escalate once.
    if perpetual
        && is_no_progress_marker(&reason)
        && recurrence >= PERPETUAL_RECURRENCE_ESCALATION_THRESHOLD  // 2 — NEW
    {
        return Intervention::EscalateBlockedGoal { goal_id, reason, why };
    }
    if perpetual && is_no_progress_marker(&reason) {           // now only recurrence < 2
        return Intervention::UnblockGoal { goal_id, reason };
    }
    if needs_review {
        return Intervention::EscalateBlockedGoal { goal_id, reason, why };
    }
    Intervention::Report
```

### Parameters (unchanged)

| Parameter | Source | Meaning |
| --------- | ------ | ------- |
| `goal_id` | `Signal::GoalBlocked.goal_id` | The blocked goal being routed. |
| `reason` | `Signal::GoalBlocked.reason` | Operator-facing block reason. |
| `perpetual` | `Signal::GoalBlocked.perpetual` | Whether the goal is a perpetual (`#2589/#2609`) goal. |
| `needs_review` | `Signal::GoalBlocked.needs_review` | Whether the block carries a "needs human review" marker. |
| `recurrence` | `problem.why.recurrence` | Goal-scoped recalled same-cause recurrences (from cognitive memory). |
| `why` | `problem.why.to_string()` | One-line root-cause WHY carried into escalation. |

When no cognitive memory is attached, `recurrence` folds to `0`, so both the
fast path and the new dead-band branch conservatively do **not** fire — an
unknown recurrence never triggers a premature escalation; the goal is handled
exactly as before (self-heal / report).

### Decision table

Given `PERPETUAL_RECURRENCE_ESCALATION_THRESHOLD = 2` and
`RECURRENCE_ESCALATION_THRESHOLD = 3`:

| `recurrence` | `perpetual` + no-progress | `needs_review` | Result | Branch |
| ------------ | ------------------------- | -------------- | ------ | ------ |
| `>= 3` | any | any | `EscalateBlockedGoal` | 1 (general fast path) |
| `2` | yes | any | `EscalateBlockedGoal` | 2 (dead-band close, **NEW**) |
| `2` | no | yes | `EscalateBlockedGoal` | 4 |
| `2` | no | no | `Report` | 5 |
| `< 2` | yes | any | `UnblockGoal` | 3 (self-heal, unchanged) |
| `< 2` | no | yes | `EscalateBlockedGoal` | 4 |
| `< 2` | no | no | `Report` | 5 |

Branch **2** ordering is load-bearing: it is placed **before** branch 3. If it
came after, a perpetual, no-progress goal at `recurrence == 2` would be
re-unblocked (branch 3) and the dead-band would persist. All comparisons use
`>=` on `u32` with no arithmetic, so no overflow or wrap is possible.

## Call site (GoalHygiene routing)

`decide_blocked_goal` is invoked from the `ProblemKind::GoalHygiene` arm of the
intervention planner in `mod.rs`. The call site is **unchanged** — it already
derives `recurrence` from the problem's WHY and passes it through:

```rust
let recurrence = problem.why.as_ref().map(|w| w.recurrence).unwrap_or(0);
let why = problem.why.as_ref().map(|w| w.to_string()).unwrap_or_default();

return decide_blocked_goal(
    goal_id,
    reason,
    perpetual,
    needs_review,
    recurrence,
    why,
);
```

No new memory recall and no new store access are introduced — the reachable
goal-scoped `recurrence` the planner already computes is what the new branch
keys on.

## Escalation output (unchanged, confirmed)

Branches 1 and 2 both return the existing variant — no new fields:

```rust
Intervention::EscalateBlockedGoal {
    goal_id,
    reason,
    why,   // one-line root-cause WHY, names the missing dependency
}
```

Acting on it routes through the unchanged `Overseer::act_escalate_blocked_goal`,
which:

1. Builds `OperatorNotification::goal_blocked_with_why(goal_id, reason, why)` so a
   human receives the root-cause analysis — including the **named missing
   dependency** (e.g. `agent-kgpacks-rs` issue-17 WS2 int8/PQ embed) — not just a
   bare symptom.
2. Deduplicates on the per-goal signature `escalate:{goal_id}` through the
   existing `blocked_goal_gate` (`WhisperGate::new(900, 20)`). Repeated escalation
   of the same `goal_id` within the window collapses to a **single** notification,
   so the newly reachable escalation path cannot produce notification (or SMTP)
   amplification.

The gate slot is committed only **after** a dispatch attempt; the notifier queues
or logs on delivery failure and never drops.

## Loop-termination guarantee

For a perpetual, no-progress goal whose root cause keeps re-parking with the same
goal-scoped signature:

- **Before:** at `recurrence == 0`, `1`, and `2` the goal took the self-heal
  branch (`UnblockGoal`), re-parked, and re-emitted the identical signature — an
  unbounded loop until `recurrence` finally reached `3` (branch 1).
- **After:** the goal still self-heals at `recurrence == 0` and `1` (each
  self-heal records an occurrence, so the next recall raises `recurrence`), but
  the **third** re-park at `recurrence == 2` takes branch 2 and escalates once
  through `blocked_goal_gate`, surfacing one operator blocker report. The
  signature is **not** re-emitted as a fresh un-escalated park, so the loop
  terminates no later than the third re-park — one cycle earlier than the general
  fast path, and always bounded by branch 1 at `recurrence == 3`.

## Constants

```rust
// root_cause.rs — general recurrence escalation floor (any recurring cause). Unchanged.
pub const RECURRENCE_ESCALATION_THRESHOLD: u32 = 3;

// root_cause.rs — NEW: lower escalation floor for a PERPETUAL, no-progress re-park
// loop, closing the [2,3) dead-band. Deliberately equals the detection floor of 2.
pub const PERPETUAL_RECURRENCE_ESCALATION_THRESHOLD: u32 = 2;
```

The episodic-detection constant `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs`)
is **unrelated** to this routing decision and is left untouched. Both thresholds
are compile-time constants, intentionally **not** env-tunable, to avoid config
sprawl.

## Tests

Hermetic — existing fakes; no network, no `~/.simard`.

`tests_goal_health.rs`:

- **T1** — perpetual no-progress goal, `recurrence = 2` ⇒ `EscalateBlockedGoal`
  (dead-band closed; **not** `UnblockGoal`). Exercises the new branch 2.
- **T2** — perpetual no-progress goal, `recurrence = 1` ⇒ `UnblockGoal`
  (still below the dead-band floor; self-heal unchanged).
- **T3** — perpetual no-progress goal, `recurrence = 0` ⇒ `UnblockGoal`
  (first park; self-heal unchanged).
- **T4** — `recurrence = 3` ⇒ `EscalateBlockedGoal` via the general fast path
  (upper bound, unchanged).
- **T5** — repeated escalation for the same `goal_id` deduped by
  `blocked_goal_gate` to a single notification.

`tests_root_cause.rs`:

- **T6** — the escalation `why` / report body contains the named missing
  dependency and reason, and no secret-shaped substrings.

Run the targeted subset:

```bash
cargo test -p simard overseer::tests_goal_health
cargo test -p simard overseer::tests_root_cause
```
