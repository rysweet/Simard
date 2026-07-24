---
title: The `ooda-stuck` escalation self-heals the missing label instead of failing silently
description: Why a blocked OODA goal can always be escalated to the operator through the GitHub-issue channel — even when the `ooda-stuck` label does not exist in the target repo. The three breaker/safeguard filers now idempotently ensure the label (create-if-missing) before filing, and gracefully degrade to filing the issue WITHOUT the label (with a structured WARN) when the label cannot be created, so `gh issue create` never exits non-zero on an unknown label and the escalation always succeeds. Preserves one-issue-per-stall idempotency; additive and non-breaking (issue #4474).
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ../reference/ooda-stuck-label-self-heal-api.md
  - ../reference/no-progress-breaker-api.md
  - ../reference/no-progress-root-cause-resolution-api.md
  - ./no-progress-root-cause-resolution.md
  - ./blocked-goal-escalation-backoff.md
  - ./steerable-ooda-daemon.md
  - ../howto/diagnose-a-no-progress-block.md
  - ../reference/goal-labels.md
  - ../../src/stewardship/gh_client.rs
  - ../../src/ooda_loop/no_progress.rs
  - ../../src/ooda_actions/advance_goal/spawn.rs
---

# The `ooda-stuck` escalation self-heals the missing label

> **Status: implemented (issue #4474).** The three sites that file an
> `ooda-stuck` tracking issue — the no-progress breaker production filer, the
> brain-failure deterministic safeguard, and the engineer-lifecycle
> open-tracking-issue path — now route their label handling through a single
> shared helper,
> [`ensure_label`](https://github.com/rysweet/Simard/blob/main/src/stewardship/gh_client.rs),
> in `src/stewardship/gh_client.rs`. The helper idempotently creates the label
> if it is missing and, when it cannot, tells the caller to file the issue
> **without** the label rather than let `gh` fail. Primary sources:
> [`src/stewardship/gh_client.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/gh_client.rs)
> (`ensure_label`, `LabelDisposition`, `LabelEnsureExecutor`),
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs)
> (`GhIssueFiler`), and
> [`src/ooda_actions/advance_goal/spawn.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs)
> (the deterministic safeguard and open-tracking-issue sites). API details:
> [`ooda-stuck` label self-heal reference](../reference/ooda-stuck-label-self-heal-api.md).

## The defect this fixes

When the OODA no-progress breaker fires for a goal it cannot un-stick, its last
resort is to escalate to a human by filing a GitHub tracking issue. Every filer
tagged that issue with the `ooda-stuck` label:

```bash
gh issue create --title "…" --body "…" --label ooda-stuck
```

But the `ooda-stuck` label **did not exist** in `rysweet/Simard`. `gh issue
create` treats an unknown `--label` as a hard error and exits non-zero:

```
could not add label: ooda-stuck not found
```

Because the filer only ever checked the exit status, a non-zero exit meant the
issue was **never created** — the escalation silently failed. The breaker had
already marked the goal `Blocked` with its sentinel, but no issue reached the
operator, and the goal stayed parked with no visible escalation artifact.

The journal signature (`simard::ooda`, `ERROR`) recorded the fingerprint:

```
no-progress breaker: gh issue create failed (goal still Blocked)
  stderr=could not add label: ooda-stuck not found
```

This recurred every ~45 minutes as the breaker re-fired across OODA cycles
2430 / 2433 / 2436 / 2439 (five occurrences, 15:47 → 21:10). The blocked-goal
escalation channel through GitHub issues was, in practice, dead.

Three call sites shared the defect, each with its own subtlety:

| Site | File | Prior behaviour |
| --- | --- | --- |
| No-progress breaker production filer | `src/ooda_loop/no_progress.rs` (`GhIssueFiler::file_issue`, ~line 116) | `--label ooda-stuck` → non-zero exit → `ERROR` log, returned `None`, goal stayed `Blocked` with no linked issue. |
| Brain-failure deterministic safeguard | `src/ooda_actions/advance_goal/spawn.rs` (~lines 378–379) | `--label ooda-stuck` → non-zero exit → `ERROR` log; the safeguard-`Blocked` mark survived but no issue was filed. |
| Engineer-lifecycle open-tracking-issue | `src/ooda_actions/advance_goal/spawn.rs` (~lines 935–936) | `--label ooda-stuck` **and** used `.status()`, which drops captured streams — a non-zero exit produced **no** log at all (a second, latent silent-failure bug). |

## The fix: ensure-or-degrade, never fail on a missing label

The fix is a single, shared label-handling seam that both **self-heals** the
missing label and **fail-safe degrades** when it cannot — so the escalation
issue is filed either way.

### 1. Ensure the label idempotently (create-if-missing)

Before filing, each site calls `ensure_label("ooda-stuck")`. The helper runs:

```bash
gh label create ooda-stuck
```

> **Design decision — repo scoping (ambient, not `-R`).** `ensure_label` uses
> the same ambient repository context (current working directory) as the
> sibling `gh issue create` call at each site — it deliberately does **not**
> pass `-R <repo>`. This is both simpler and *more correct*: the original
> failure signature was `could not add label: ooda-stuck not found`, which
> proves `gh issue create` had already resolved the repo from the ambient cwd
> and failed *only* on the label. Creating the label in that same ambient repo
> guarantees the label and the issue always target the identical repository. An
> earlier draft of the design passed an explicit `-R <repo>` slug to
> `gh label create`; that was rejected because (a) none of the three call sites
> has a validated `owner/repo` slug in scope — the deterministic safeguard runs
> *before* `goal_repo_slug` is resolved, and neither `apply_lifecycle_decision`
> nor `GhIssueFiler` receives a repo — and (b) resolving a slug independently
> risks creating the label in a *different* repo than `gh issue create` targets.
> Keeping both commands ambient sidesteps both problems and keeps the change
> additive (no repo threading, no trait/struct signature change).

- **Exit 0** → the label now exists → attach it.
- **"already exists"** (case-insensitive stderr match) → the label was already
  there → attach it. This is what makes the call idempotent: a label created on
  the first stall is simply re-observed on the next, never re-created, never an
  error.

This is the create-if-missing self-heal: the first time any goal stalls in a
repo without the label, Simard creates the label once and every subsequent
escalation reuses it.

### 2. Degrade gracefully when the label can't be created

A token that can *file issues* may not have permission to *create labels*
(label creation needs repo-write). If `gh label create` fails for any
reason other than "already exists" — an authorization error, a spawn failure —
the helper does **not** propagate an error and does **not** abort the
escalation. It returns a `LabelDisposition::Omit { reason }`, and the caller
files the issue **without** the `--label` argument:

```bash
gh issue create --title "…" --body "…"   # no --label; escalation still succeeds
```

The degradation is always surfaced with a structured `tracing::warn` on the
site's own target (`simard::ooda` or `simard::ooda_brain`), carrying the reason.
This honours the **no-silent-fallback** rule: the issue is still filed, but the
operator can see in the logs that the label was omitted and why.

### 3. Stop swallowing exit codes at the open-tracking-issue site

The engineer-lifecycle site (`spawn.rs` ~line 935) previously used `.status()`,
which discards captured stdout/stderr and — combined with only logging the
`Err(_)` spawn case — meant a non-zero `gh` exit produced no diagnostics at all.
It now uses `.output()` and inspects the exit status, emitting a structured
`warn` with the lossy-decoded, length-bounded (≤ 2 KiB) stderr on failure. This
closes the latent second bug so that site can never silently fail again either.

## What is preserved

- **One issue per stall (idempotency).** The self-heal changes only *label*
  handling. The dedup that guarantees a stalled goal is escalated exactly once —
  the breaker-authored `[no-progress-tracking]` `WipRef` link and the
  `already_tracked` check in the escalation path — is untouched. A re-stall
  never spams duplicate `ooda-stuck` issues. See
  [the no-progress breaker API](../reference/no-progress-breaker-api.md) and
  [blocked-goal escalation backoff](./blocked-goal-escalation-backoff.md).
- **Existing tracing targets and return contracts.** Each caller keeps its own
  target (`simard::ooda` vs `simard::ooda_brain`) and return type
  (`Option<FiledIssue>` vs unit). Only the label concern is centralised.
- **The `eprintln!` operator line** at the deterministic-safeguard success path
  (`spawn.rs` ~line 384) is unchanged; no new `print!`/`println!` is introduced.
- **argv-only, no-shell invocation.** The label is a hardcoded constant (no
  leading `-`) and no repo argument is added, so the new `gh label create`
  subprocess call is as injection-safe as the existing ones — there is no
  attacker-influenced argv value at all.

## End-to-end effect

A blocked goal is now **always** escalable through the GitHub-issue channel:

1. Breaker fires, marks the goal `Blocked` with its sentinel.
2. `ensure_label` creates `ooda-stuck` on the first stall (or observes it, or
   degrades to no-label with a `WARN`).
3. `gh issue create` succeeds — with the label when possible, without it when
   not — and the operator receives the tracking issue.
4. The filed issue is linked back to the goal as its
   `[no-progress-tracking]` artifact, and the dedup guard prevents any duplicate
   on a re-stall.

The `could not add label: ooda-stuck not found` error class can no longer break
escalation.

## See also

- [`ooda-stuck` label self-heal API reference](../reference/ooda-stuck-label-self-heal-api.md)
- [No-progress breaker API reference](../reference/no-progress-breaker-api.md)
- [Concept: the no-progress breaker explains WHY and self-resolves before escalating](./no-progress-root-cause-resolution.md)
- [How to diagnose a no-progress block](../howto/diagnose-a-no-progress-block.md)
- [Goal labels / tags API reference](../reference/goal-labels.md)
