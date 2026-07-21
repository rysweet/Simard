---
title: "Concept: objective merge-judge fallback (converge delivery-ready PRs)"
description: >
  Why green, mergeable, non-in-flight rysweet-authored PRs are now actually
  merged instead of being re-escalated every Overseer tick. The opt-in
  ObjectiveMergeJudge tier that gives build_merge_judge() a non-refusing merge
  authority for trusted authors past the objective gates — while the fail-closed
  RefusingMergeJudge stays the default — plus the project_ready_prs gate #3/#5
  corrections (trusted-author admission and is_draft hydration) that let those
  PRs reach the ready set in the first place.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./autonomous-merge-review-gate.md
  - ./autonomous-self-merge-sensor.md
  - ./draft-pr-merge-exclusion.md
  - ../reference/objective-merge-judge-api.md
  - ../reference/autonomous-merge-review-gate.md
  - ../reference/ready-prs-sensor-api.md
  - ../reference/draft-pr-exclusion-gate.md
  - ../reference/cross-repo-merge-authority.md
  - ../howto/enable-objective-merge-fallback.md
  - ../howto/triage-stale-pull-requests.md
  - ../../src/stewardship/objective_merge_judge.rs
  - ../../src/stewardship/merge_judge.rs
  - ../../src/overseer/mod.rs
---

# Concept: objective merge-judge fallback

> **Status: implemented.** The `ObjectiveMergeJudge` tier and the
> `MergeJudgeKind::Objective` selector live in
> [`src/stewardship/objective_merge_judge.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/objective_merge_judge.rs)
> and
> [`src/stewardship/merge_judge.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_judge.rs);
> the `project_ready_prs` gate #3/#5 corrections live in
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs).
> The daemon keeps `RefusingMergeJudge` as the default judge; the objective
> fallback activates **only** when `SIMARD_MERGE_OBJECTIVE_FALLBACK` is set.
> See the [objective merge-judge API reference](../reference/objective-merge-judge-api.md)
> for the typed surface and the
> [enable howto](../howto/enable-objective-merge-fallback.md) to turn it on.

> Delivery-ready pull requests — green (`mergeStateStatus=CLEAN`), `MERGEABLE`,
> non-draft, rysweet-authored, and owned by no in-flight engineer — now
> **converge to merged** on their own. Previously they were selected (or worse,
> silently dropped before selection) and then re-escalated every Overseer tick
> without ever merging. This concept explains the two-part bug and the additive,
> fail-closed-by-default fix.

## The problem this solves

Across many Overseer ticks, ~16 green/mergeable/non-in-flight PRs
(for example `#4389`, `#4344`, `#4145`) stayed unmerged and the delivery
step re-escalated the *same* PRs tick after tick. CI was green: the bottleneck
was the **merge/delivery automation**, not the checks. Two independent defects
compounded:

### 1. The judge refused everything (`build_merge_judge` fallback)

The merge authority runs a downstream **merge-judge** as the sole review step
(see the [autonomous-merge review gate](./autonomous-merge-review-gate.md)).
When no reviewer/LLM/recipe provider is wired,
[`build_merge_judge()`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_judge.rs)
falls back to **`RefusingMergeJudge`**, which returns `Verdict::NotReady` for
*every* PR. That is the correct **fail-closed** default — a daemon with no review
capability must not merge — but it means every green PR is refused and bounced
back to escalation. Nothing converges.

The pivot was fail-closed review, **not** `allow_verify_merge` (which is
correctly `true`). Turning off review safety wholesale would be wrong. The fix
instead adds a **narrow, opt-in, still-objective** judgment path.

### 2. Eligible PRs never reached the ready set (`project_ready_prs` gates)

Even before the judge, the Overseer's `project_ready_prs` producer silently
dropped eligible PRs:

