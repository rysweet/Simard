---
title: Draft-PR merge exclusion (the merge-queue draft rail)
description: >
  Why Simard's autonomous merge-queue never attempts to merge a draft pull
  request. A thin, deterministic guardrail that carries each PR's isDraft state
  through the ready-PR sensor and projection, then EXCLUDES any draft (or any PR
  whose draft state is unknown) from the ready-PR candidate set — closing the
  #4339 bug where a CLEAN/MERGEABLE draft (PR #4336) was admitted every tick and
  then deterministically failed `gh pr merge` with "Pull Request is still a
  draft". Pure narrowing: it can only remove candidates, never broaden merge
  eligibility.
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./autonomous-self-merge-sensor.md
  - ./agentic-merge-queue-reasoning.md
  - ./operational-autonomy-model.md
  - ../reference/draft-pr-exclusion-gate.md
  - ../reference/ready-prs-sensor-api.md
  - ../reference/cross-repo-merge-authority.md
  - ../howto/enable-autonomous-self-merge-canary.md
---

# Draft-PR merge exclusion (the merge-queue draft rail)

> **Status: implemented.** This page describes the shipped guardrail in present
> tense. It closes issue #4339 in the agentic observe/orient merge-queue
> introduced by #4097 (autonomous self-merge sensor) and #1880 (Merge Readiness
> objective gates).

## The bug this closes

GitHub will **never** merge a draft pull request. `gh pr merge` on a draft fails
deterministically, server-side:

```
GraphQL: Pull Request is still a draft (mergePullRequest)
```

Before this rail, Simard's merge-queue could not *see* a PR's draft state. A
draft PR that was otherwise clean — `mergeStateStatus=CLEAN`,
`mergeable=MERGEABLE`, CI green, correct author, correct engineer marker — passed
every objective gate and was admitted to the ready-PR candidate set. The acting
Overseer then spent its per-tick merge attempt on it, and the attempt failed
every single tick:

```
WARN overseer::tick: overseer intervention failed — isolated, continuing
  intervention="verify_and_merge_pr"
  error=capability merge failed: merge-authority: gh command failed:
  `gh pr merge 4336 --repo rysweet/Simard --squash --delete-branch`
  exited exit status: 1: GraphQL: Pull Request is still a draft (mergePullRequest)
```

Because the draft (PR #4336) sorted to the front of the candidate set every
tick, the merge-queue burned its one merge attempt on a PR that could never
merge and never advanced to genuinely-ready non-draft PRs. `prs_merged` stayed
pinned at **0**.

The root cause was purely one of *visibility*: the ready-PR sensor and the
merge-authority listing never fetched `isDraft`, so the objective gates could not
consider it. This rail fetches it and excludes drafts.

## What the rail does

Draft-ness is an **objective fact**, not a judgment call, so it is enforced as a
small deterministic rail — **not** routed through the agentic reasoning step. In
one sentence: **a draft pull request is never a merge candidate, and a PR whose
draft state is unknown is treated as a draft (excluded).**

Concretely, the rail:

1. **Fetches `isDraft`.** The two `gh pr list` `--json` field sets the ready-PR
   producers consume now include `isDraft`, so draft state reaches the objective
   pre-filter instead of being invisible. (`gh pr view` also requests `isDraft`
   for snapshot completeness, but its result never feeds the pre-filter and does
   not touch `PrSnapshot` — see the reference.)
2. **Carries it through the data plane.** The `isDraft` boolean rides on the
   open-PR summary and on the projection candidate — the two shapes the two
   ready-PR producers consume — so no second `gh` round-trip is needed.
3. **Excludes drafts in BOTH producers.** A deterministic exclusion is applied in
   the sensor (`survey_ready_prs`) *and* in the projection (`project_ready_prs`),
   so the invariant holds wherever `ready_prs` is produced. A candidate is
   admitted **only if** its draft state is explicitly `false`.

## Fail-closed: unknown draft state is treated as draft

The rail admits a PR **only** when its draft state is known-and-`false`. If the
`gh` listing omits `isDraft` (an older `gh`, a partial JSON, a field that GitHub
stops returning), the draft state parses as *unknown* — and unknown is
**excluded**, exactly like a real draft.

| `isDraft` from `gh` | Draft state | Ready-PR candidate? |
|---|---|---|
| `false` | known: not a draft | **admitted** (subject to all other gates) |
| `true` | known: draft | **excluded** |
| absent / null | unknown | **excluded** (fail-closed) |

This mirrors the existing fail-closed posture in `survey_ready_prs` (an unset
author, a `gh` error, a missing label all yield exclusion, never a merge). A
genuinely-ready PR whose `isDraft` was momentarily missing is simply re-evaluated
on the next tick once the field is present again — the cost of fail-closed here
is at most one deferred tick, never a wrong merge.

## Pure narrowing — it can only remove candidates

This rail is a **monotone narrowing** of the candidate set. Adding it can only
ever *remove* PRs from `ready_prs`; it can never admit a PR the previous gates
would have rejected, and it never broadens auto-merge eligibility in any way. A
draft can never be merged, so excluding it removes only work that was guaranteed
to fail.

Every pre-existing gate is preserved and unchanged:

- **G2 author filter** — whole-login, case-insensitive match against
  `SIMARD_AUTOMERGE_AUTHOR`.
- **G3 engineer-PR gate** — the `simard-autonomous` label (primary) OR an
  engineer-exclusive branch namespace (secondary).
- **Objective gates** — base-branch allowlist + `MERGEABLE` + all checks green.
- **MergeJudge** — the authoritative six-criteria evidence gate in
  `merge_authority`, still the single source of merge truth.

The draft rail sits alongside G2/G3 as one more deterministic narrowing filter.
It touches neither `PrSnapshot` nor `evaluate_objective_gates`, so the #1880
operator-dashboard Merge Readiness panel — which shares
`evaluate_objective_gates` — is unaffected.

## Defense-in-depth, not the only line

Even without this rail, GitHub's server-side `mergePullRequest` refuses a draft —
that is the authoritative, TOCTOU-safe boundary and it is unchanged. The rail is
a **local, pre-emptive** narrowing that stops Simard from *wasting* a merge
attempt (and emitting a recurring journal error) on a PR the server would reject
anyway. The server remains the final authority; the rail keeps the merge-queue
efficient and the journal clean.

## Intended effect

After this rail deploys, the acting Overseer no longer attempts to merge draft
PRs. The recurring `Pull Request is still a draft` failure disappears from the
daemon journal, the per-tick merge attempt is spent on a genuinely-ready
non-draft PR, and `prs_merged` is free to advance above 0.

For the exact field sets, struct fields, and per-producer placement, see the
[draft-PR exclusion gate reference](../reference/draft-pr-exclusion-gate.md).
