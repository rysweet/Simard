---
title: Merge draft-state re-validation
description: >
  Why the merge authority gates on a first-class `is_draft` fact evaluated
  against the fresh per-attempt PR snapshot it already fetches, so a GREEN,
  non-draft, MERGEABLE PR merges
  instead of being re-escalated to the operator every tick with the spurious
  "Pull Request is still a draft" abort (issues #4344 / #4145). Explains the
  observed 13-tick self-merge stall, why a missing draft field let the objective
  gates pass and then let `gh pr merge` abort downstream, and how a fail-closed
  draft gate plus a fresh pre-merge snapshot turns the loop into a deterministic
  merge-or-actionable-refusal.
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: concept
status: reference
related:
  - ../reference/merge-draft-gate-api.md
  - ../reference/cross-repo-merge-authority.md
  - ./operational-autonomy-model.md
  - ./autonomous-merge-review-gate.md
  - ../howto/diagnose-a-still-a-draft-merge-refusal.md
  - ../howto/triage-stale-pull-requests.md
  - ../design/overseer.md
---

# Merge draft-state re-validation

> **Operator symptom (issues [#4344](https://github.com/rysweet/Simard/issues/4344)
> / [#4145](https://github.com/rysweet/Simard/issues/4145)):** the same GREEN,
> non-draft, MERGEABLE PRs were re-escalated to the operator on **13 consecutive
> Overseer ticks** (05:30Z–10:34Z, ~5 h) and never merged. The merge attempt on
> PR #4336 aborted with **"Pull Request is still a draft"** even though GitHub
> reported the PR as **non-draft and mergeable**. The autonomous merge path was
> stalled: it looped escalating instead of landing merge-ready work.

The [merge authority](../reference/cross-repo-merge-authority.md) evaluates a set
of deterministic **objective gates** (base-branch allowlist → mergeable → CI
green) and then an agentic **merge-judge** before squash-merging a PR. The stall
had a precise root cause: **draft state was never a gate**.

## The seam: a fact the gates never checked

The pre-fix pipeline snapshotted the PR with

```text
gh pr view <PR> --json body,statusCheckRollup,mergeable,reviewDecision,baseRefName,labels
```

`isDraft` was **not** in that field list, so `PrSnapshot` carried no draft fact
and `evaluate_objective_gates` had nothing to check. The gates therefore
**passed** for a PR the pipeline believed was ready — and the merge decision was
handed to `gh pr merge`, which does its own server-side draft check at mutation
time. When that server-side check saw a stale or racy draft flag it aborted with
`Pull Request is still a draft`.

That abort surfaced as an **`Err`** ("could not merge"), not a
`MergeOutcome::Refused` with an actionable reason. The Overseer's only response to
an un-mergeable-but-un-refused PR is to **escalate to the operator** — and because
nothing about the PR changed tick-to-tick, it escalated the *same* PR again on the
*next* tick. One missing fact turned a merge-ready PR into a 13-tick escalation
loop.

## Why this is a false positive

GitHub reported PR #4336 as non-draft and mergeable. The `gh pr merge` abort was
acting on **stale PR state** — a draft flag read at one moment and merged at
another, across a window in which the PR had already been marked ready-for-review.
The pipeline had no way to tell "genuinely still a draft" from "the merge tool
raced a stale draft flag", so it treated a transient race as a hard,
never-clearing failure.

## The fix: make draft a gate, evaluated against the pre-merge snapshot

The [merge draft gate](../reference/merge-draft-gate-api.md) closes the seam with
two deliberately additive changes:

1. **`isDraft` is now a first-class fact.** It is added to the `gh pr view --json`
   field list and parsed into a new `PrSnapshot.is_draft: bool`
   (`#[serde(default, rename = "isDraft")]`, so absent or malformed JSON degrades
   to `false` rather than panicking). Because `PrSnapshot` derives `Default`, the
   new field can be filled with `..Default::default()`; note Rust struct literals
   are total, so the ~17 existing `PrSnapshot { … }` fixture/caller sites each add
   `is_draft: false` (a mechanical, non-draft-defaulting update) rather than
   "compiling unchanged".

2. **A fail-closed draft gate, evaluated against the existing pre-merge snapshot.**
   A new draft gate refuses the merge when `is_draft == true`; the separate,
   pre-existing mergeable gate (Gate 1) still independently requires `mergeable ==
   "MERGEABLE"`. The draft gate is AND-composed into `evaluate_objective_gates` —
   after the base-branch allowlist and before the mergeable/CI gates — so it can
   only ever *remove* a merge, never authorise one the other gates would block.
   The objective gates are evaluated against the PR snapshot the merge path
   already fetches immediately before the merge mutation
   (`merge_authority.rs:824`), so the gate reasons about *current* draft state; no
   extra re-fetch is introduced.

Together these turn the failure mode inside-out. A genuinely-draft PR is now
**`Refused`** with a single actionable reason ("PR is still a draft") — an
expected, quiet outcome that does **not** trigger operator escalation. A
non-draft, mergeable PR passes the gate against fresh state and **merges**. The
`gh pr merge` "still a draft" abort is no longer reachable for a PR the pipeline
just confirmed non-draft, because the decision and the mutation now read the same
fresh snapshot.

## Escalate only when genuinely non-mergeable

The behavioural contract after the fix:

| PR state (fresh snapshot)        | Outcome                                   | Operator escalation? |
| -------------------------------- | ----------------------------------------- | -------------------- |
| non-draft + `MERGEABLE` + green  | `Merged`                                  | no — it merged       |
| `isDraft == true`                | `Refused` ("PR is still a draft")         | no — quiet refusal   |
| `mergeable != "MERGEABLE"`       | `Refused` ("mergeable status is …")       | no — quiet refusal   |
| could not evaluate (`gh` failed) | `Err`                                     | yes — genuine block  |

The Overseer escalates a PR to the operator only when it is **genuinely
non-mergeable** or could not be evaluated at all — never because a mergeable PR
tripped a stale-draft race. That is the difference between a merge path that lands
work and one that re-announces the same GREEN PR forever.

## Relationship to the autonomy model

The draft gate is a **preserved safety gate**, not a weakening of autonomy. As
with every gate in the [operational autonomy model](./operational-autonomy-model.md),
it can only ever *remove* a merge from the autonomous path — it can never
authorise a merge the base-allowlist, mergeable, CI, or merge-judge gates would
otherwise block. Fail-closed on ambiguity (absent/malformed `isDraft` ⇒ treated as
not-ready by the mergeable check chain) means a crafted or missing field can never
turn a not-ready PR into a merge.

## Invariants

- **Draft is a first-class fact.** `PrSnapshot.is_draft` is parsed from
  `isDraft`; absent/malformed JSON degrades to `false` without panicking.
- **Fail closed.** The merge proceeds only when `is_draft == false` **and**
  `mergeable == "MERGEABLE"` **and** every pre-existing gate passes.
- **Fresh state at the gate.** Objective gates run against the PR snapshot the
  merge path already fetches immediately before the merge mutation
  (`merge_authority.rs:824`), so the decision and the mutation see the same draft
  state (no TOCTOU stale-draft abort). No additional re-fetch is added.
- **Draft ⇒ quiet Refused, not Err.** A genuinely-draft PR is `Refused` with an
  actionable reason and does **not** escalate to the operator; escalation is
  reserved for genuinely non-mergeable PRs and evaluation failures.
- **Additive / non-breaking.** `PrSnapshot` derives `Default`; the new gate is
  AND-composed and never short-circuits or bypasses an existing gate.

## Related reading

- [Merge draft gate API reference](../reference/merge-draft-gate-api.md) — the
  `PrSnapshot.is_draft` field, the `isDraft` JSON wiring, the gate ordering, and
  the single pre-merge snapshot the gate reasons against (no extra re-fetch).
- [Cross-repo merge authority reference](../reference/cross-repo-merge-authority.md)
  — the full objective-gates + merge-judge pipeline this gate slots into.
- [Diagnose a "still a draft" merge refusal](../howto/diagnose-a-still-a-draft-merge-refusal.md)
  — the operator playbook for the symptom.
- [Operational autonomy model](./operational-autonomy-model.md) — when
  and why Simard self-merges without a human approver.
