---
title: Draft-PR merge exclusion (the isDraft objective gate)
description: >
  Why Simard's autonomous self-merge never merges — and never re-tries merging — a
  pull request GitHub reports as a draft. Draft status (`isDraft == true`) is a
  first-class objective merge gate: a draft PR is excluded from the delivery-ready
  candidate set AND short-circuited defensively before `gh pr merge` is ever
  invoked, so the overseer tick-loop can no longer burn cycles retrying
  "Pull Request is still a draft".
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./autonomous-self-merge-sensor.md
  - ./autonomous-merge-review-gate.md
  - ./operational-autonomy-model.md
  - ../reference/draft-pr-merge-gate.md
  - ../reference/cross-repo-merge-authority.md
  - ../howto/enable-autonomous-self-merge-canary.md
---

# Draft-PR merge exclusion (the `isDraft` objective gate)

> **Status: implemented.** This page describes the shipped draft-exclusion gate in
> present tense. It closes the loop in which a GitHub draft PR that was otherwise
> `MERGEABLE` with all checks green was classified "green and merge-ready" every
> tick, then failed `mergePullRequest` with *"Pull Request is still a draft"* — a
> loop that burned overseer cycles for hours.

## The problem: `isDraft` was invisible to the merge pipeline

GitHub lets an author mark a pull request as a **draft** to signal *"not ready to
merge yet"* even when the branch is technically mergeable and CI is green. The
GraphQL `mergePullRequest` mutation refuses a draft outright:

```
GraphQL: Pull Request is still a draft (mergePullRequest)
```

Before this change, Simard's merge-readiness classifier keyed **only** on
`mergeable` and the CI check rollup. It never read `isDraft`. So a draft PR that
happened to be `MERGEABLE` with all required checks `SUCCESS` looked identical to
a truly merge-ready PR:

1. The `ready_prs` survey pre-filter classified the draft as a delivery-ready
   candidate.
2. `PrReadyToMerge` fired → `DeliveryReady` → `VerifyAndMergePr`.
3. The authoritative gate passed its objective + agentic checks.
4. `gh pr merge --squash` invoked `mergePullRequest`, which **refused** with
   *"still a draft"*.
5. Nothing about the PR changed, so the **next** tick observed the same draft,
   re-classified it merge-ready, and retried — indefinitely.

