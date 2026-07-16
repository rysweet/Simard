---
title: "Overseer root-cause (\"WHY\") API"
description: >
  Public surface of the Overseer's mandatory root-cause principle: the RootCause /
  CauseCandidate / Confidence / Likelihood / CauseSource model and the additive
  Problem.why field in signal.rs, the pure root_cause::analyze analyzer and
  PriorOccurrence / root_cause_signature helpers, the Remediation / RemediationClass
  classification and PlannedIntervention.remediation field plus the why fields on
  Intervention::UnblockGoal / EscalateBlockedGoal, the Overseer::with_memory recall+store
  seam over CognitiveMemoryOps, the decide_blocked_goal recurrence routing, the
  OperatorNotification::goal_blocked_with_why constructor, the OverseerActivityRecord.problem_entries
  ProblemEntry feed rows, and the extended OverseerTickReport / OverseerTotals counters.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
status: design — not yet implemented
related:
  - ../concepts/overseer-root-cause-why.md
  - ../howto/configure-overseer-root-cause-why.md
  - ../design/overseer.md
  - ./overseer-goal-board-health-api.md
  - ./overseer-activity-feed.md
  - ./overseer-recurrence-dead-band-escalation-api.md
  - ../howto/configure-overseer-recurrence-escalation.md
  - ./cognitive-memory-ranked-episodic-recall.md
  - ./no-progress-breaker-api.md
---

# Overseer root-cause ("WHY") API reference

