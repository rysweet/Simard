---
title: No-progress breaker API reference
description: Reference for the OODA no-progress breaker — the per-goal consecutive-no-action safeguard, its sentinel `[OODA-SAFEGUARD]` Blocked marker, the standing/perpetual-goal runtime exemption, the `perpetual_idled` report field, and the load-time `heal_stale_no_progress_blocks` self-heal (#2589).
last_updated: 2026-07-05
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/perpetual-goal-no-progress-exemption.md
  - ../concepts/no-progress-root-cause-resolution.md
  - ../concepts/steerable-ooda-daemon.md
  - ../reference/no-progress-root-cause-resolution-api.md
  - ../reference/ooda-no-progress-why-recipe.md
  - ../reference/completion-evidence-gate-api.md
  - ../reference/goal-board-api.md
  - ../howto/unblock-stuck-ooda-goals.md
  - ../howto/diagnose-a-no-progress-block.md
  - ../../src/goal_curation/no_progress_breaker.rs
  - ../../src/ooda_loop/no_progress.rs
  - ../../src/goal_board_store/mod.rs
  - ../../src/goal_curation/types.rs
---

# No-progress breaker API reference

> **Status: implemented.** The breaker constants, the sentinel marker helpers,
> and the per-goal counter live in
> [`src/goal_curation/no_progress_breaker.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/no_progress_breaker.rs).
> The cycle driver and its report live in
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs).
> The load-time self-heal lives in
> [`src/goal_board_store/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_board_store/mod.rs).
> The standing/perpetual flag it reuses is
> [`ActiveGoal::is_perpetual()`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/types.rs).

> **`perpetual_idled` is now scoped to NON-research standing goals (#4399).** The
> `perpetual_idled` exemption below is unchanged for standing goals whose charter
> genuinely permits bursty idling. For the standing **research** goal, an idle is a
> **fault**: the shared `classify_standing_idle` classifier routes it to the new
> `research_idle_faults` report field and re-orients the goal instead of exempting
> it. See the
> [never-idle rail API reference](./research-goal-never-idle-rail-api.md) and
> [concept](../concepts/research-goal-never-idle.md).

This reference specifies the API of the no-progress breaker and the
standing/perpetual-goal exemption added in issue #2589. For the rationale, see
[Standing/perpetual goals are exempt from the no-progress hard-block](../concepts/perpetual-goal-no-progress-exemption.md).

## Contents

- [Threshold and sentinel](#threshold-and-sentinel)
- [Marker helpers](#marker-helpers)
- [`NoProgressTracker`](#noprogresstracker)
- [`NoProgressResolution`](#noprogressresolution)
- [Cycle driver: `apply_no_progress_breaker`](#cycle-driver-apply_no_progress_breaker)
- [`NoProgressBreakerReport`](#noprogressbreakerreport)
- [Standing/perpetual runtime exemption](#standingperpetual-runtime-exemption)
- [Load-time self-heal: `heal_stale_no_progress_blocks`](#load-time-self-heal-heal_stale_no_progress_blocks)
- [Daemon hydration wiring](#daemon-hydration-wiring)
- [What is unchanged](#what-is-unchanged)

## Threshold and sentinel

```rust
/// Consecutive no-action cycles on one goal before the breaker fires.
/// Deliberately small (2–3) so a livelock is broken quickly.
pub const NO_PROGRESS_BREAKER_THRESHOLD: u32 = 3;

/// Sentinel prefix for a breaker-authored `GoalProgress::Blocked` reason
/// (`U+1F512` lock + `[OODA-SAFEGUARD]` token).
pub const NO_PROGRESS_BLOCKED_PREFIX: &str =
    "\u{1F512} [OODA-SAFEGUARD] OODA goal made no shippable progress for ";

/// Sentinel suffix. A full reason renders as `{PREFIX}{count}{SUFFIX}`.
pub const NO_PROGRESS_BLOCKED_SUFFIX: &str =
    " consecutive no-action cycles; needs human review";
```

## Marker helpers

```rust
/// True when `reason` was authored by the no-progress breaker. Keys on the
/// `NO_PROGRESS_BLOCKED_PREFIX` sentinel ALONE (issue #16), so it recognises both
/// the legacy `{PREFIX}{count}{SUFFIX}` reason and the WHY-bearing
/// `{PREFIX}{count} … why=<TOKEN> evidence=[…]` reason. Distinct from the
/// brain-failure marker and from operator/scope/dependency block reasons.
pub fn is_no_progress_marker(reason: &str) -> bool;

/// Render the legacy sentinel Blocked reason for a goal escalated after
/// `consecutive` no-action cycles. The root-cause upgrade (issue #16) authors the
/// richer `no_progress_blocked_reason_with_why` instead; see the
/// [root-cause resolution API](./no-progress-root-cause-resolution-api.md).
pub fn no_progress_blocked_reason(consecutive: u32) -> String;
```

`is_no_progress_marker` is the **single predicate** that both the runtime
exemption and the load-time self-heal use to recognise a safeguard-authored
block and distinguish it from every other kind of `Blocked`.

## `NoProgressTracker`

The per-goal consecutive-no-action counter that drives the breaker. It is **not**
passed to the breaker as a parameter — it lives on `OodaState::no_progress_tracker`
and is `Serialize`/`Deserialize`, so the counter survives the daemon's periodic
exec-reload restarts (a livelock spanning a restart must still be bounded). During
a breaker pass the driver detaches it from `state` with `std::mem::take` — so the
done-gate closure can borrow the board immutably while the tracker mutates — and
restores it before returning.

```rust
/// Per-goal consecutive no-action counter. Lives on `OodaState`; persisted with
/// the goal board so a livelock spanning a restart is still bounded.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct NoProgressTracker { /* counts: HashMap<String, u32> */ }

impl NoProgressTracker {
    /// Current consecutive no-action count for `goal_id` (`0` when untracked).
    pub fn consecutive(&self, goal_id: &str) -> u32;

    /// Record forward progress on `goal_id`, resetting its consecutive
    /// no-action count to `0`. This is the primitive the perpetual exemption
    /// calls to keep a standing goal from ever climbing to the threshold.
    pub fn record_progress(&mut self, goal_id: &str);

    /// Record a no-action cycle and return the breaker's resolution, evaluating
    /// `disposition` (the done-gate) lazily only once the count hits `threshold`.
    /// Clears the counter on any terminal resolution.
    pub fn record_and_resolve(
        &mut self,
        goal_id: &str,
        threshold: u32,
        disposition: impl FnOnce() -> StuckGoalDisposition,
    ) -> NoProgressResolution;
}
```

## `NoProgressResolution`

The disposition the breaker computes for a stuck goal at the threshold (via the
done-gate). Each terminal variant carries the payload its side effect needs.

> **Extended by the root-cause-resolution upgrade (issue #16, implemented).**
> The four variants below are the *base* ladder. The upgrade first classifies
> **why** a goal stalled and adds self-resolving rungs — `Heal { why }`,
> `Defer { blocking_ref, evidence }`, `SpawnEngineer { task, why }` — reaching
> `Escalate` (WHY-bearing) only as a last resort. See the
> [root-cause resolution API reference](./no-progress-root-cause-resolution-api.md#extended-noprogressresolution)
> for the extended enum and the
> [concept](../concepts/no-progress-root-cause-resolution.md) for the ladder.

```rust
pub enum NoProgressResolution {
    /// Below the threshold — record the no-op and let the goal retry next cycle.
    Continue,
    /// Threshold reached with evidence present — done-gate certified complete.
    MarkDone,
    /// Threshold reached and obsolete — drop it, carrying the human-readable
    /// reason.
    Drop { reason: String },
    /// Threshold reached and unresolved — hard-block the goal with the sentinel
    /// `blocked_reason` and file a review issue from `issue_title`/`issue_body`.
    Escalate {
        blocked_reason: String,
        issue_title: String,
        issue_body: String,
    },
}

impl NoProgressResolution {
    /// `true` for every variant except `Continue` — the breaker fired and the
    /// goal has left the no-action loop (so `record_and_resolve` clears its
    /// counter).
    pub fn is_terminal(&self) -> bool;
}
```

A standing/perpetual goal **never reaches** a resolution — it is exempted before
`record_and_resolve` runs (see below), so `Escalate` can never be produced for
it.

## Cycle driver: `apply_no_progress_breaker`

```rust
/// Drive the no-progress breaker over one cycle's outcomes at the default
/// `NO_PROGRESS_BREAKER_THRESHOLD`. Returns a report of what fired.
pub(crate) fn apply_no_progress_breaker(
    state: &mut OodaState,
    outcomes: &[ActionOutcome],
    evidence: &dyn EvidenceSource,
    filer: &dyn NoProgressIssueFiler,
) -> NoProgressBreakerReport;
```

There is **no** `tracker` parameter: the counter state lives on
`OodaState::no_progress_tracker` (see above). The driver detaches it internally
with `std::mem::take(&mut state.no_progress_tracker)` for the duration of the
pass and restores it before returning. `evidence` is the injected
`EvidenceSource` the done-gate consults; `filer` is the injected
`NoProgressIssueFiler` that files the tracking issue on escalation.

Tests call the threshold-parameterised form,
`apply_no_progress_breaker_with_threshold(state, outcomes, evidence, filer, threshold)`,
and inject fakes (a canned `EvidenceSource` and a recording
`NoProgressIssueFiler`) so the breaker runs hermetically with no network and no
live `gh`.

## `NoProgressBreakerReport`

```rust
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct NoProgressBreakerReport {
    /// Goals the done-gate certified complete at threshold.
    pub marked_done: Vec<String>,
    /// Goals dropped as obsolete at threshold.
    pub dropped: Vec<String>,
    /// Goals hard-blocked with the `[OODA-SAFEGUARD]` sentinel and escalated
    /// for human review.
    pub escalated: Vec<String>,
    /// NEW (#2589): standing/perpetual goals that idled this cycle. Their
    /// counters were reset and they were kept active — an idle is NORMAL for a
    /// bursty goal, not a fault, so this list is informational only.
    ///
    /// SCOPED by #4399: this now holds only NON-research standing goals. A
    /// standing RESEARCH goal that idles is routed instead to
    /// `research_idle_faults` (a fault → re-orient); see the
    /// [never-idle rail API](./research-goal-never-idle-rail-api.md).
    pub perpetual_idled: Vec<String>,
}

impl NoProgressBreakerReport {
    /// True when the breaker took a *disruptive* action this cycle. Keys ONLY
    /// on `marked_done` / `dropped` / `escalated`. `perpetual_idled` does NOT
    /// count as a firing, so a cycle whose only breaker activity was a standing
    /// goal idling still reports `fired() == false`.
    pub fn fired(&self) -> bool;

    /// Compact one-line cycle-log summary. Extended by #2589 to carry the idle
    /// count:
    ///
    /// ```text
    /// done={n} dropped={n} escalated={n} idled={n}
    /// ```
    pub fn log_line(&self) -> String;
}
```

`perpetual_idled` is additive and default-derived; it never affects `fired()`.
It exists as an explicit assertion hook for tests and for observability.

**Where the summary line is emitted (caveat).** The cycle driver logs
`log_line()` **only when `fired()` is true** (i.e. on a disruptive action). A
cycle whose *only* breaker activity is a standing goal idling therefore emits
**no** summary line — the `idled={n}` field is visible only in cycles that also
marked/dropped/escalated some other goal. The authoritative, always-present
signal for a standing-goal idle is the per-goal `tracing::info!` emitted by the
exemption (below) plus the `perpetual_idled` entry on the returned report.

## Standing/perpetual runtime exemption

Inside the per-outcome loop of `apply_no_progress_breaker_with_threshold`, after
a cycle is confirmed to be a no-action outcome and **before** `record_and_resolve`
computes the resolution, the breaker checks the goal's standing flag. `tracker`
here is the local the driver detached from `state.no_progress_tracker` with
`std::mem::take`, so resetting it mutates the same counter that is restored onto
`state` at the end of the pass:

```rust
// Perpetual/standing goals (#2580/#2589) are inherently bursty: they ship
// durable improvements periodically and idle between. An idle cycle is NORMAL,
// not a livelock, so they must never be hard-blocked by the no-progress
// safeguard. Reset the counter, keep the goal active, let the next cycle
// re-select it. Reuses the SAME is_perpetual() flag as the completion gate.
if state
    .active_goals
    .active
    .iter()
    .any(|g| g.id == goal_id && g.is_perpetual())
{
    tracker.record_progress(goal_id); // reset → consecutive == 0
    report.perpetual_idled.push(goal_id.to_string());
    tracing::info!(
        target: "simard::ooda",
        goal = %goal_id,
        "no-progress breaker: standing/perpetual goal idled this cycle — normal \
         for a bursty goal; counter reset, goal stays active (not blocked)",
    );
    continue;
}
```

Semantics:

- The exemption sits **before** `record_and_resolve`, so the counter never
  climbs toward the threshold and the `MarkDone` / `Drop` / `Escalate` match is
  never entered for a standing goal.
- The goal's `status` is left **untouched** (e.g. `NotStarted` / `InProgress` as
  it was) — only the counter is reset.
- Non-perpetual goals skip this block entirely and hit the existing
  escalation path unchanged.

## Load-time self-heal: `heal_stale_no_progress_blocks`

A pure, by-value board transform that clears stale safeguard blocks left on
standing goals by an older daemon build. It follows the same idiom as the
sibling [`filter_tombstoned`](./goal-board-api.md).

```rust
/// Self-heal perpetual/standing goals a prior daemon version parked with the
/// no-progress safeguard sentinel. A standing goal is bursty by design and must
/// never carry a `[OODA-SAFEGUARD] … needs human review` block, so on load we
/// clear any such stale marker back to `NotStarted` (the canonical
/// re-dispatchable state used by `roll_to_new_cycle`). Normal goals — and blocks
/// authored by any OTHER path (operator, scope, dependency, brain-failure) — are
/// left exactly as-is. In-memory only; persisted naturally by the next
/// `commit_cycle`. No load-time `~/.simard` write.
pub fn heal_stale_no_progress_blocks(board: GoalBoard) -> GoalBoard;
```

Behaviour, per active goal:

| Goal | Status | Result |
| --- | --- | --- |
| perpetual | `Blocked(r)` where `is_no_progress_marker(r)` | → `NotStarted` (healed) |
| perpetual | `Blocked(r)` where `!is_no_progress_marker(r)` (operator/scope/dependency) | unchanged |
| perpetual | not `Blocked` | unchanged |
| non-perpetual | `Blocked(r)` where `is_no_progress_marker(r)` | unchanged (still blocked) |

The pass is **idempotent**: a second call is a no-op because healed goals are no
longer `Blocked`.

## Daemon hydration wiring

The heal runs at **two** sites in the OODA daemon, both **after** tombstoned
goals are filtered (so a tombstoned goal is never resurrected). The per-cycle
site is the one that actually un-parks a goal; the startup site is a harmless,
idempotent early pass.

### 1. Startup hydration — once, before the loop (`daemon/mod.rs`)

```rust
let board = crate::goal_board_store::filter_tombstoned(persistent.board, &tombstones);
let board = crate::goal_board_store::heal_stale_no_progress_blocks(board);
let mut state = OodaState::new(board);
state.no_progress_tracker = persistent.no_progress;
```

### 2. Per-cycle re-sync — every cycle, inside the loop (`daemon/mod.rs`) — REQUIRED

The daemon reloads the authoritative `goal_board.json` from disk at the top of
**every** cycle, so a startup-only heal is undone on the first iteration (disk
still says `Blocked`). The heal must therefore also run at the per-cycle
re-sync, **before** `overwrite_memory_cache` so the healed board reaches the
snapshot cache that `run_ooda_cycle` reads:

```rust
let cycle_tombstones = crate::ooda_loop::load_tombstones(&state_root);
let persistent = crate::goal_board_store::load(&state_root);
state.active_goals = crate::goal_board_store::heal_stale_no_progress_blocks(
    crate::goal_board_store::filter_tombstoned(persistent.board, &cycle_tombstones),
);
state.no_progress_tracker = persistent.no_progress;
// overwrite_memory_cache(&state.active_goals, …) now sees the healed board
```

**Why both sites are required.** A goal parked by an older build is `Blocked`
on disk. Without the per-cycle heal, the trace defeats itself: iteration 1
reloads `Blocked` from disk → a `Blocked` goal is never dispatched → it produces
no outcome → the runtime exemption never fires → `commit_cycle` re-persists
`Blocked`. The goal stays parked forever and the startup-only heal is a no-op.
Healing at the per-cycle re-sync clears the marker in memory each cycle; once
the goal is dispatchable (`NotStarted`) it is re-selected, ships or idles, and
the next `commit_cycle` persists the cleared status. The pass is idempotent, so
keeping the startup call too is harmless.

Because the heal is in-memory, neither site writes to `~/.simard`; the cleared
status is persisted by the next `commit_cycle`.

## What is unchanged

- `NO_PROGRESS_BREAKER_THRESHOLD`, both sentinel constants, `is_no_progress_marker`,
  and `no_progress_blocked_reason` — unchanged.
- The `NoProgressResolution` control flow and the escalation path for **normal**
  goals — byte-for-byte unchanged.
- `ActiveGoal::is_perpetual()` and `description_marks_standing()` — reused, not
  modified. There is exactly one standing/perpetual notion across the completion
  gate (#2580/#2589) and this breaker exemption.
- `simard goal unblock` / `simard goal unblock-all` — still available for the
  (now rare) manual cases; see the
  [runbook](../howto/unblock-stuck-ooda-goals.md).
- Re-investigation of **already-blocked** bare goals (#17) — the safeguard now
  re-runs the WHY reasoner over goals already parked with a bare
  `[OODA-SAFEGUARD] … needs human review` marker, not just goals crossing the
  threshold. See the
  [no-progress re-investigation API](../reference/no-progress-reinvestigation-api.md).

## See also

- [Concept: a goal with an open, mergeable PR is awaiting merge — never reaped](../concepts/no-progress-awaiting-merge-exemption.md) — the #4441 awaiting-merge branch that idles a completed-but-unmerged goal instead of reaping it.
- [No-progress awaiting-merge API reference](../reference/no-progress-awaiting-merge-api.md) — `EvidenceSource::open_mergeable_pr`, `StuckGoalDisposition::AwaitingMerge`, the non-terminal `NoProgressResolution::AwaitMerge`, and the `awaiting_merge` report field.
- [Concept: the breaker explains WHY and self-resolves before escalating](../concepts/no-progress-root-cause-resolution.md) — the root-cause classification and resolution ladder layered on this base breaker.
- [Root-cause resolution API reference](../reference/no-progress-root-cause-resolution-api.md) — `NoProgressClass`, the WHY types, the reasoner, and the extended resolution ladder.
- [The `ooda-no-progress-why` recipe reference](../reference/ooda-no-progress-why-recipe.md) — the optional agentic WHY narrator.
- [Concept: re-investigating already-blocked OODA goals](../concepts/ooda-reinvestigate-blocked-goals.md) — the #17 pass that upgrades bare blocks to a concrete WHY.
- [No-progress re-investigation API reference](../reference/no-progress-reinvestigation-api.md) — the rail, dedupe set, and re-investigation pass.
- [Concept: standing/perpetual goals are exempt from the no-progress hard-block](../concepts/perpetual-goal-no-progress-exemption.md)
- [Completion-evidence gate API](../reference/completion-evidence-gate-api.md) — the sibling gate that makes standing goals non-completable.
- [Goal board API reference](../reference/goal-board-api.md) — `filter_tombstoned` and load/save semantics.
- [Diagnose a no-progress block and read its WHY](../howto/diagnose-a-no-progress-block.md)
- [Unblock OODA goals stuck after a safeguard lockout](../howto/unblock-stuck-ooda-goals.md)
