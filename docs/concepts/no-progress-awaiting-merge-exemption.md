---
title: A goal with an open, mergeable PR is awaiting merge — never reaped
description: Why the OODA no-progress breaker no longer reaps and re-dispatches an engineer whose workstream has already delivered an open, non-draft, mergeable PR; the duplicate-PR incident (issue #4441) it fixes; the non-terminal `AwaitMerge` disposition that lets the loop idle a completed-but-unmerged goal instead of acting; and the fail-closed predicate that only ever suppresses a legitimate reap, never a genuine stall.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./no-progress-root-cause-resolution.md
  - ./perpetual-goal-no-progress-exemption.md
  - ./deploy-aware-done-gate.md
  - ./closed-loop-outcome-verification.md
  - ../reference/no-progress-awaiting-merge-api.md
  - ../reference/no-progress-breaker-api.md
  - ../reference/completion-evidence-gate-api.md
  - ../howto/diagnose-an-awaiting-merge-idle.md
  - ../howto/diagnose-a-no-progress-block.md
---

# A goal with an open, mergeable PR is awaiting merge — never reaped

> **Status: implemented (issue #4441).** The `open_mergeable_pr` evidence
> signal, the `CompletionEvidenceGate::awaiting_merge` pass-through, and the
> `StuckGoalDisposition::AwaitingMerge` disposition live in
> `src/goal_curation/completion_gate.rs` and
> `src/goal_curation/no_progress_breaker.rs`. The non-terminal
> `NoProgressResolution::AwaitMerge` resolution and its no-op side effect
> (**no** `spawn_engineer`, **no** reap) live in
> `src/ooda_loop/no_progress.rs`. For the exact types, the `gh` query, and the
> fail-closed table see the
> [awaiting-merge API reference](../reference/no-progress-awaiting-merge-api.md).

This concept explains why the OODA no-progress breaker treats a goal whose
workstream already has an **open, non-draft, mergeable PR** as *awaiting an
external merge action*, not as a stalled engineer to reap and re-dispatch. It
sits alongside — and does not replace — the
[root-cause resolution ladder](./no-progress-root-cause-resolution.md) and the
[standing/perpetual exemption](./perpetual-goal-no-progress-exemption.md).

## The incident: duplicate PRs from reaping finished engineers

The no-progress breaker fires when a goal produces
`NO_PROGRESS_BREAKER_THRESHOLD` (3) consecutive no-action cycles. Before this
fix, its liveness check considered only *elapsed no-action cycles* — it never
asked whether the work had **already been delivered**.

An engineer that finishes its workstream, opens a clean PR, and then simply
waits for a human (or the merge queue) to land it produces no further
board-visible "action". After three such idle cycles the breaker classified the
goal as stalled, reaped the engineer, and re-dispatched a fresh one — which
redid the work and opened a **second** PR for the same workstream. This is the
observed root cause of merge-ready PRs (e.g. #4440 and #4398) lingering
unmerged while duplicate PRs accumulated: the loop was fighting its own
finished work.

The fix adds the missing question to done-detection: **"has this workstream
already delivered an open, mergeable PR?"** If yes, the goal is *awaiting
merge*, and the breaker must idle it — not reap it.

## What "awaiting merge" means

A goal is `AwaitingMerge` only when an associated PR (resolved from the goal's
`wip_refs` via the first ref of kind `"pr"`) satisfies **all three** clauses:

| Clause | Predicate | Why |
| --- | --- | --- |
| Open | `state == OPEN` | A closed/merged PR is not "awaiting" anything. A merged PR is already `Done` via the [deploy-aware done-gate](./deploy-aware-done-gate.md). |
| Not draft | `isDraft == false` | A draft PR is explicitly not ready to merge, so it does not signal delivered work. |
| Mergeable | `mergeable == MERGEABLE` | A `CONFLICTING` or `UNKNOWN` PR is not landable; the engineer may still have work to do. (`gh pr view --json mergeable` returns GitHub's `MergeableState` enum — `MERGEABLE`/`CONFLICTING`/`UNKNOWN` — **not** the `mergeStateStatus` `CLEAN`/`DIRTY` enum, which this branch does not query.) |

If any clause fails — draft, dirty, conflicting, unknown, no tracked PR, or the
`gh` query errored — the goal is **not** suppressed and falls through to the
existing reap/escalate path unchanged.

## Non-terminal by design: idle, don't act

`AwaitMerge` is deliberately a **non-terminal** resolution. It is neither `Done`
(which reaps/archives the goal) nor `Escalate`/`SpawnEngineer` (which
re-dispatches). Its side effect is to **do nothing**: the goal stays tracked and
active, the breaker takes no disruptive action, and merging is left to the
external operator or merge queue.

Concretely, in `apply_resolution_side_effects` the `AwaitMerge` arm:

- does **not** call `dispatcher.spawn_engineer(...)` — so no duplicate PR is created;
- does **not** reap the engineer or mark the goal `Blocked`/`Completed`;
- records the goal in the report's `awaiting_merge` list and emits one
  structured `tracing::info!` line documenting the suppression decision.

Because the resolution is non-terminal, it does **not** contribute to the
report's [`fired()`](../reference/no-progress-awaiting-merge-api.md#report-field)
— idling a completed-awaiting-merge goal is normal, not a breaker firing (the
same treatment given to a standing/perpetual idle).

## Indefinite wait, instant fallback

As long as the open, mergeable PR persists, the goal waits **indefinitely**.
There is no secondary timeout that would re-arm the reaper — a timeout would
just reintroduce the duplicate-PR race the fix eliminates. The evidence is
re-evaluated every cycle, so the safety property is preserved by re-checking,
not by parking:

- If the PR **merges**, the [deploy-aware done-gate](./deploy-aware-done-gate.md)
  certifies the goal `Done` on the next cycle and it archives normally.
- If the PR **closes without merging**, goes **draft**, or degrades below
  `MERGEABLE` (e.g. a new conflict), the `awaiting_merge` signal flips to
  `false` and the goal falls straight back to the existing reap/escalate path on
  the very next cycle.

There is no unbounded resource growth: an awaiting-merge goal spawns no work and
holds no engineer.

## Fail-closed: can only cause a legitimate reap, never suppress one

The new signal follows the same fail-closed convention as the rest of the
[completion-evidence gate](./deploy-aware-done-gate.md): any uncertainty
resolves to "not awaiting merge". The `EvidenceSource::open_mergeable_pr`
trait method **defaults to `Ok(false)`**, and the live `GhCliEvidenceSource`
resolves `false` on **any** of: no tracked PR, a `gh` spawn/exit error, a JSON
parse failure, or a `mergeable` value that is anything other than `MERGEABLE`
(including `UNKNOWN` and `CONFLICTING`).

The consequence is one-directional: an error or ambiguity can only fail to
suppress a reap that would otherwise happen — it can **never** suppress a reap
that *should* happen. A genuinely stalled engineer (no PR, or a broken PR) is
always still reaped and escalated. The breaker's thresholds and stalled-engineer
semantics are untouched.

## What is unchanged

- `NO_PROGRESS_BREAKER_THRESHOLD` and the sentinel constants — unchanged.
- The reap/escalate control flow for genuinely-stalled engineers (no PR, draft,
  dirty, conflicting, or unverifiable) — byte-for-byte unchanged.
- The *behavior* of the existing root-cause resolution ladder rungs (`Heal` /
  `Defer` / `SpawnEngineer` / `Escalate` / `SurfaceInvestigationFailure`) —
  unchanged. `AwaitMerge` is purely additive: the `NoProgressResolution` and
  `StuckGoalDisposition` enums each gain one variant, `NoProgressResolution::is_terminal()`
  gains one non-terminal arm, and each of the two `NoProgressResolution` match
  sites in `src/ooda_loop/no_progress.rs` gains one idle-only arm. No existing
  rung's control flow is altered.
- The [deploy-aware done-gate](./deploy-aware-done-gate.md) verdict semantics —
  unchanged; `awaiting_merge` is a *pass-through* query on `EvidenceSource`,
  not a new `CompletionVerdict` variant.
- No `Bridge` naming; no `print!`/`println!` — the suppression decision is
  surfaced only through structured `tracing` + OTel.

## See also

- [Awaiting-merge API reference](../reference/no-progress-awaiting-merge-api.md) — the `open_mergeable_pr` signal, the `gh` query, the `AwaitingMerge` disposition, the `AwaitMerge` resolution, the report field, and the fail-closed table.
- [No-progress breaker API reference](../reference/no-progress-breaker-api.md) — the base breaker, its threshold, sentinel, and report.
- [Concept: the no-progress breaker explains WHY and self-resolves before escalating](./no-progress-root-cause-resolution.md) — the root-cause ladder this branch precedes.
- [Concept: deploy-aware done-gate](./deploy-aware-done-gate.md) — the sibling gate that certifies a **merged** PR as `Done`.
- [Diagnose an awaiting-merge idle](../howto/diagnose-an-awaiting-merge-idle.md) — read the suppression trace and confirm no duplicate PR was created.