This is the reproduction shape (Simard PR #4336): `isDraft=true`,
`mergeable=MERGEABLE`, every verify check `SUCCESS`. The overseer looped on it from
~02:19Z to ~06:21Z, one failed merge attempt per 15-minute tick, burning cycles
that should have gone to real work.

## The fix: draft status is a first-class objective gate

`isDraft` is now carried end-to-end through the merge pipeline and enforced as an
**objective merge gate**, exactly like the base-branch allowlist, `mergeable`, and
the CI rollup. A draft PR is **never merge-ready**.

Enforcement has **two genuinely independent layers**, mirroring the existing
creative-idea-label exclusion, so no code path can call `mergePullRequest` on a
draft:

| Layer | Where | Effect |
|---|---|---|
| **Classification / objective gate** | `evaluate_objective_gates` | A draft PR fails the objective pass. Because the same function is evaluated at survey time *and* inside the authoritative merge fn, this single gate excludes drafts from the `ready_prs` survey pre-filter, from the dashboard's merge-readiness verdict, **and** from the merge fn's own pre-`squash_merge` check. It never becomes a `PrReadyToMerge` candidate, and even a direct caller is refused. **This is the layer that prevents the merge.** |
| **Defensive short-circuit + log** | `merge_pr_if_merge_ready_with_judge` | An explicit `is_draft` check placed **before** `evaluate_objective_gates` (next to the creative-idea skip) returns `MergeOutcome::Refused` and emits exactly one structured `reason="draft"` log line. It is *redundant* with the objective gate for merge prevention — its job is the dedicated, greppable draft log and resilience if the gate order is ever changed. |

The objective gate stops the loop at its source: a draft is never a candidate in
the survey, never merge-ready on the dashboard, and refused inside the merge fn.
The short-circuit is belt-and-suspenders — placed ahead of the objective gate so
its draft-specific log line wins, and so the skip survives any future reordering
of the objective gates.

### Where the draft gate sits in the objective pass

The draft gate is an **early objective gate**, evaluated right after the
base-branch allowlist and before/with the `MERGEABLE` gate, so a draft
short-circuits cheaply:

```
evaluate_objective_gates(snapshot):
  Gate 0  base-branch allowlist        (baseRefName ∈ SIMARD_MERGE_BASE_ALLOWLIST)
  Gate 1  NOT a draft                   (isDraft == false)          ← this change
  Gate 2  mergeable                     (mergeable == "MERGEABLE")
  Gate 3  CI green                      (every check SUCCESS/NEUTRAL/SKIPPED)
```

A draft PR fails Gate 1 with a single, actionable reason. Like every other
objective gate, the reason carries **no PR number** — `evaluate_objective_gates`
receives only a `PrSnapshot` (which has no `number` field), so it uses the literal
`<PR>` placeholder exactly as the base-branch gate does (`gh pr edit <PR> …`):

```text
PR is a draft (isDraft=true). Draft PRs are never auto-merged. Mark it ready for review before merging: `gh pr ready <PR>`.
```

## Where `isDraft` comes from

`isDraft` is a `bool` requested from GitHub via the same `gh pr view` / `gh pr
list` JSON path the other objective fields use, added to all three field lists:

- `view_pr` — `gh pr view <PR> --json ...,isDraft`
- `list_open_prs` — `gh pr list --json ...,isDraft` (dashboard panel)
- `list_prs_by_author` — `gh pr list --author <login> --json ...,isDraft`
  (the `ready_prs` survey)

It is parsed as a **strict `bool`** (`#[serde(default, rename = "isDraft")]`) and
threaded onto both [`PrSnapshot`] and [`OpenPrSummary`], with
`OpenPrSummary::to_snapshot()` propagating it into the snapshot the gate reads. The
`#[serde(default)]` keeps every existing snapshot constructed without the field
back-compatible: a snapshot with no `is_draft` defaults to `false` (not a draft),
preserving non-draft behavior exactly.

[`PrSnapshot`]: ../reference/cross-repo-merge-authority.md
[`OpenPrSummary`]: ../reference/cross-repo-merge-authority.md

## Safety posture

- **Additive & non-breaking.** Non-draft merge behavior is completely unchanged. A
  PR with `isDraft=false` flows through exactly as before. The gate only *removes*
  an unintended action (merging an author-marked not-ready PR); it never adds a
  merge.
- **Fail-closed, not fail-open.** A missing `isDraft` defaults to `false` **only
  for well-formed JSON**. The `gh` query still fails hard on a non-zero exit or
  empty stdout *before* serde parsing, so a truncated or errored response can never
  silently degrade a draft into a mergeable PR.
- **Strict typing.** `isDraft` binds to a real `bool` — no lenient string-to-bool
  coercion. It is consumed only in a conditional; it is never interpolated into
  shell arguments, format strings, or the merge command.
- **Structured logging only.** The short-circuit logs `pr_number` (a `u32`) and
  `reason="draft"` via `tracing` (with OTel) — never a token, PR body, branch
  contents, or raw `gh` JSON. No `print!`/`println!`. One line per skip, on the
  existing tick cadence — no per-tick spam.
- **The objective gate is the enforcement.** `evaluate_objective_gates` refuses a
  draft at both survey time and inside the merge fn, so no code path can call
  `mergePullRequest` on a draft. The pre-`squash_merge` short-circuit adds a
  dedicated `reason="draft"` log and defends against future gate reordering; it is
  redundant with the objective gate for merge prevention, not a substitute for it.

## What changed for the operator

Nothing to configure. The draft gate is unconditional and always on — a draft PR
is never merge-ready in any repo Simard governs, regardless of allowlist state.

- The overseer no longer loops on *"Pull Request is still a draft"*.
- A draft PR that is otherwise green appears in the dashboard's merge-readiness
  panel with a **not-ready** verdict whose reason names the draft status.
- To make such a PR eligible again, mark it ready for review:
  `gh pr ready <PR>`. On the next survey it re-enters the candidate set and is
  evaluated normally.

For the exact API — the `is_draft` field, the gate reason string, the
short-circuit `MergeOutcome`, and the serde contract — see the
[draft-PR merge gate reference](../reference/draft-pr-merge-gate.md).
