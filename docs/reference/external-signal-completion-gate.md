---
title: External-signal completion gate reference
description: Reference for the external-signal completion gate (#2456) — the signal-strength classifier that decides goal completion from strong external signals (merged PR, closed issue, green CI, build/test exit-success, satisfied postcondition, verified deploy) instead of weak self-reports, the refuted→OpenTrackingIssue routing, the goal_completion_verification / goal_false_completion_rate metrics, the hold-for-review handling of no-signal completions (with GoalProgress::CompletedUnverified kept as future design), and how it extends (not duplicates) the deploy-aware CompletionEvidenceGate (#2450).
last_updated: 2026-06-28
review_schedule: as-needed
owner: simard
doc_type: reference
status: partially implemented
related:
  - ../concepts/trustworthy-confidence-and-external-completion.md
  - ../concepts/deploy-aware-done-gate.md
  - ./completion-evidence-gate-api.md
  - ./trustworthy-confidence-api.md
  - ./progress-evidence-api.md
  - ../howto/interpret-brain-confidence-and-verify-completion.md
  - ../../src/goal_curation/completion_gate.rs
  - ../../src/goal_curation/operations.rs
  - ../../src/goal_curation/types.rs
---

# External-signal completion gate reference

> **Status: partially implemented (issue [#2456](https://github.com/rysweet/Simard/issues/2456), open).**
>
> **Shipped now** — a verification-outcome classifier that *extends* the
> deploy-aware [`CompletionEvidenceGate`](./completion-evidence-gate-api.md)
> (#2450) without duplicating it, in
> [`src/goal_curation/completion_gate.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs):
> the
> [`VerificationOutcome`](#verificationoutcome-shipped) enum
> (`Verified` / `UnverifiedNoSignal` / `Refuted` / `Error`); `classify_outcome`
> and `classify_from_missing` mapping a `CompletionVerdict` to an outcome;
> `has_derivable_signal` (a goal has an external signal iff it has a PR ref, an
> issue ref, or is self-affecting); `record_completion_verification` emitting
> `goal_completion_verification` and `record_false_completion_rate` emitting the
> `goal_false_completion_rate` time-series via `self_metrics::record_metric`; and
> `false_completion_rate` = `refuted / (verified + refuted)`. `archive_completed_evidence_aware`
> records the per-event outcome and the per-batch rate for every completion
> candidate. All are unit-tested.
>
> **Exists already** (the #2450 precedent this extends):
> [`CompletionEvidence`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
> (`pr_merged`, `issue_closed`, `self_affecting`, `deployed`),
> [`CompletionVerdict`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
> (`Complete` / `Blocked`), the injected `EvidenceSource` seam, and
> [`GoalProgress`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/types.rs).
>
> **Future design (NOT in this slice)** — the more invasive surface below is kept
> as forward design because it would change load-bearing contracts: the
> `SignalStrength` taxonomy and new strong-signal fields on `CompletionEvidence`
> (`checks_passed`, `exit_success`, `postcondition_satisfied`), the
> `CompletionVerdict::Unverified` arm, and the `GoalProgress::CompletedUnverified`
> state (which would ripple through ~60 exhaustive `match GoalProgress` sites).
> The shipped slice deliberately marks no-signal completions *unverified at the
> metric level* and keeps them blocked-and-retained (never archived) rather than
> minting a new persistent goal state.

This reference specifies the typed surface of the external-signal completion
gate (#2456). For the rationale, see
[trustworthy confidence + external-signal completion](../concepts/trustworthy-confidence-and-external-completion.md).

## Contents

- [What changes](#what-changes)
- [Signal-strength taxonomy](#signal-strength-taxonomy)
- [`CompletionEvidence` (extended)](#completionevidence-extended)
- [`VerificationOutcome` (shipped)](#verificationoutcome-shipped)
- [`CompletionVerdict` (extended)](#completionverdict-extended)
- [`GoalProgress::CompletedUnverified`](#goalprogresscompletedunverified)
- [Refuted completions route to `OpenTrackingIssue`](#refuted-completions-route-to-opentrackingissue)
- [Verification metrics](#verification-metrics)
- [Relationship to the deploy-aware done-gate (#2450)](#relationship-to-the-deploy-aware-done-gate-2450)
- [Environment knobs](#environment-knobs)
- [Compatibility & wire stability](#compatibility-and-wire-stability)

## What changes

The current "mark complete" decision trusts **artifact existence** — a commit
or a PR existing is treated as completion. #2456 would replace that with a
signal-strength ladder: **mere existence would no longer be sufficient.** A goal
would reach `Completed` only when a subordinate's done-claim is corroborated by
at least one *strong* external signal; otherwise it would land in the honest
[`CompletedUnverified`](#goalprogresscompletedunverified) state (held for review,
never archived) or be `refuted` and routed to a tracking issue.

## Signal-strength taxonomy

```rust
/// How much trust a completion signal earns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalStrength {
    /// Independently verifiable; can complete a goal.
    Strong,
    /// Self-report-adjacent; never sufficient on its own.
    Weak,
}
```

| Strength | Signal | Source |
| --- | --- | --- |
| **Strong** | merged PR | `EvidenceSource::pr_merged` (existing) |
| **Strong** | closed linked issue | `EvidenceSource::issue_closed` (existing) |
| **Strong** | verified deploy | `EvidenceSource::is_deployed` / [#2450 gate](./completion-evidence-gate-api.md) |
| **Strong** | green CI / checks passed | `EvidenceSource::checks_passed` (new) |
| **Strong** | build/test exit-success | engineer run exit status (new) |
| **Strong** | satisfied goal-graph postcondition | goal-graph predicate (new) |
| **Weak** | open (un-merged) PR exists | wip-ref scan |
| **Weak** | local commit exists | git log |
| **Weak** | subordinate "I finished" text | progress-claim string |

> Strong signals are gathered through the same injected
> [`EvidenceSource`](./completion-evidence-gate-api.md#evidence-sources) seam the
> deploy-aware gate already uses, so the ladder runs hermetically in tests with
> no network and no live `gh`.

## `CompletionEvidence` (extended)

The existing struct would gain the new strong-signal facts. Existing fields are
unchanged. The struct today carries only `pr_merged`, `issue_closed`,
`self_affecting`, and `deployed`.

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionEvidence {
    pub pr_merged: bool,        // existing
    pub issue_closed: bool,     // existing
    pub self_affecting: bool,   // existing
    pub deployed: bool,         // existing
    // --- new in #2456 ---
    /// CI / required checks are green for the goal's merged change.
    pub checks_passed: bool,
    /// The engineer run that claimed completion exited 0 (build/tests passed).
    pub exit_success: bool,
    /// A goal-graph postcondition predicate for this goal is satisfied.
    pub postcondition_satisfied: bool,
}

impl CompletionEvidence {
    /// `true` if **any** strong signal is present.
    pub fn has_strong_signal(&self) -> bool;
    /// `true` if a strong signal was *derivable* (checkable) but failed
    /// (e.g. CI red, exit ≠ 0, issue still open after a merged PR).
    pub fn is_refuted(&self) -> bool;
}
```

## `VerificationOutcome` (shipped)

The per-completion verdict used for [metrics](#verification-metrics) and for the
[calibration ground truth](./trustworthy-confidence-api.md#calibration-expected-calibration-error).
Shipped today in
[`completion_gate.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
(the broader-design name in earlier drafts was `CompletionVerification`; the
variants are identical).

```rust
// src/goal_curation/completion_gate.rs — SHIPPED
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationOutcome {
    /// ≥1 external postcondition was satisfied (gate said Complete).
    Verified,
    /// No external postcondition is derivable — held unverified, not trusted.
    UnverifiedNoSignal,
    /// A derivable external signal contradicted the completion claim.
    Refuted,
    /// A signal lookup failed (gh/git/drift query error) this cycle.
    Error,
}

impl VerificationOutcome {
    pub fn metric_label(&self) -> &'static str; // "verified" | "unverified_no_signal" | "refuted" | "error"
    pub fn metric_code(&self) -> f64;           // 0.0 | 1.0 | 2.0 | 3.0
    pub fn is_false_completion(&self) -> bool;  // true only for Refuted
}

/// Classify a gate verdict for one goal.
pub fn classify_outcome(goal: &ActiveGoal, verdict: &CompletionVerdict) -> VerificationOutcome;
/// Classify directly from a blocked verdict's missing-evidence list.
pub fn classify_from_missing(goal: &ActiveGoal, missing: &[MissingEvidence]) -> VerificationOutcome;
/// A goal has an external signal iff it has a PR ref, an issue ref, or is self-affecting.
pub fn has_derivable_signal(goal: &ActiveGoal) -> bool;
```

Classification rules over the existing `CompletionVerdict`:

| Verdict | Goal has derivable signal? | `VerificationOutcome` |
| --- | --- | --- |
| `Complete` | — | `Verified` |
| `Blocked` containing `CouldNotVerify` | — | `Error` |
| `Blocked` (no `CouldNotVerify`) | yes | `Refuted` |
| `Blocked` (no `CouldNotVerify`) | no | `UnverifiedNoSignal` |

`Error` dominates: a `CouldNotVerify` blocker classifies as `Error` even when a
real refutation signal is also present (the truth is simply unknown this cycle).

## `CompletionVerdict` (extended)

The existing two-arm verdict gains a third arm for the honest-unverified case.
`Complete` and `Blocked` are unchanged.

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum CompletionVerdict {
    /// ≥1 strong signal present — completion/archive may proceed.
    Complete(CompletionEvidence),
    /// A strong clause is checkable but failed — keep active, record blocker.
    Blocked {
        evidence: CompletionEvidence,
        missing: Vec<MissingEvidence>,
    },
    // --- new in #2456 ---
    /// Claimed done with no strong signal derivable — neither proven nor
    /// refuted. Maps to `GoalProgress::CompletedUnverified`.
    Unverified(CompletionEvidence),
}
```

Decision table (subordinate claims done). **A refuted checkable signal takes
precedence:** if any strong signal is *refuted*, the goal is not completed even
when another strong signal is present.

| Strong signal present? | Strong signal refuted? | Verdict | `GoalProgress` | `CompletionVerification` |
| --- | --- | --- | --- | --- |
| yes | no | `Complete` | `Completed` | `Verified` |
| yes | yes | `Blocked` → tracking issue | unchanged (stays active) | `Refuted` |
| no | yes | `Blocked` → tracking issue | unchanged (stays active) | `Refuted` |
| no | no | `Unverified` | `CompletedUnverified` | `UnverifiedNoSignal` |
| lookup failed | — | `Blocked { CouldNotVerify }` | unchanged | `Error` |

## `GoalProgress::CompletedUnverified`

A new, additive variant **proposed** for
[`GoalProgress`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/types.rs)
(today the enum ends at `Completed`).

```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GoalProgress {
    Proposed,
    NotStarted,
    InProgress { percent: u32 },
    Blocked(String),
    Paused,
    Completed,
    /// Subordinate reported done, but no strong external signal corroborates
    /// it. Held for human review — explicitly distinct from `Completed`, and
    /// **not archivable**.
    CompletedUnverified,
}

impl Display for GoalProgress {
    // Self::CompletedUnverified => "completed-unverified"
}
```

**Not archivable.** The archive-candidate predicate
[`completion_gate.rs::is_complete_candidate`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
would continue to match `Completed` (or `InProgress { percent >= 100 }`)
**only**, and
[`archive_completed_with_evidence`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
would archive just those candidates — `CompletedUnverified` is deliberately
excluded so unproven work is never silently archived. It would surface on the
operator dashboard as `completed-unverified` and be held until a strong signal
arrives (promoting it to `Completed`) or review re-routes it.

> **Exhaustive matches to update.** Adding the variant would ripple through
> every exhaustive `match GoalProgress` (orient demotion, advance-goal,
> goal-session, the operator dashboard current-work / workboard / goals views,
> and the string-parse round-trip). All must be updated to handle
> `CompletedUnverified` explicitly; none should silently absorb it into a `_`
> arm.

## Refuted completions route to `OpenTrackingIssue`

When a done-claim is **refuted** (a strong signal was checkable and failed — CI
red, exit ≠ 0, merged PR but the linked issue is still open), the goal would
**not** be completed or archived. The blocker would be recorded and the
engineer-lifecycle path would emit
[`OpenTrackingIssue`](./ooda-brain-decision-protocol.md) so a human sees the
false-done claim. This would reuse the existing lifecycle action; no new
routing machinery is added.

## Verification metrics

The shipped sink is the existing
[`self_metrics::record_metric`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
(`~/.simard/metrics/metrics.jsonl`).

- **Per-event (shipped):**
  `record_completion_verification(outcome)` emits
  `record_metric("goal_completion_verification", outcome.metric_code(), outcome.metric_label())`
  for every completion candidate evaluated by
  `archive_completed_evidence_aware`. `metric_code()` encodes the
  [`VerificationOutcome`](#verificationoutcome-shipped) variant
  (`verified=0`, `unverified_no_signal=1`, `refuted=2`, `error=3`).
- **Rolling rate (shipped):**
  `false_completion_rate(&[VerificationOutcome]) -> Option<f64>` computes

  ```
  false_completion_rate = refuted / (verified + refuted)
  ```

  `unverified_no_signal` and `error` are **excluded from the denominator** — the
  rate measures *wrong* completions among *checkable* ones, so signal-less goals
  do not dilute it. `record_false_completion_rate(&outcomes)` emits this per
  archival batch as the dedicated `goal_false_completion_rate` time-series metric
  (value = the rate; `context = "refuted=R checkable=C"` so a reader can re-pool a
  rate across batches). `archive_completed_evidence_aware` calls it once per pass;
  it is a no-op when the batch had nothing checkable.

These per-event outcomes are also the ground-truth feed for the brain's
[calibration / ECE](./trustworthy-confidence-api.md#calibration-expected-calibration-error)
(`verified → true`, `refuted → false`; the rest excluded).

## Relationship to the deploy-aware done-gate (#2450)

This gate **extends, and does not duplicate,**
[`CompletionEvidenceGate`](./completion-evidence-gate-api.md):

- The deploy-aware gate guards **archival** (merged PR + closed issue + verified
  deploy). #2456 guards the earlier **"mark complete"** decision so weak signals
  never reach the archive gate as `Completed`.
- **Semantics differ by stage, deliberately.** Mark-complete uses **OR** (any one
  strong signal corroborates the done-claim → `Completed`); archival still
  enforces the deploy-gate's **AND** (merged PR + closed issue + verified deploy).
  A goal marked `Completed` via a single strong signal must still clear the AND
  gate before it archives.
- "Deploy-verified" (`!DeployDrift::needs_deploy`) is consumed as **one strong
  signal** in the ladder above.
- Both share the injected `EvidenceSource` seam and the goal types in
  `src/goal_curation/types.rs`.

## Environment knobs

| Variable | Default | Meaning |
| --- | --- | --- |
| `SIMARD_COMPLETION_VERIFICATION` | `strict` | `strict` = require a strong signal to reach `Completed` (weak ⇒ `CompletedUnverified`); `lenient` = legacy artifact-existence behaviour (for rollback only); `off` = disable the ladder. |
| `SIMARD_COMPLETION_EVIDENCE` | (existing) | The deploy-aware archive gate kill-switch ([#2450](./completion-evidence-gate-api.md#kill-switch)); honored unchanged. |

## Compatibility and wire stability

- **Additive serde.** `GoalProgress::CompletedUnverified` is a new variant;
  legacy goal-board snapshots never contain it, and it only deserializes on new
  code. Existing variants serialize byte-identically.
- **Verdict back-compat.** `Complete` / `Blocked` shapes are unchanged; only the
  new `Unverified` arm and the new `CompletionEvidence` fields are added.
- **Rollback.** `SIMARD_COMPLETION_VERIFICATION=lenient` restores the prior
  artifact-existence completion behaviour without a redeploy, for emergency use.

## See also

- [Concept: trustworthy confidence + external-signal completion](../concepts/trustworthy-confidence-and-external-completion.md)
- [Concept: deploy-aware done-gate](../concepts/deploy-aware-done-gate.md)
- [Completion-evidence gate API reference](./completion-evidence-gate-api.md)
- [Trustworthy-confidence API reference](./trustworthy-confidence-api.md)
- [How-to: interpret brain confidence and verify completion](../howto/interpret-brain-confidence-and-verify-completion.md)
