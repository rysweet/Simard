---
title: Done-gate slug dedup API reference
description: >
  The typed surface of the slug-keyed done-gate convergence that makes a
  completed goal converge on a single done-gate PR — the injected PR-lister seam,
  the sanitize_goal_slug() helper ([a-z0-9-]), the keep-oldest-CLEAN selection,
  the bot-author-AND-exact-slug supersede scoping, the stale-CONFLICTING
  out-of-flight branch pruning, idempotency, and the edge-case matrix.
last_updated: 2026-07-21
owner: simard
doc_type: reference
status: partially implemented
related:
  - ../concepts/done-gate-slug-convergence.md
  - ../concepts/deploy-aware-done-gate.md
  - ./completion-evidence-gate-api.md
  - ./goal-board-api.md
  - ../howto/triage-stale-pull-requests.md
  - ../../src/goal_curation/completion_gate.rs
  - ../../src/goal_curation/advance_goal/spawn.rs
  - ../../src/goal_curation/advance_goal/goal_session.rs
---

# Done-gate slug dedup API reference

> **Status: partially implemented.** The convergence, the `sanitize_goal_slug()`
> helper, and the ownership-scoped supersede decision below are implemented and
> unit-tested (over an injected PR-lister) in
> [`src/goal_curation/completion_gate.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs).
> Wiring these decisions into the advance-goal spawn/session path
> ([`advance_goal/spawn.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/advance_goal/spawn.rs),
> [`advance_goal/goal_session.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/advance_goal/goal_session.rs))
> is a tracked follow-up and is **not yet integrated**. This page specifies the
> typed surface that wiring will consume.

This reference specifies the API of the slug-keyed done-gate convergence. For
the *why* and the safety narrative, see
[the done-gate slug convergence concept](../concepts/done-gate-slug-convergence.md).

**One-line summary:** given a goal slug, the done-gate keeps the **oldest
`CLEAN`** bot-authored done-gate PR matching that slug and supersedes/closes the
rest (bot-author **and** exact-slug scoped); it prunes stale
`CONFLICTING`/`DIRTY` engineer branches for out-of-flight slugs by logic.

## Contents

- [`PrLister` seam](#prlister-seam)
- [`sanitize_goal_slug()`](#sanitize_goal_slug)
- [`converge_done_gate_prs()`](#converge_done_gate_prs)
- [Supersede scoping](#supersede-scoping)
- [Idempotency](#idempotency)
- [Edge-case matrix](#edge-case-matrix)

## `PrLister` seam

Convergence is written against an **injected** PR-lister so it is unit-testable
without live `gh`. Production wires the `gh`-backed implementation.

```rust
pub struct OpenPr {
    pub number: u32,
    pub author_login: String,       // authenticated GitHub login
    pub head_branch: String,        // used to match the goal slug
    pub merge_state: MergeState,    // Clean | Conflicting | Dirty | Unknown
    pub created_at: OffsetDateTime, // "oldest CLEAN" tiebreak
    pub is_done_gate: bool,         // done-gate PR vs. engineer branch
}

pub trait PrLister: Send + Sync {
    /// Open PRs whose head branch carries the given sanitized slug prefix.
    fn list_for_slug(&self, repo: &str, slug: &str) -> SimardResult<Vec<OpenPr>>;
}
```

## `sanitize_goal_slug()`

The goal slug is sanitized **before** any branch/argv/path use. Only
`[a-z0-9-]` survives; everything else (uppercase, whitespace, `.`, `/`, `..`,
shell metacharacters) is stripped or rejected.

```rust
/// Lowercase; keep only [a-z0-9-]; collapse repeated '-'; trim leading/trailing '-'.
/// Returns None for an empty result (⇒ convergence is skipped, no unscoped close).
pub fn sanitize_goal_slug(raw: &str) -> Option<String> { /* … */ }
```

An empty sanitized slug **skips** convergence entirely — the gate never falls
back to an unscoped supersede.

## `converge_done_gate_prs()`

```rust
pub struct ConvergeOutcome {
    pub kept: Option<u32>,        // the single surviving done-gate PR
    pub superseded: Vec<u32>,     // done-gate PRs closed as duplicates
    pub pruned_branches: Vec<u32>,// stale CONFLICTING/DIRTY out-of-flight branches
}

/// Converge the open PRs for one goal slug onto a single done-gate PR.
/// - keeps the OLDEST Clean done-gate PR authored by `bot_login`,
/// - supersedes/closes the remaining bot-authored, exact-slug done-gate PRs,
/// - prunes stale Conflicting/Dirty engineer branches when the slug is NOT in `inflight`.
/// Pure decision + scoped side-effects; no `--admin`, argv-only `gh`.
pub fn converge_done_gate_prs(
    lister: &dyn PrLister,
    repo: &str,
    slug: &str,               // caller passes the sanitized slug
    bot_login: &str,
    inflight: &InflightRefs,
) -> SimardResult<ConvergeOutcome> { /* … */ }
```

**Selection rule.** Among done-gate PRs (`is_done_gate == true`) with
`merge_state == Clean`, keep the one with the earliest `created_at`. If none is
`Clean`, keep/open a single done-gate PR and supersede any other bot-authored
slug-matching done-gate PRs.

## Supersede scoping

A PR is superseded/closed **only** when **all** of the following hold — a
deliberately conjunctive guard so the fix can never touch a human or unrelated
PR:

1. `pr.author_login == bot_login` (Simard's bot identity), **and**
2. the PR's head branch matches the **exact sanitized slug** prefix, **and**
3. `pr.is_done_gate` (for the supersede path) or it is a stale
   `Conflicting`/`Dirty` engineer branch for an **out-of-flight** slug (for the
   prune path), **and**
4. it is not the kept PR.

Closes are argv-only `gh` with a supersede note referencing the kept PR — no
`--admin`, no force.

## Idempotency

Re-running convergence on an already-converged goal is a **no-op**: the single
kept PR is re-selected and there is nothing left to supersede or prune. This
makes the gate safe to run on every done-gate tick.

## Edge-case matrix

| Situation | Result |
|---|---|
| 3 competing `CLEAN` done-gate PRs, one slug | Oldest kept; other two superseded |
| No `CLEAN` done-gate PR | Keep/open one; supersede other bot+slug done-gate PRs |
| Human-authored PR matching slug | Never touched (author ≠ bot) |
| Bot PR for a **different** slug | Never touched (exact-slug scope) |
| Stale `CONFLICTING` engineer branch, slug **in** flight | Left alone (goal still active) |
| Stale `CONFLICTING` engineer branch, slug **out of** flight | Pruned |
| Empty/invalid slug after sanitization | Convergence skipped (no unscoped close) |
| Already converged (single kept PR) | No-op (idempotent) |

## Telemetry

Each convergence emits a structured `tracing` event (OTel) with the kept PR,
superseded set, and pruned branches. No `println!`, no secrets, and the raw
(unsanitized) slug is never used in an argv or path.