> **Status: design — not yet implemented** (issue
> [#2635](https://github.com/rysweet/Simard/issues/2635)). This reference is the
> **binding contract** for the feature we are about to build: every type, trait,
> function, and field below is what the implementing PR will add to `src/overseer/`.
> Documentation and implementation land in the **same pull request**; until then
> these signatures are the specification, not shipped code.

Module: `simard::overseer`.
Primary sources: a new `root_cause.rs` module and its `tests_root_cause.rs`, plus
purely additive edits to `signal.rs`, `intervention.rs`, `mod.rs`, `notify.rs`,
`activity.rs`, and `wiring.rs`.

For the conceptual overview see
[Overseer root-cause ("WHY") principle](../concepts/overseer-root-cause-why.md);
for operator configuration and verification see
[Configure and observe the Overseer root-cause principle](../howto/configure-overseer-root-cause-why.md).

The root-cause principle is **purely additive**: it introduces one new module,
new struct/enum types, one additive field on `Problem` and on
`PlannedIntervention`, an additive `why` field on the
`Intervention::EscalateBlockedGoal` variant, an additive
`OperatorNotification::goal_blocked_with_why` constructor, and new report / feed
members. No existing type, function, or field is renamed or removed, and every
existing Overseer test keeps passing unchanged.

## Change map

```
src/overseer/signal.rs         + struct RootCause, CauseCandidate
                               + enum Confidence, Likelihood, CauseSource
                               + impl Display for RootCause
                               + Problem.why: Option<RootCause>   (additive; defaults None)
src/overseer/root_cause.rs     NEW: pure analyzer
                               + fn analyze(&Problem, &ObservedState, &[PriorOccurrence]) -> RootCause
                               + struct PriorOccurrence
                               + fn root_cause_signature(&Problem, &CauseCandidate) -> String
                               + const RECURRENCE_ESCALATION_THRESHOLD: u32 = 3
src/overseer/intervention.rs   + enum RemediationClass { RootCause, Acknowledged, SymptomMitigation }
                               + struct Remediation { class, root_cause_addressed, unaddressed_note }
                               + PlannedIntervention.remediation: Remediation
                               + Intervention::UnblockGoal{ …, why: String }
                               + Intervention::EscalateBlockedGoal{ …, why: String }
src/overseer/mod.rs            + Overseer.mem: Option<Arc<dyn CognitiveMemoryOps>>
                               + Overseer::with_memory(Arc<dyn CognitiveMemoryOps>) -> Self
                               + Overseer::recall_occurrences(&str) -> Vec<PriorOccurrence>   (read-only)
                               + Overseer::record_occurrence(&Problem, &str)                (deferred, best-effort)
                               + run_cycle populates Problem.why + PlannedIntervention.remediation
                               + decide_blocked_goal recurrence routing
                               + CycleReport.entries: Vec<ProblemEntry>
src/overseer/notify.rs         + OperatorNotification::goal_blocked_with_why(id, reason, why)  (additive)
src/overseer/activity.rs       + struct ProblemEntry
                               + OverseerActivityRecord.problem_entries: Vec<ProblemEntry>
                               + OverseerTotals.{root_cause_analyses, symptom_mitigations,
                                 root_causes_addressed}
                               + humanize_tick symptom-mitigation summary
src/overseer/wiring.rs         + OverseerTickReport.{root_cause_analyses, symptom_mitigations,
                                 root_causes_addressed}; tally_outcome arms
src/overseer/root_cause.rs +   tests_root_cause.rs (hermetic)
```

## Root-cause data model (`signal.rs`)

All root-cause types are `Eq`-safe by construction (enums, no `f64`) so they can
live on the `Eq` activity-feed record.

### `RootCause`

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootCause {
    /// Ranked cause candidates, highest-likelihood first.
    pub candidates: Vec<CauseCandidate>,
    /// The single human-readable one-line WHY.
    pub primary_rationale: String,
    /// Overall confidence in the primary rationale.
    pub confidence: Confidence,
    /// Whether the WHY came from telemetry, recalled prior occurrences, or both.
    pub source: CauseSource,
    /// Count of prior same-signature occurrences recalled from cognitive memory
    /// (`0` when none, or when memory is unavailable / degraded).
    pub recurrence: u32,
}
```

`impl std::fmt::Display for RootCause` renders the canonical one-line WHY used in
traces, the feed, and notifications:

```
{primary_rationale} (confidence: {confidence}, source: {source}[, seen {recurrence}× before])
```

The trailing `, seen N× before` clause is present only when `recurrence > 0`.

### `CauseCandidate`

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CauseCandidate {
    /// Stable cause label — used in dedup / escalation signatures.
    pub label: String,
    /// Relative likelihood among the candidates.
    pub likelihood: Likelihood,
    /// Rendered signal / telemetry / recall references this candidate rests on.
    pub evidence: Vec<String>,
}
```

### Enums

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Confidence { Low, Medium, High }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Likelihood { Low, Medium, High }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CauseSource { Telemetry, MemoryRecall, Both }
```

`Confidence` and `Likelihood` are ordinal enums (not `f64`) precisely so
`RootCause` — and every struct that embeds it — can derive `Eq`. Each also has
an `impl Display`, used to render the one-line WHY inside `RootCause`'s `Display`.

### `Problem.why`

`Problem` gains one additive field:

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub kind: ProblemKind,
    pub priority: Priority,
    pub dedup_key: String,
    pub summary: String,
    pub evidence: Vec<Signal>,
    /// The structured WHY. `None` only transiently in Orient (pure); ALWAYS
    /// `Some(_)` by the time Decide runs. `Option` keeps every existing
    /// `Problem { .. }` constructor back-compatible (they set `why: None`).
    pub why: Option<RootCause>,
}
```

`Problem` is `Clone + Debug + PartialEq` (not `Copy`), so embedding a `RootCause`
is safe. Orient constructs each `Problem` with `why: None`; `run_cycle` populates
it before Decide. The field is populated for **every** `ProblemKind`, not just
goal problems.

## Analyzer (`root_cause.rs`)

A new, self-contained, **pure and deterministic** module — no I/O, no in-loop LLM
call — so the WHY is fully unit-testable and hermetic (guideline **G3**: it
weighs multiple evidence-linked candidates rather than one brittle heuristic).

### `analyze`

```rust
/// Deterministic, structured multi-candidate synthesis of the WHY for one
/// problem. Combines the problem's evidence signals, the observed telemetry, and
/// any recalled prior occurrences into a ranked `RootCause`. Never fails; an
/// unknown cause becomes a single `unknown-cause` candidate at `Confidence::Low`,
/// `source = Telemetry`.
pub fn analyze(
    problem: &Problem,
    observed: &ObservedState,
    recall: &[PriorOccurrence],
) -> RootCause;
```

Per-`ProblemKind` candidate generation (evidence-linked, reusing existing
predicates — no new heuristic invented):

| `ProblemKind` / signal | Candidate labels (ranked by evidence) |
|---|---|
| `GoalHygiene` / `GoalBlocked` (perpetual + no-progress marker) | `parked-by-no-progress-safeguard` (false park), `not-tagged-perpetual`, `starved-by-higher-priority-work` — primary chosen from `BlockedGoal.{perpetual, needs_review, consecutive_no_action, reason}` via `is_no_progress_marker` |
| `ProcessHealth` / `DistillFailureRate` | `schema/format drift`, `model regression`, `upstream payload change` — weighted by `distill_fail_pct` magnitude |
| `ProcessHealth` / `RestartChurn` | `panic loop`, `resource exhaustion`, `bad deploy` |
| `ResourcePressure` / `BudgetPressure` | `spend spike from parallel launches`, `runaway retry`, `budget mis-set` |
| `QualityRegression` / `CiFailureCluster` | `flaky infra`, `real regression in <repo>`, `dependency break` |
| fallback | single `unknown-cause` candidate, `Confidence::Low`, `source = Telemetry` |

When `recall` is non-empty, candidates whose `label` matches a prior occurrence's
`cause_label` are promoted, `recurrence` is set to the match count, and `source`
becomes `MemoryRecall` (or `Both` when telemetry also supports it).

### `PriorOccurrence`

```rust
/// A read-only projection of one prior occurrence of the same problem signature,
/// recalled from cognitive memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PriorOccurrence {
    pub cause_label: String,
    pub action: String,
    pub outcome: String,
}
```

### `root_cause_signature`

```rust
/// Stable signature for dedup / escalation of a root cause. Combines the
/// problem's `dedup_key` with the primary cause `label` so a filed issue dedups
/// on the ROOT CAUSE (across symptom recurrences), mirroring
/// `crate::stewardship::failure_signature` semantics.
pub fn root_cause_signature(problem: &Problem, primary: &CauseCandidate) -> String;
```

### `RECURRENCE_ESCALATION_THRESHOLD`

```rust
/// Recalled same-signature recurrences at (or above) which a repeatedly-mitigated
/// problem is escalated as a systemic root cause instead of mitigated again.
/// Aligned with `no_progress_breaker::NO_PROGRESS_BREAKER_THRESHOLD` semantics.
pub const RECURRENCE_ESCALATION_THRESHOLD: u32 = 3;
```

Referred to as `N` throughout the docs.

## Remediation classification (`intervention.rs`)

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemediationClass {
    /// The action targets and fixes the primary root cause.
    RootCause,
    /// No fix is needed: the "cause" is a DELIBERATE operator / dependency block,
    /// correctly respected by a no-op. Counts as `root_cause_addressed` and is
    /// NEVER a symptom-mitigation, so an intentional block never inflates the
    /// symptom counter nor raises the "root cause unaddressed" alarm.
    Acknowledged,
    /// The action only mitigates the symptom; the root cause stays live.
    SymptomMitigation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remediation {
    pub class: RemediationClass,
    /// `true` for `RootCause` and `Acknowledged`; `false` only for
    /// `SymptomMitigation`. (Equivalently: `class != SymptomMitigation`.)
    pub root_cause_addressed: bool,
    /// `Some(<primary cause label>)` iff `class == SymptomMitigation` — always
    /// surfaced, never silent. `None` for `RootCause` and `Acknowledged` (neither
    /// leaves a live, unaddressed cause).
    pub unaddressed_note: Option<String>,
}
```

`PlannedIntervention` gains the classification (additive; set by `run_cycle`
after `gate()`):

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedIntervention {
    pub intervention: Intervention,
    pub admitted: bool,
    pub note: String,
    /// Whether this planned action targets the root cause or only the symptom.
    pub remediation: Remediation,
}
```

The two goal-health `Intervention` variants carry the rendered WHY so it reaches
the act / notify seam without re-analysis (additive fields; `Intervention::label()`
is unchanged — it ignores fields):

```rust
UnblockGoal { goal_id: String, reason: String, why: String },
EscalateBlockedGoal { goal_id: String, reason: String, why: String },
```

## Memory seam & loop integration (`mod.rs`)

```rust
impl Overseer {
    /// Attach cognitive memory (amplihack-memory-lib) for root-cause recall +
    /// store. Optional: when absent, root-cause analysis degrades to
    /// telemetry-only (`source = Telemetry`), logged via `tracing`, never silent.
    pub fn with_memory(mut self, mem: Arc<dyn CognitiveMemoryOps>) -> Self;

    /// READ-ONLY recall of prior same-signature occurrences, keyed on
    /// `Problem.dedup_key`. Uses the non-reinforcing `search_facts`. Returns `[]`
    /// when memory is absent or on any recall error (logged). Never mutates
    /// Simard state.
    fn recall_occurrences(&self, dedup_key: &str) -> Vec<PriorOccurrence>;

    /// DEFERRED, best-effort store of this occurrence's
    /// `{signature, primary cause, action, outcome}` after acting, so future
    /// recall detects recurrence. A store error is `tracing`-logged and ignored
    /// (non-fatal). Takes the per-problem `ProblemEntry` and the act `ActOutcome`.
    fn record_occurrence(&self, entry: &ProblemEntry, outcome: &ActOutcome);
}
```

The `Overseer` struct gains `mem: Option<Arc<dyn CognitiveMemoryOps>>`
(default `None`). `CognitiveMemoryOps` is
`crate::cognitive_memory::CognitiveMemoryOps`; a hermetic in-memory
implementation is available via `LibraryCognitiveMemory::in_memory()`.

### `run_cycle`

`run_cycle` (Observe → Orient → Decide → plan) now, for each problem:

1. `let recall = self.recall_occurrences(&problem.dedup_key);` — read-only.
2. `problem.why = Some(root_cause::analyze(&problem, &observed, &recall));`
3. `let iv = decide(&problem);` — Decide reads `problem.why` for the
   `EscalateBlockedGoal` `why` and for recurrence routing.
4. `planned.remediation = remediation_for(&iv, &why);`
5. pushes a `ProblemEntry { key, summary, why, action, remediation }` into
   `CycleReport.entries`.

Recall is read-only; **all writes are deferred** to the act/tick phase via
`record_occurrence`, so `run_cycle` keeps its "reports what WOULD be done"
no-mutation contract.

### `decide_blocked_goal` routing

See the [routing table in the concept doc](../concepts/overseer-root-cause-why.md#blocked-perpetual-goal-routing-the-operators-exact-ask).
In short: `recurrence < N` → one-off `UnblockGoal` (`RootCause`); `recurrence ≥ N`
→ `FileIssue` on the deduped `root_cause_signature` (`RootCause`, escalates the
cause); genuine `needs_review` → `EscalateBlockedGoal` (`RootCause`); plain
operator/dependency block → `Report` (`Acknowledged`, cause = deliberate block,
addressed — **not** a symptom, so it never fires the "unaddressed" alarm).

## Notification (`notify.rs`)

```rust
impl OperatorNotification {
    /// Build a blocked-goal escalation carrying the root-cause WHY. The `why`
    /// (rendered `RootCause`) is written into the message body as a
    /// `WHY (root cause): …` line so an escalation reaches a human WITH the
    /// diagnosed cause, not just the symptom. Additive: the existing
    /// `goal_blocked(id, reason)` constructor is retained.
    pub fn goal_blocked_with_why(goal_id: &str, reason: &str, why: &str) -> Self;
}
```

The `problem` body becomes:

```
Goal `{goal_id}` is blocked and needs human review.
  Reason: {reason}
  Why: {why}
```

## Activity feed (`activity.rs`)

```rust
/// One problem the Overseer handled this tick, with its WHY and remediation
/// class — the durable per-problem feed row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProblemEntry {
    pub key: String,
    pub summary: String,
    pub why: RootCause,
    pub action: String,
    pub remediation: Remediation,
}
```

`OverseerActivityRecord` gains the vector (additive; `#[serde(default)]` so old
records deserialize):

```rust
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverseerActivityRecord {
    pub timestamp: String,
    pub enabled: bool,
    pub report: OverseerTickReport,
    /// Per-problem rows: problem + WHY + action + root-cause/symptom.
    pub problem_entries: Vec<ProblemEntry>,
}
```

`ProblemEntry` and `OverseerActivityRecord` remain `Eq` because `RootCause`
uses ordinal enums, not `f64`. If a concurrent overseer-log-detail change adds
its own per-problem type, `ProblemEntry`'s fields merge into it (keep both
sides). `humanize_tick` renders each entry as
`"{summary} — WHY: {why} — {action} [root-cause|acknowledged|symptom]"` and, when
`symptom_mitigations > 0`, appends `"(N symptom-mitigation(s), root cause
unaddressed)"`. `[acknowledged]` entries (deliberate blocks) are never counted in
that summary.

`OverseerTotals` gains the running counters:

```rust
pub struct OverseerTotals {
    // … existing fields …
    /// Problems for which a structured WHY was produced.
    pub root_cause_analyses: u64,
    /// Actions labelled symptom-mitigation (root cause left unaddressed). A
    /// deliberate block (`Acknowledged`) is NOT counted here.
    pub symptom_mitigations: u64,
    /// Actions that addressed the root cause — class `RootCause` or `Acknowledged`
    /// (i.e. `remediation.root_cause_addressed == true`).
    pub root_causes_addressed: u64,
}
```

## Per-tick report (`wiring.rs`)

`OverseerTickReport` stays `Copy + Eq` — **scalars only** (a `Vec` would break
`Copy`; the rich `Vec<ProblemEntry>` lives on `OverseerActivityRecord`). It gains
three scalar counters, each emitted as a `tracing` key by `overseer_tick`:

```rust
pub struct OverseerTickReport {
    // … existing fields …
    /// Problems for which a structured root-cause WHY was produced this tick.
    pub root_cause_analyses: usize,
    /// Interventions labelled symptom-mitigation (root cause unaddressed) this
    /// tick. Deliberate blocks (`Acknowledged`) are NOT counted here.
    pub symptom_mitigations: usize,
    /// Interventions that addressed the root cause this tick — class `RootCause`
    /// or `Acknowledged` (`remediation.root_cause_addressed == true`).
    pub root_causes_addressed: usize,
}
```

`overseer_tick` tallies the counters as it executes the plan: it counts
`root_cause_analyses` from the problems carrying a `why`, and — only for an action
that actually **took effect** (any outcome except a dedup/rate-limit suppression
no-op) — increments `root_causes_addressed` when `remediation.root_cause_addressed`
is `true` (both `RootCause` and `Acknowledged`) and `symptom_mitigations` only when
`remediation.class == SymptomMitigation`. Counting after the act (not from the
plan) keeps the feed honest: a suppressed self-heal or an errored escalation is
never reported as a cause addressed. Because an `Acknowledged` deliberate block
sets `root_cause_addressed == true`, it never increments `symptom_mitigations`, so
an intentional block can neither inflate the symptom count nor fire the feed alarm.

## Configuration

Root-cause analysis is **always-on and mandatory** — there is no enable/disable
flag, by design (the principle is "ALWAYS ask WHY"). The only availability-gated
part is the memory-recall enrichment, which degrades gracefully (telemetry-only,
logged) when no `CognitiveMemoryOps` handle is attached. The acting paths the WHY
feeds remain governed by the existing
[`SIMARD_OVERSEER_GOAL_HEALTH`](./overseer-goal-board-health-api.md) gate. `N` is
the compile-time `RECURRENCE_ESCALATION_THRESHOLD` constant (default 3).

## Tests (`tests_root_cause.rs`)

Hermetic — existing fakes plus `LibraryCognitiveMemory::in_memory()`; no network,
no `~/.simard`. Cover: (1) a structured `why` on **every** problem; (2) a
symptom-only action labelled `SymptomMitigation` with the root cause recorded
unaddressed, and a recurring perpetual re-block routed to a deduped root-cause
`FileIssue`; (2b) a deliberate operator/dependency block labelled `Acknowledged`
(not `SymptomMitigation`), leaving `symptom_mitigations` unincremented and the
feed alarm silent; (3) the WHY rendered in `OverseerActivityRecord.problem_entries`
and the symptom-mitigation summary in `humanize_tick`; (4) first-time false-park
still self-heals (recurrence 0 → `UnblockGoal`, `RootCause`) — pins #2609;
(5) graceful memory degrade (`mem = None` → `source = Telemetry`, no panic);
(6) the notification body carries the WHY line; (7) recurrence accumulates across
two ticks on the same signature; (8) Orient stays pure (`why: None`).

Run:

```bash
cargo test -p simard overseer::root_cause
cargo test -p simard overseer::
```

## Invariants

- `OverseerTickReport` stays `Copy + Eq` (scalar counters only).
- `OverseerActivityRecord` / `ProblemEntry` stay `Eq` (ordinal enums, no `f64`).
- Every serialized additive field carries a clean default (`#[serde(default)]`
  or `Option`/`Vec` default) so old feed / report JSON deserializes unchanged.
- The analyzer is deterministic and does no I/O; recall/store are the only memory
  touches and both degrade gracefully with a `tracing` log — no silent fallback.
- `run_cycle` performs no Simard mutations (recall read-only; stores deferred).

## See also

- [Overseer root-cause ("WHY") principle](../concepts/overseer-root-cause-why.md)
- [Configure and observe the Overseer root-cause principle](../howto/configure-overseer-root-cause-why.md)
- [Overseer goal-board health API](./overseer-goal-board-health-api.md)
- [Overseer activity feed reference](./overseer-activity-feed.md)
- [Ranked episodic recall & memory reinforcement](./cognitive-memory-ranked-episodic-recall.md)
