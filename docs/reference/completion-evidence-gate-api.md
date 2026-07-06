---
title: Completion-evidence gate API reference
description: Reference for the CompletionEvidenceGate that blocks goal completion and archival without a merged PR, a closed linked issue, and (for self-affecting changes) a verified deploy — its types, the self-affecting classifier, the evidence sources, the archive integration, the kill-switch, and the pinned prompt wording.
last_updated: 2026-06-27
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/deploy-aware-done-gate.md
  - ../reference/progress-evidence-api.md
  - ../reference/self-deploy-api.md
  - ../howto/diagnose-a-rejected-goal-completion.md
  - ../../src/goal_curation/completion_gate.rs
  - ../../src/goal_curation/operations.rs
  - ../../src/goal_curation/types.rs
---

# Completion-evidence gate API reference

> **Status: implemented.** The `CompletionEvidenceGate`, its evidence types, the
> `is_self_affecting` classifier, `archive_completed_with_evidence` /
> `archive_completed_evidence_aware`, the `GhCliEvidenceSource`, and the
> `SIMARD_COMPLETION_EVIDENCE` kill-switch live in
> [`src/goal_curation/completion_gate.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs).
> They sit alongside the legacy
> [`archive_completed`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/operations.rs)
> and the [`progress_evidence`](../reference/progress-evidence-api.md) checker;
> the production daemon installs the gate on `OodaClients.completion_evidence`
> unless the kill-switch is `off`.

This reference specifies the API for the deploy-aware done-gate. For the
rationale, see [deploy-aware-done-gate](../concepts/deploy-aware-done-gate.md).
The gate lives alongside the existing
[`progress_evidence`](../reference/progress-evidence-api.md) checker in
`src/goal_curation/` and shares the goal types in
`src/goal_curation/types.rs`.

## Contents

- [`CompletionEvidence`](#completionevidence)
- [`MissingEvidence`](#missingevidence)
- [`CompletionVerdict`](#completionverdict)
- [`CompletionEvidenceGate`](#completionevidencegate)
- [Evidence sources](#evidence-sources)
- [Self-affecting classification](#self-affecting-classification)
- [Archive integration](#archive-integration)
- [Brain Decide integration](#brain-decide-integration)
- [Kill-switch](#kill-switch)
- [Pinned prompt wording](#pinned-prompt-wording)

## `CompletionEvidence`

The verified facts the gate gathered for one goal.

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionEvidence {
    /// A PR in the goal's wip_refs (or referencing its issue) is merged.
    pub pr_merged: bool,
    /// The goal's linked issue is closed.
    pub issue_closed: bool,
    /// The change affects Simard's own running code (see classifier).
    pub self_affecting: bool,
    /// For self-affecting goals: the merged change is running
    /// (`!DeployDrift::needs_deploy`). `true` for non-self-affecting goals.
    pub deployed: bool,
}
```

## `MissingEvidence`

```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MissingEvidence {
    /// No merged PR found for this goal.
    PrNotMerged,
    /// The linked issue is still open.
    IssueOpen,
    /// Self-affecting change is merged but not yet running.
    NotDeployed,
    /// A git/gh/drift query failed; completion cannot be verified this cycle.
    CouldNotVerify { detail: String },
}
```

## `CompletionVerdict`

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum CompletionVerdict {
    /// All applicable clauses hold — completion/archive may proceed.
    Complete(CompletionEvidence),
    /// One or more clauses fail — keep the goal active, record the blocker.
    Blocked { evidence: CompletionEvidence, missing: Vec<MissingEvidence> },
}
```

## `CompletionEvidenceGate`

```rust
/// Verifies the three-part done-gate. Evidence lookups are injected so tests run
/// hermetically with no network and no live `gh`.
pub struct CompletionEvidenceGate<E: EvidenceSource> {
    source: E,
}

impl<E: EvidenceSource> CompletionEvidenceGate<E> {
    pub fn new(source: E) -> Self;

    /// Evaluate one goal. Never panics. On a source error, returns
    /// `Blocked` with a `CouldNotVerify` so a goal is never completed on
    /// unverifiable evidence, and the cycle is never crashed.
    pub fn evaluate(&self, goal: &ActiveGoal) -> CompletionVerdict;
}
```

## Evidence sources

```rust
pub trait EvidenceSource: Send + Sync {
    /// Is any PR for this goal merged? (wip_refs of kind "pr", or a merged PR
    /// referencing the goal's issue.)
    fn any_pr_merged(&self, goal: &ActiveGoal) -> SimardResult<bool>;
    /// Is the goal's linked issue closed?
    fn issue_closed(&self, goal: &ActiveGoal) -> SimardResult<bool>;
    /// Is the merged self-change running? Backed by the Workstream A
    /// `ReconcileDetector` (`!DeployDrift::needs_deploy`).
    fn is_deployed(&self, goal: &ActiveGoal) -> SimardResult<bool>;
}
```

The production `EvidenceSource` resolves PR/issue state through the same `gh`
surface the [PR-finalization pipeline](../reference/pr-finalization-pipeline.md)
uses, and resolves `is_deployed` through the
[`ReconcileDetector`](../reference/self-deploy-api.md#reconciledetector). Tests
inject a canned source.

## Self-affecting classification

```rust
/// A goal affects Simard's own running code when either:
///   * its `repo` routes to the Simard repo (the default `None`, or an explicit
///     "Simard" slug), OR
///   * it bumps a pinned dependency rev in Simard's own `Cargo.toml`
///     (detected from the goal description / wip_refs touching `Cargo.toml`).
/// Docs-only goals and goals targeting another repo's own surface are NOT
/// self-affecting, so clause 3 (deployed) is skipped for them.
pub fn is_self_affecting(goal: &ActiveGoal) -> bool;
```

When `is_self_affecting(goal)` is `false`, `CompletionEvidence::deployed` is set
to `true` unconditionally (clause 3 does not apply) and `NotDeployed` can never
appear in `missing`.

## Archive integration

`archive_completed` is replaced at its call sites by an evidence-aware variant.
The legacy function is retained for the kill-switch path only.

```rust
/// Archive only goals the gate certifies `Complete`. Goals that fail the gate
/// are retained on the active board; their blockers are returned so the caller
/// can record and surface them. Returns (archived, blocked).
pub fn archive_completed_with_evidence<E: EvidenceSource>(
    board: &mut GoalBoard,
    gate: &CompletionEvidenceGate<E>,
) -> (Vec<ActiveGoal>, Vec<(ActiveGoal, Vec<MissingEvidence>)>);
```

A goal whose `status` is `Completed` but whose gate verdict is `Blocked` is
**kept active** and its `current_activity` is annotated with the missing
evidence, so the dashboard and the next cycle see why it did not archive.

## Brain Decide integration

Before a brain decision may persist `GoalProgress::Completed` (or an
`STATUS: ACHIEVED` that maps to it), the Decide application path calls
`gate.evaluate(goal)` and downgrades a `Blocked` verdict to "still in progress,"
recording the missing evidence. This holds even when the engineer-lifecycle brain
fell back to a default decision ([#2419](https://github.com/rysweet/Simard/issues/2419)):
a defaulted completion cannot pass a gate it never satisfied.

## Kill-switch

```text
SIMARD_COMPLETION_EVIDENCE=off
```

Set to `off` to bypass the gate and restore the legacy unguarded
`archive_completed` behaviour. Any other value (or unset) keeps the gate active.
This mirrors `SIMARD_PROGRESS_EVIDENCE`
([progress-evidence kill-switch](../operations/progress-evidence-kill-switch.md)).
Use it only to recover from a gate defect.

## Pinned prompt wording

The done-gate is stated in the curation and decide prompts and **content-pinned**
by tests so the wording cannot silently drift:

| Prompt asset | Pinned statement |
| --- | --- |
| `prompt_assets/simard/goal_curator_system.md` | A goal is complete only with a merged PR, a closed linked issue, and — for changes to Simard's own running code — a verified deploy. |
| `prompt_assets/simard/ooda_decide.md` | Do not propose `STATUS: ACHIEVED` without merged + closed + (if self-affecting) deployed evidence. |

The content-pin tests assert these exact sentences survive prompt edits, the same
mechanism used by
[self-maintain-dependency-pins](../howto/self-maintain-dependency-pins.md).

## See also

- [deploy-aware-done-gate concept](../concepts/deploy-aware-done-gate.md)
- [How to diagnose a rejected goal completion](../howto/diagnose-a-rejected-goal-completion.md)
- [Progress-evidence API](../reference/progress-evidence-api.md) — the sibling percent-increase gate.
- [Self-deploy API reference](../reference/self-deploy-api.md) — the deploy evidence source.
