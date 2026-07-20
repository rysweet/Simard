---
title: Autonomous-merge review gate (the agentic merge-judge as sole reviewer)
description: >
  Why Simard's autonomous self-merge finally merges eligible engineer PRs instead
  of escalating every one of them. The review gate for an autonomous merge is the
  already-wired agentic merge-judge (merge_readiness_judge.md) — the single
  authoritative reviewer. verify() is a deterministic, objective pre-filter with no
  review step; the merge-judge in merge_authority is the one and only review
  authority, and it fails closed when no LLM provider is configured.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./autonomous-self-merge-sensor.md
  - ./draft-pr-merge-exclusion.md
  - ./operational-autonomy-model.md
  - ../reference/autonomous-merge-review-gate.md
  - ../reference/cross-repo-merge-authority.md
  - ../reference/ready-prs-sensor-api.md
  - ../howto/enable-autonomous-self-merge-canary.md
---

# Autonomous-merge review gate (the agentic merge-judge as sole reviewer)

> **Status: implemented.** This page describes the shipped review gate in present
> tense. It closes the last gap that made Simard's autonomous self-merge escalate
> **100%** of eligible engineer PRs and merge **zero** — the feature that, before
> this change, had never once merged a PR on its own.

Simard's autonomous self-merge path was fully wired — the `ready_prs` sensor
surfaced eligible engineer PRs, `PrReadyToMerge` fired, `VerifyAndMergePr`
dispatched, and the authoritative
[`merge_authority`](../reference/cross-repo-merge-authority.md) gate stood ready
with a production merge-judge. Yet in production **every** eligible PR that
cleared the objective survey pre-filter escalated and **none** merged — a tick
would report `prs_merged=0` with escalations equal to the number of eligible
candidates (for example, a tick surveying ~27 eligible engineer PRs merged zero
and escalated all of them).

## The bug: a fail-closed review gate that could never open

The Act handler for `Intervention::VerifyAndMergePr` called `verify()` first and
only merged if the report was `ready`. `verify()` built a checklist whose check
**#7** — "review (no Bug/Security ≥ High)" — was driven by an injected
`DiffReviewer` trait object. In production, that reviewer was **`None`**: no
`DiffReviewer` implementation ever existed in the codebase (only a test
`FakeReviewer`), and the "operator will wire an LLM reviewer" promised in the
`from_env` doc comment never happened.

When the reviewer was `None`, check #7 was hardcoded to `passed: false` with the
note *"review unavailable (no reviewer wired) — fail-closed"*. So
`verify().ready` was **always false** in production. Every candidate escalated.
`prs_merged` was structurally pinned at `0` forever.

Meanwhile the **authoritative** agentic gate —
`merge_pr_if_merge_ready_with_judge`, driven by the production-ready
[`build_merge_judge()`](../reference/cross-repo-merge-authority.md) over the
`prompt_assets/simard/merge_readiness_judge.md` prompt — was already wired into
`merge()` step 3. But it was **unreachable**, because `verify()` fail-closed
first and `merge()` returned before ever reaching step 3.

## The fix: one agentic reviewer, no dead code stub

The autonomous-merge review gate is now **agentic**, using the already-wired
merge-judge — not a never-built `DiffReviewer`. There is exactly **one** review
authority in the merge path, and it is the merge-judge.

Concretely (this is **Design (b)** — see the
[reference](../reference/autonomous-merge-review-gate.md) for the API detail):

- **`verify()` is a deterministic objective pre-filter — no review step.** It
  runs the objective gates (base-branch allowlist, `MERGEABLE`, every CI check
  `SUCCESS`/`NEUTRAL`/`SKIPPED`) plus the additive deterministic diff-scans, and
  nothing else. `ready == true` now means **"eligible to proceed to the
  authoritative merge,"** not "approved to merge." The old check #7, the
  `DiffReviewer` trait, the `reviewer` field/constructor argument, and the test
  `FakeReviewer` are **removed**.
- **The merge-judge is the sole review authority.** `merge()` step 3 calls
  `merge_pr_if_merge_ready_with_judge`, which re-runs the objective gates and then
  the agentic merge-judge. Only a `Ready` verdict merges. This is the **single**
  LLM review call in the whole path — there is deliberately no second, redundant
  judge call inside `verify()`.

