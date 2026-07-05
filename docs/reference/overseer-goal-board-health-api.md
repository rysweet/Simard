---
title: "Overseer goal-board health API"
description: >
  Public surface of the Overseer's goal-board health handling: the BlockedGoal
  observation and ObservedState.blocked_goals field, the read-only
  GoalCurator::blocked_goals and mutating GoalCurator::unblock capabilities, the pure
  sensor::blocked_goals_from_board projection, the Signal::GoalBlocked signal and its
  ProblemKind::GoalHygiene classification, the decide_blocked_goal router, the
  Intervention::UnblockGoal / EscalateBlockedGoal actions, the ActOutcome variants, the
  OperatorNotification::goal_blocked constructor and OperatorNotifier seam, the
  SIMARD_OVERSEER_GOAL_HEALTH flag and goal_health_enabled resolver, and the extended
  OverseerTickReport / OverseerTotals counters.
last_updated: 2026-07-05
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/overseer-goal-board-health.md
  - ../howto/configure-overseer-goal-board-health.md
  - ../design/overseer.md
  - ../reference/overseer-activity-feed.md
  - ../reference/no-progress-breaker-api.md
---

# Overseer goal-board health API reference

> **Status: implemented** (issue
> [#2609](https://github.com/rysweet/Simard/issues/2609), merged in
> [#2616](https://github.com/rysweet/Simard/pull/2616)). Every type, trait,
> function, and field below ships in `src/overseer/`.

Module: `simard::overseer`.
Primary sources: additive edits to `capabilities.rs`, `sensor.rs`, `signal.rs`,
`intervention.rs`, `guardrails.rs`, `config.rs`, `notify.rs`, `activity.rs`,
`wiring.rs`, and `mod.rs`.

For the conceptual overview see
[Overseer goal-board health](../concepts/overseer-goal-board-health.md); for
operator configuration see
[Configure and observe Overseer goal-board health](../howto/configure-overseer-goal-board-health.md).

Goal-board health is **purely additive**: it introduces new struct/enum variants,
two new capability methods (with defaulted bodies), and new report/notification
members. No existing type, function, or field is renamed or removed, and every
existing Overseer test keeps passing unchanged.

## Change map

```
src/overseer/capabilities.rs   + struct BlockedGoal
                               + ObservedState.blocked_goals: Vec<BlockedGoal>
                               + GoalCurator::blocked_goals() (defaulted, read-only)
                               + GoalCurator::unblock(goal_id) (defaulted no-op)
src/overseer/sensor.rs         + blocked_goals_from_board(&GoalBoard) -> Vec<BlockedGoal>
                               + blocked_goal_of / safeguard_marker_count (private)
src/overseer/signal.rs         + Signal::GoalBlocked{ … } + signals_from arm
src/overseer/intervention.rs   + Intervention::{UnblockGoal, EscalateBlockedGoal} + label()
src/overseer/guardrails.rs     + classify() arm (both ⇒ RiskClass::Routine)
src/overseer/config.rs         + SIMARD_OVERSEER_GOAL_HEALTH_ENV
                               + goal_health_enabled_from() / goal_health_enabled()
src/overseer/notify.rs         + OperatorNotification::goal_blocked(id, reason)
                               + trait OperatorNotifier (+ impl for DualChannelNotifier)
src/overseer/activity.rs       + OverseerTotals.{goals_unblocked, goals_escalated}
                               + humanize_tick arms; interventions() includes them
src/overseer/wiring.rs         + OverseerTickReport.{goals_unblocked, goals_escalated,
                                 goals_health_suppressed}; tally_outcome arms;
                               + BoardGoalCurator::{blocked_goals, unblock};
                               + build_overseer wires goal-health + operator notifier
src/overseer/mod.rs            + Overseer.{notifier, blocked_goal_gate, goal_health_enabled}
                               + with_operator_notifier / with_goal_health_enabled
                               + ActOutcome::{GoalUnblocked, GoalEscalated,
                                 GoalHealthSuppressed}
                               + act_unblock_goal / act_escalate_blocked_goal /
                                 try_whisper_carve_subgoal
                               + classify_signal + decide arm + decide_blocked_goal
src/operator_commands_ooda/daemon/mod.rs  tick log line + goals_unblocked/goals_escalated
src/overseer/tests_goal_health.rs  NEW: 10 tests (fakes only, no network)
```

## `BlockedGoal`

```rust
// src/overseer/capabilities.rs
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockedGoal {
    /// The blocked goal's id on the active board.
    pub id: String,
    /// The verbatim `GoalProgress::Blocked` reason string (carries the marker).
    pub reason: String,
    /// True when the goal is standing/perpetual (`ActiveGoal::is_perpetual`).
    pub perpetual: bool,
    /// True when the block carries a "needs human review" safeguard marker
    /// (the no-progress OODA-SAFEGUARD marker or the brain-failure marker).
    pub needs_review: bool,
    /// Consecutive no-action / no-progress cycles parsed from the safeguard
    /// marker, or `0` when the block is not a counted safeguard marker.
    pub consecutive_no_action: u32,
}
```

One blocked goal observed on Simard's board — the unit of goal-board *health* the
Overseer observes and acts on. It reuses the **existing** perpetual detection
(`ActiveGoal::is_perpetual`, #2589/#2609) and the **existing** safeguard-marker
predicates; it never invents a second notion of either.

## `ObservedState.blocked_goals`

```rust
// src/overseer/capabilities.rs — added to ObservedState
/// Blocked goals observed on Simard's goal board this Observe pass — the
/// goal-board *health* signal. Empty when the board is clean or unreadable
/// (degrade-to-empty, never a panic).
pub blocked_goals: Vec<BlockedGoal>,
```

The acting Overseer's `run_cycle` populates this field via
`self.caps.goals.blocked_goals().unwrap_or_default()`. The read-only
`observed_from_snapshot` adapter leaves it empty (the status snapshot does not
carry the board), keeping that projection side-effect free.

## `GoalCurator` — new capability methods

```rust
// src/overseer/capabilities.rs — added to trait GoalCurator
/// Read the goal-board *health* — the goals currently `Blocked`. Read-only; a
/// board read failure degrades to an empty list. Defaulted to `Ok(vec![])`.
fn blocked_goals(&self) -> Result<Vec<BlockedGoal>, OverseerError> { Ok(Vec::new()) }

/// Auto-unblock + reactivate a false-parked goal — the exact `simard goal
/// unblock` operation: restore a `Blocked` goal to `NotStarted`. Defaulted to a
/// no-op for fakes that do not model a board.
fn unblock(&self, _goal_id: &str) -> Result<(), OverseerError> { Ok(()) }
```

Both have **defaulted bodies** so existing `GoalCurator` fakes compile unchanged.
The production adapter overrides them (see [Wiring](#wiring)).

## `sensor::blocked_goals_from_board`

```rust
// src/overseer/sensor.rs
pub fn blocked_goals_from_board(board: &GoalBoard) -> Vec<BlockedGoal>;
```

Pure, read-only projection: one `BlockedGoal` per active goal in a
`GoalProgress::Blocked(reason)` state. For each it sets:

- `perpetual` ← `ActiveGoal::is_perpetual()`;
- `needs_review` ← `is_no_progress_marker(reason) || is_brain_failure_marker(reason)`;
- `consecutive_no_action` ← the leading count parsed from the safeguard marker's
  `{prefix}{n}{suffix}` shape (`0` for a non-safeguard block).

The count parser strips whichever marker prefix matches
(`NO_PROGRESS_BLOCKED_PREFIX` or `BRAIN_FAILURE_BLOCKED_PREFIX`) and reads the
leading digits. See the
[no-progress breaker API](../reference/no-progress-breaker-api.md) for the marker
constants.

## `Signal::GoalBlocked` and classification

```rust
// src/overseer/signal.rs
pub enum Signal {
    // …existing variants…
    GoalBlocked {
        goal_id: String,
        reason: String,
        perpetual: bool,
        needs_review: bool,
        consecutive_no_action: u32,
    },
}
```

`signals_from` emits one `GoalBlocked` per `ObservedState.blocked_goals` entry.
`classify_signal` maps it to the **existing** `ProblemKind::GoalHygiene` with
`Priority::High` when `needs_review`, else `Priority::Normal`, and a dedup key of
`goal:blocked:{goal_id}`. `observer::signal_kind_label` returns `"GoalBlocked"`.

## `Intervention::UnblockGoal` / `EscalateBlockedGoal`

```rust
// src/overseer/intervention.rs
pub enum Intervention {
    // …existing variants…
    /// SELF-HEAL a false-parked standing/perpetual goal: auto-unblock +
    /// reactivate (the `simard goal unblock` operation).
    UnblockGoal { goal_id: String, reason: String },
    /// ESCALATE a genuinely-blocked "needs human review" goal to the operator.
    EscalateBlockedGoal { goal_id: String, reason: String },
}
```

`Intervention::label()` returns `"unblock_goal"` / `"escalate_blocked_goal"`.
`guardrails::classify` returns `RiskClass::Routine` for both (routine
stewardship, no LLM budget), so the default autonomy gate admits them.

## `decide_blocked_goal`

```rust
// src/overseer/mod.rs
fn decide_blocked_goal(
    goal_id: String, reason: String, perpetual: bool, needs_review: bool,
) -> Intervention {
    if perpetual && is_no_progress_marker(&reason) {
        return Intervention::UnblockGoal { goal_id, reason };   // false park ⇒ self-heal
    }
    if needs_review {
        return Intervention::EscalateBlockedGoal { goal_id, reason }; // genuine ⇒ escalate
    }
    Intervention::Report                                         // deliberate block ⇒ report
}
```

`decide` calls this from the `ProblemKind::GoalHygiene` arm when the problem's
evidence contains a `Signal::GoalBlocked`; a `GoalHygiene` problem **without**
`GoalBlocked` evidence keeps the pre-existing `TransferGoal`-to-Simard behaviour.
The routing reuses `is_no_progress_marker` and the `perpetual` flag — it invents
no new notion of "false park" or "genuine block".

## Act path and `ActOutcome`

```rust
// src/overseer/mod.rs
pub enum ActOutcome {
    // …existing variants…
    GoalUnblocked { goal_id: String },
    GoalEscalated { goal_id: String },
    GoalHealthSuppressed { reason: &'static str },  // "duplicate" | "cap_reached"
}
```

- `act_unblock_goal` — fails closed if `RecursionGuard::is_configured()` is false
  (`OverseerError::Recursion`); otherwise consults `blocked_goal_gate` on
  signature `unblock:{goal_id}`, calls `GoalCurator::unblock`, commits the dedup
  slot **after** success, traces on `overseer::goal_health`, then best-effort
  `try_whisper_carve_subgoal`. Returns `GoalUnblocked` or `GoalHealthSuppressed`.
- `act_escalate_blocked_goal` — same fail-closed identity check and dedup on
  signature `escalate:{goal_id}`; requires a wired `notifier`
  (`OverseerError::Capability` otherwise); builds
  `OperatorNotification::goal_blocked` and calls `OperatorNotifier::notify`;
  commits the dedup slot after the dispatch attempt (the notifier never drops).
  Returns `GoalEscalated` or `GoalHealthSuppressed`.
- `try_whisper_carve_subgoal` — best-effort advisory whisper (only when the
  Whisperer is enabled and wired) steering Simard to carve one bounded, shippable
  sub-goal; a whisper error/suppression is ignored (the self-heal already
  succeeded).

The dedup gate is `blocked_goal_gate: WhisperGate::new(900, 20)` — a 15-minute
window and a per-hour cap of 20 — held on the `Overseer` alongside the
`whisper_gate`.

### Overseer builder methods

```rust
// src/overseer/mod.rs
pub fn with_operator_notifier(self, notifier: Box<dyn OperatorNotifier>) -> Self;
pub fn with_goal_health_enabled(self, enabled: bool) -> Self;
```

`goal_health_enabled` defaults to `false` on a bare `Overseer`; the daemon sets
it from `config::goal_health_enabled()`. When `false`, `run_cycle` holds any
`UnblockGoal` / `EscalateBlockedGoal` with the note
`held: goal-board health disabled (SIMARD_OVERSEER_GOAL_HEALTH)`.

## `OperatorNotification::goal_blocked` and `OperatorNotifier`

```rust
// src/overseer/notify.rs
impl OperatorNotification {
    /// kind "goal-blocked"; headline "goal {id} needs human review".
    pub fn goal_blocked(goal_id: &str, reason: &str) -> Self;
}

/// Object-safe seam the acting Overseer notifies the operator through.
pub trait OperatorNotifier: Send + Sync {
    fn notify(&self, notification: &OperatorNotification) -> NotifyReport;
}

impl OperatorNotifier for DualChannelNotifier { /* delegates to inherent notify */ }
```

The seam lets the Overseer hold the mandatory `DualChannelNotifier` (email +
Signal, never-drop) in production while tests inject a recording fake — reusing
the one "notify on both channels, never drop" guarantee rather than adding a
second notification path. The returned `NotifyReport` exposes `dispatched()` and
`all_sent()` (methods, not fields).

## Config — `SIMARD_OVERSEER_GOAL_HEALTH`

```rust
// src/overseer/config.rs
pub const SIMARD_OVERSEER_GOAL_HEALTH_ENV: &str = "SIMARD_OVERSEER_GOAL_HEALTH";

pub fn goal_health_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool;
pub fn goal_health_enabled() -> bool; // reads the real process environment
```

Opt-out semantics with **default ON**: enabled unless
`SIMARD_OVERSEER_GOAL_HEALTH` is an explicit falsey value (`0`/`false`/`no`/`off`),
**and** only while the acting Overseer is enabled — a disabled Overseer
(`overseer_acting_enabled_from` false) forces goal-board health off.

## Visibility — report, totals, feed

```rust
// src/overseer/wiring.rs — OverseerTickReport (per tick)
pub goals_unblocked: usize,
pub goals_escalated: usize,
pub goals_health_suppressed: usize,

// src/overseer/activity.rs — OverseerTotals (rolling window)
pub goals_unblocked: u64,
pub goals_escalated: u64,
```

`tally_outcome` maps `GoalUnblocked → goals_unblocked`, `GoalEscalated →
goals_escalated`, `GoalHealthSuppressed → goals_health_suppressed`.
`OverseerActivity::interventions()` counts `goals_unblocked + goals_escalated`
(suppressed and held are excluded). `humanize_tick` renders *"self-healed N
blocked goal(s)"* and *"escalated N blocked goal(s) for human review"*. The
daemon tick log line includes `goals_unblocked=` and `goals_escalated=`. All of
this flows into the durable
[Overseer activity feed](../reference/overseer-activity-feed.md).

## Wiring

```rust
// src/overseer/wiring.rs — BoardGoalCurator
fn blocked_goals(&self) -> Result<Vec<BlockedGoal>, OverseerError> {
    Ok(blocked_goals_from_board(&self.load()?))
}
fn unblock(&self, goal_id: &str) -> Result<(), OverseerError> {
    // load → find active goal → status = GoalProgress::NotStarted → save_goal_board
    // (under the BoardWriteLock flock); errors if the goal id is not on the board.
}
```

`build_overseer` wires goal-board health onto the production Overseer:

```rust
.with_goal_health_enabled(goal_health_enabled())
.with_operator_notifier(Box::new(DualChannelNotifier::from_env()))
```

so it is enabled by default (opt-out) and escalations ride the same email +
Signal notifier the merge path uses.

## Invariants

- **Additive.** New variants/fields/methods only; existing tests unchanged.
- **Reused notions.** `perpetual` ← `is_perpetual`; `needs_review` ←
  `is_no_progress_marker` / `is_brain_failure_marker`. No second notion.
- **Mutual exclusion.** A perpetual + no-progress false park ⇒ `UnblockGoal`
  (never escalated); another `needs_review` block ⇒ `EscalateBlockedGoal`;
  anything else ⇒ `Report`.
- **Self-heal = unblock only.** `Blocked → NotStarted` under the write-lock,
  nothing more.
- **Dedup.** One action per goal per `WhisperGate::new(900, 20)` window/cap; the
  slot is consumed only after success / a dispatch attempt.
- **Fail-closed.** Unconfigured steward identity ⇒ `OverseerError::Recursion`,
  nothing mutated or sent.
- **Never-drop escalation.** Escalation reuses the mandatory dual-channel
  notifier; there is no second path.
- **Isolated.** Failures/panics are caught by the panic-isolated tick and
  reflected in `errors` / `panicked`.

## Test coverage

`src/overseer/tests_goal_health.rs` (10 tests, fakes only, no network):
`blocked_goals_projection_surfaces_perpetual_and_needs_review_goals`,
`run_cycle_populates_observed_blocked_goals_and_emits_signals`,
`goal_blocked_signal_maps_to_a_goal_hygiene_problem`,
`perpetual_no_progress_goal_is_unblocked_once_and_not_escalated`,
`needs_review_goal_escalates_to_operator_on_both_channels`,
`self_heal_and_escalate_fail_closed_without_a_distinct_identity`,
`disabled_goal_health_holds_both_actions`,
`goal_health_enable_flag_is_opt_out_and_off_when_overseer_off`,
`a_failing_unblock_is_isolated_and_the_tick_survives`,
`decide_routes_a_blocked_goal_by_shape`,
`goal_health_interventions_are_routine_and_admitted_by_default_gate`.

Run: `cargo test -p simard overseer::tests_goal_health`.

## See also

- Concept: [Overseer goal-board health](../concepts/overseer-goal-board-health.md)
- How-to: [Configure and observe Overseer goal-board health](../howto/configure-overseer-goal-board-health.md)
- Design: [Overseer — operator/observer co-process](../design/overseer.md)
- Related: [Overseer activity feed](../reference/overseer-activity-feed.md),
  [No-progress breaker API](../reference/no-progress-breaker-api.md)