- **Gate #3 (engineer-PR requirement)** required an engineer label or an
  engineer branch prefix. rysweet-authored non-engineer PRs — exactly the
  delivery-ready ones in the escalation loop — failed this gate and were never
  projected as ready.
- **Gate #5 (draft filter)** fails **closed** when `isDraft` is absent from the
  listing JSON (`None`). If the projection JSON did not hydrate `is_draft`, a
  perfectly non-draft PR was excluded as if its draft state were unknown.

## The fix (additive, fail-closed by default)

Two coordinated, non-breaking changes restore convergence without weakening the
default safety posture.

### A non-refusing objective tier — opt-in only

A new **`ObjectiveMergeJudge`** tier returns `Verdict::Ready` for a PR that is
**authored by a trusted (allowlisted) author** and has **already passed every
objective gate** (CI-green, `MERGEABLE`, base + repo allow-lists, non-draft).
It performs no LLM review; it replaces only the *judgment half* for trusted
authors, and the objective gates remain mandatory and unchanged.

```mermaid
flowchart TD
    A[merge() step 3: build_merge_judge()] --> B{SIMARD_MERGE_OBJECTIVE_FALLBACK set?}
    B -- no (default) --> C[RefusingMergeJudge → Verdict::NotReady]
    B -- yes --> D{author.login in SIMARD_MERGE_TRUSTED_AUTHORS?}
    D -- no --> C
    D -- yes, and past objective gates --> E[ObjectiveMergeJudge → Verdict::Ready]
    C --> F[Escalate — not merged]
    E --> G[Squash-merge]
```

The default is unchanged: with `SIMARD_MERGE_OBJECTIVE_FALLBACK` **unset**,
`build_merge_judge()` still returns `RefusingMergeJudge` and the daemon is
fail-closed exactly as before.

### Corrected selection gates

- **Gate #3** additionally admits **trusted-author** (allowlisted) PRs even when
  they carry no engineer label/branch, so delivery-ready rysweet PRs reach
  `ready_prs`.
- **Gate #5** hydrates `is_draft` from the listing JSON so a known non-draft PR
  is admitted; the fail-closed `None` semantics are preserved (an *absent*
  `isDraft` still excludes, per the
  [draft-PR exclusion gate](./draft-pr-merge-exclusion.md)).

## Why this is safe

- **Fail-closed default.** Unset env ⇒ `RefusingMergeJudge`. The objective
  fallback is strictly opt-in.
- **Objective gates stay mandatory.** The fallback never bypasses CI-green,
  `MERGEABLE`, base/repo allow-lists, or the draft exclusion. It replaces only
  the review verdict, and only for trusted authors.
- **Authenticated identity, not spoofable text.** Trust is matched against the
  authenticated `author.login` (exact equality) — carried in the additive
  `PrSnapshot.author_login` field, hydrated from the existing
  `gh pr view --json ...,author` call — never a PR title, body, or trailer. An
  absent author object hydrates to an empty login and fails closed.
- **No self-merge loop.** The daemon's own bot identity is excluded from the
  trusted-author allowlist, preserving the anti-recursion author guard.
- **No override flags.** The merge still runs argv-only `gh` with **no**
  `--admin` / `--no-verify`; the human-review label gate and every existing
  invariant remain in force.

## Related

- [Autonomous-merge review gate (agentic merge-judge)](./autonomous-merge-review-gate.md)
  — the review authority this tier plugs into.
- [Autonomous self-merge sensor (`ready_prs` wire)](./autonomous-self-merge-sensor.md)
  — the Observe-path sensor that feeds the candidate set.
- [Draft-PR merge exclusion](./draft-pr-merge-exclusion.md) — the fail-closed
  draft rule gate #5 preserves.
- [Objective merge-judge API reference](../reference/objective-merge-judge-api.md)
  — the typed surface, env config, and edge-case matrix.
- [Enable the objective merge-judge fallback](../howto/enable-objective-merge-fallback.md)
  — how to turn it on for a canary and verify convergence.