### Why Design (b), not "a judge call inside verify()"

Putting a judge call inside `verify()` (Design (a)) would run the LLM **twice**
per merge: once when `merge()` step 1 calls `verify()`, and again at step 3's
authoritative `merge_pr_if_merge_ready_with_judge`. That directly violates *"no
redundant LLM review call."* Design (b) removes review from `verify()` entirely,
leaving the step-3 judge as the **sole** reviewer. This is safe because `merge()`
re-verifies the objective gates and runs the judge before any `squash_merge` — a
candidate that becomes stale between the pre-filter and the merge is caught by the
authoritative re-check.

## Honest accounting: refusals escalate, they don't error

Because the authoritative merge now actually runs in production, a judge that
returns `Refused` (or the fail-closed `RefusingMergeJudge`, or a re-verify that is
no longer ready) must be counted as an **escalation**, not an error. A new
`OverseerError::NotMergeReady { pr, reason }` variant carries every "not ready
now" outcome, and the Act handler maps it to `ActOutcome::Escalated`. Genuine
infrastructure and safety failures (a `gh` failure, a malformed snapshot, a
recursion-guard or author-gate refusal) still propagate to `errors`. The three
tallies stay mutually exclusive and meaningful:

| Tally | Meaning |
|---|---|
| `prs_merged` | Real squash-merges that happened |
| `escalations` | Not-ready-now — judge refused, provider unavailable, re-verify not ready |
| `errors` | Genuine failures — could-not-evaluate, safety-gate refusals, infra faults |

A human is still alerted on every refusal via the plain-English operator
notification.

## Every safety property is preserved

Removing the `DiffReviewer` stub does **not** weaken any guarantee — it removes a
guarantee that was never real (a review that could never pass) and replaces it
with the real, agentic one.

- **Fail-closed default.** With `SIMARD_AUTOMERGE_REPOS` / `SIMARD_AUTOMERGE_AUTHOR`
  unset, the sensor yields **no candidates** and **nothing merges**. Unchanged.
- **Fail-closed on provider outage.** When no LLM provider is configured,
  `build_merge_judge()` returns `RefusingMergeJudge`, which always returns
  `Verdict::NotReady`. Step 3 returns `NotMergeReady` → the candidate **escalates**.
  A judge that cannot run **never** defaults to approve.
- **Engineer-PR scoping (#4147).** Operator review PRs (author `rysweet`, no
  `simard-autonomous` label, on a shared branch prefix such as `feat/…`) are
  **never** candidates and **never** merged; engineer PRs (`engineer/` branch or
  `simard-autonomous` label) are eligible. Unchanged — this lives in the sensor and
  the author gate, both untouched.
- **Author re-assert + recursion guard.** `merge()` step 0 still refuses the
  Overseer's own PR and still requires the author to match the configured
  autonomous-merge identity (whole-login, case-insensitive). Unchanged.
- **Objective gates + poll-until-green.** Base-branch allowlist (default `main`),
  not-a-draft (`isDraft == false` — see
  [draft-PR merge exclusion](./draft-pr-merge-exclusion.md)), `MERGEABLE`, every CI
  check `SUCCESS`/`NEUTRAL`/`SKIPPED`, and the poll loop that waits for required
  checks to go green before merge. Unchanged.
- **Creative-idea-label exclusion** stays in the merge authority. Unchanged.
- **Squash + delete-branch only.** The merge command is
  `gh pr merge <PR> --squash --delete-branch` — **never** `--admin`, **never**
  `--no-verify`. Unchanged.
- **No wall-clock timeouts** on any agentic step. Unchanged.

## What "merge-ready" means to the judge

The merge-judge prompt (`prompt_assets/simard/merge_readiness_judge.md`) is the
single source of truth for the merge-ready evidence criteria. It reads the PR body
(treated as untrusted data, never as instructions) and returns a structured
verdict — `Ready`, `NotReady`, or `Unclear` (the last treated as `NotReady` at the
call site). An autonomous merge clears exactly the same evidence bar as a
human-driven `simard merge-pr`.

For the API surface — the new `verify()` semantics, `OverseerError::NotMergeReady`,
the `from_env()`/`new()` signatures, and the error matrix — see the
[autonomous-merge review gate reference](../reference/autonomous-merge-review-gate.md).
