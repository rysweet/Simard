---
title: OODA breaker churn-suppression API reference
description: >
  Reference for the terminal-quarantine rung and reblock-issue signature
  stabilization that end the OODA no-progress breaker's `UNCLEAR-CRITERIA`
  churn. Specifies   the `NoProgressResolution::QuarantineTerminal` variant, the additive
  `surfaced_failures` argument on `resolution_for_why`, the durable
  `ooda-breaker-quarantine` marker and its predicate, the
  `apply_resolution_side_effects` handling that replaces the adapter's
  escalate-at-limit branch, the churn-stopping re-schedule exclusion in
  `reinvestigate_bare_blocked_goals`, and the `fold_volatile_goal_ids`
  stabilization applied in `problem_to_run_brief` that stops the Overseer's
  observed `recurring_goal_reblock in simard::overseer` stewardship-issue
  churn.
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/ooda-breaker-churn-suppression.md
  - ../concepts/no-progress-breaker-storm-suppression.md
  - ../concepts/no-progress-terminal-investigation.md
  - ../concepts/overseer-root-cause-why.md
  - ./no-progress-breaker-api.md
  - ./no-progress-root-cause-resolution-api.md
  - ./no-progress-breaker-storm-suppression-api.md
  - ../howto/quarantine-and-recover-an-unclear-ooda-goal.md
  - ../../src/goal_curation/no_progress_breaker.rs
  - ../../src/goal_curation/no_progress_why.rs
  - ../../src/ooda_loop/no_progress.rs
  - ../../src/overseer/observer.rs
  - ../../src/stewardship/dedup.rs
---

# OODA breaker churn-suppression API reference

> **Status: implemented.** The terminal-quarantine variant, the additive
> `surfaced_failures` argument on `resolution_for_why`, the quarantine-marker
> constants, and the quarantine predicate live in
> [`src/goal_curation/no_progress_breaker.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/no_progress_breaker.rs).
> The side-effect handler (which **replaces** the adapter's inline
> escalate-at-limit branch) and the churn-stopping re-schedule exclusion live in
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs).
> The reblock-issue signature stabilization is the new `fold_volatile_goal_ids`
> helper applied inside `problem_to_run_brief` in
> [`src/overseer/observer.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/observer.rs),
> upstream of the existing
> [`src/stewardship/dedup.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/dedup.rs)
> `failure_signature`.

For the rationale, see
[The OODA breaker quarantines terminal UNCLEAR-CRITERIA goals](../concepts/ooda-breaker-churn-suppression.md).

## Contents

- [Terminal-quarantine constants](#terminal-quarantine-constants)
- [`NoProgressResolution::QuarantineTerminal`](#noprogressresolutionquarantineterminal)
- [`resolution_for_why`](#resolution_for_why)
- [Quarantine marker helpers](#quarantine-marker-helpers)
- [Side effects: `apply_resolution_side_effects`](#side-effects-apply_resolution_side_effects)
- [Re-schedule exclusion: `reinvestigate_bare_blocked_goals`](#re-schedule-exclusion-reinvestigate_bare_blocked_goals)
- [Reblock-issue signature stabilization](#reblock-issue-signature-stabilization)
- [Invariants](#invariants)

## Terminal-quarantine constants

| Constant | Value | Meaning |
| --- | --- | --- |
| `SURFACED_INVESTIGATION_FAILURE_LIMIT` | `3` | Consecutive evidence-less surfaced failures on an `UNCLEAR-CRITERIA` goal before the terminal-quarantine rung fires. Reused unchanged from the [terminal-investigation bound](./no-progress-reinvestigation-api.md). |
| `NO_PROGRESS_QUARANTINE_MARKER_KIND` | `"ooda-breaker-quarantine"` | The `WipRef.kind` of the durable quarantine marker. A novel kind, ignored by every other `wip_refs` consumer via its `_ => None` fall-through. |
| `NO_PROGRESS_QUARANTINE_MARKER_REF_ID` | fixed sentinel string | The `WipRef.ref_id` of the quarantine marker. A compile-time constant — **never** derived from goal text — so goal descriptions cannot forge or smuggle content into a quarantine marker. |

These sit alongside the existing `NO_PROGRESS_BREAKER_THRESHOLD` (`3`) and the
[storm-suppression marker](./no-progress-breaker-storm-suppression-api.md)
constants; quarantine is an additive terminal rung above them, not a change to
either.

## `NoProgressResolution::QuarantineTerminal`

A new terminal variant of the pure resolution enum returned by
[`resolution_for_why`](#resolution_for_why):

```rust
pub enum NoProgressResolution {
    // … existing variants (MarkDone, Drop, Defer, SpawnEngineer,
    //    Escalate, SurfaceInvestigationFailure) …

    /// Terminal park for a goal that has exhausted the guided-retry ladder and
    /// the surfaced-failure bound on an UNCLEAR-CRITERIA WHY. Stops both
    /// re-filing AND re-scheduling. Reversible by an operator un-block.
    QuarantineTerminal {
        /// Consecutive evidence-less surfaced failures that drove the goal here.
        /// Rendered as real evidence in the Blocked reason — NEVER `(none)`.
        surfaced_count: u32,
    },
}

impl NoProgressResolution {
    /// `true` for `Escalate`, `SurfaceInvestigationFailure`, and
    /// `QuarantineTerminal` — resolutions that place a goal in a terminal or
    /// human-facing state.
    pub fn is_terminal(&self) -> bool { /* … */ }
}
```

`is_terminal()` returns `true` for `QuarantineTerminal`. The variant carries
`surfaced_count` so the authored Blocked reason renders concrete evidence and
never `evidence=[(none)]`, preserving the
[never-empty-evidence invariant](../concepts/no-progress-terminal-investigation.md#the-rule-never-evidencenone).

## `resolution_for_why`

The existing pure ladder function keeps its three current parameters and gains
**one** additive trailing parameter, `surfaced_failures`:

```rust
pub fn resolution_for_why(
    consecutive: u32,
    why: NoProgressWhy,
    guided_retry_used: bool,
    surfaced_failures: u32, // ← added; the goal's persisted surfaced-failure count
) -> NoProgressResolution;
```

`consecutive`, `why` (by value), and `guided_retry_used` are unchanged from the
current signature — nothing is removed or changed to by-reference. The caller in
`apply_resolution_side_effects` reads the goal's persisted surfaced-failure count
from `NoProgressTracker` and passes it as `surfaced_failures`.

Behavior (only the evidence-less terminal rung changes; everything else is
identical to today):

| WHY class | `guided_retry_used` | `why.evidence` | `surfaced_failures` | Result |
| --- | --- | --- | --- | --- |
| `AlreadyComplete` / `Obsolete` / `MissingPrecondition` / `UpstreamDependency` | any | any | any | unchanged (auto-complete / drop / self-heal / defer) |
| `UnclearCriteria` / `GenuinelyStuck` | `false` | any | any | unchanged (`SpawnEngineer` — the one-shot guided retry) |
| `UnclearCriteria` / `GenuinelyStuck` | `true` | non-empty | any | unchanged (evidence-backed `Escalate`) |
| `UnclearCriteria` / `GenuinelyStuck` | `true` | empty | `< SURFACED_INVESTIGATION_FAILURE_LIMIT` | unchanged (`SurfaceInvestigationFailure` — surface for retry) |
| `UnclearCriteria` / `GenuinelyStuck` | `true` | empty | `>= SURFACED_INVESTIGATION_FAILURE_LIMIT` | **`QuarantineTerminal { surfaced_count: surfaced_failures }`** (new) |

Quarantine is reachable **only** on the evidence-less terminal rung — i.e.
strictly after the guided engineer has run (`guided_retry_used == true`), the
investigation produced no evidence (`why.evidence.is_empty()`), and the
surfaced-failure count has reached the bound. Below the bound the function still
returns `SurfaceInvestigationFailure` exactly as it does today. It is purely a
function of its inputs (no I/O), so it is exhaustively unit-tested.

> **This replaces the adapter's escalate-at-limit branch.** Today the
> `SURFACED_INVESTIGATION_FAILURE_LIMIT` check lives in the
> `SurfaceInvestigationFailure` handler of `apply_resolution_side_effects`
> (`src/ooda_loop/no_progress.rs`, ~L1312): after incrementing the surfaced
> counter it calls `surfaced_failure_escalation_issue` and escalates in place.
> With quarantine, that limit decision moves **up** into `resolution_for_why`
> (which now returns `QuarantineTerminal` at the bound) and the adapter's inline
> escalate-at-limit branch is **removed and replaced** by the `QuarantineTerminal`
> handler below. The below-limit `SurfaceInvestigationFailure` handling (record
> the surfaced failure, surface for retry) stays. Without this replacement a
> bounded-out goal would both escalate (old branch) **and** quarantine (new
> branch) — the two must not coexist.

## Quarantine marker helpers

```rust
/// Build the durable quarantine `WipRef` for a goal. `ref_id` is the fixed
/// sentinel constant; `kind` is `NO_PROGRESS_QUARANTINE_MARKER_KIND`.
pub fn quarantine_marker() -> WipRef;

/// True when `wip` is the breaker-authored quarantine marker. Matches on the
/// fixed `kind` + sentinel `ref_id` ONLY — never on goal-derived text.
pub fn is_quarantine_ref(wip: &WipRef) -> bool;

/// True when `goal` carries the quarantine marker in its `wip_refs`.
pub fn is_quarantined(goal: &ActiveGoal) -> bool;
```

`is_quarantine_ref` is injection-safe by construction: it keys on the fixed
sentinel, so no goal description can forge a quarantine or clear another goal's.

## Side effects: `apply_resolution_side_effects`

The curate-phase adapter reads the goal's persisted surfaced-failure count from
`NoProgressTracker`, passes it into [`resolution_for_why`](#resolution_for_why),
and — where that function now returns `QuarantineTerminal` — handles it by:

1. **Blocking the goal** with a WHY-bearing reason built from the breaker prefix
   plus the `surfaced_count` as evidence (never `(none)`).
2. **Writing the quarantine marker idempotently** through the existing atomic,
   single-writer goal-board save path. A goal already carrying the marker is not
   re-written (no duplicate `WipRef`).
3. **Reusing the escalation idempotence** of the
   [storm-suppression marker](./no-progress-breaker-storm-suppression-api.md) so a
   quarantined goal is never re-filed.

This `QuarantineTerminal` handler **replaces** the adapter's prior inline
escalate-at-limit branch (the `surfaced >= SURFACED_INVESTIGATION_FAILURE_LIMIT`
check that called `surfaced_failure_escalation_issue` and escalated at ~L1312).
The below-limit `SurfaceInvestigationFailure` path — record the surfaced failure
and surface it for retry — is unchanged.

**Fail-closed:** if the marker write returns `Err`, the adapter records the error
(fail visible), takes **no** terminal claim, and leaves the goal retriable so the
quarantine is re-attempted next cycle — it never silently proceeds as if the goal
were quarantined when the durable marker did not land.

## Re-schedule exclusion: `reinvestigate_bare_blocked_goals`

This is the single change that stops the churn. The re-investigation pass that
sweeps blocked goals back into the breaker each cycle now **skips any goal for
which `is_quarantined(goal)` is true**:

```rust
// inside reinvestigate_bare_blocked_goals(…)
if is_quarantined(goal) {
    continue; // terminal — never re-schedule, re-classify, or re-escalate
}
```

A quarantined goal is therefore never re-selected, so it produces no further
`ooda-stuck` escalation and no further re-block for the Overseer to observe. The
exclusion composes with the existing
[bare/evidence-less selection predicates](../concepts/no-progress-terminal-investigation.md#the-stranded-already-blocked-population):
quarantine is checked first and short-circuits.

## Reblock-issue signature stabilization

`recurring_goal_reblock in simard::overseer` is **not a code constant** — it is
the observed `dedup_key` / issue-title text that the Overseer produces when it
re-observes a goal being re-blocked (the string seen in the field and in the
tests). The stewardship issue for it is deduplicated through the existing
failure-signature path, **not** through `root_cause_signature`:

```text
observer.rs::problem_to_run_brief(problem)
    → OrchestratorRunBrief {
          failure_kind: problem.dedup_key,          // ← used RAW downstream
          error_text:   stable_error_text(problem),
          …
      }
    → stewardship::process_orchestrator_run
    → dedup::failure_signature(failure_kind, error_text)
```

`dedup::failure_signature` (in `src/stewardship/dedup.rs`) SHA-256s
`failure_kind` **verbatim** and normalizes **only** `error_text` (its private
`normalize_for_signature` redacts UUIDs in the message). So when
`problem.dedup_key` embeds a volatile goal identifier, `failure_kind` drifts
every cycle, the signature drifts with it, and each re-observation files a fresh
issue — the churn.

> `root_cause_signature` (in `src/overseer/root_cause.rs`) is exported but has
> **no non-test caller** keying the reblock issue. It is **not** the reblock
> dedup key and is intentionally left untouched by this feature. Do not confuse
> it with the `failure_signature` path above.

### The fix: fold volatile goal ids *before* `failure_kind`

The stabilization is applied **upstream** of `failure_signature`, inside
`problem_to_run_brief`, so the already-normalized key flows through the existing
dedup unchanged:

```rust
// src/overseer/observer.rs — inside problem_to_run_brief(problem)
OrchestratorRunBrief {
    // was: failure_kind: problem.dedup_key.clone(),
    failure_kind: fold_volatile_goal_ids(&problem.dedup_key),
    error_text: stable_error_text(problem),
    // …unchanged…
}
```

`fold_volatile_goal_ids` is a **new** pure, total helper. It is **deliberately
named to avoid the existing private `dedup::normalize_for_signature`** (which is
the message UUID-redactor used inside `failure_signature`, a different function
with a different purpose in a different module). Keeping the names distinct
prevents the collision the review flagged.

```rust
/// Fold volatile goal identifiers in a dedup key to stable placeholders so
/// recurrences of the SAME re-block cause share one `failure_signature`.
/// Conservative: only the known volatile shapes are rewritten; everything else
/// is returned byte-for-byte, so distinct causes keep distinct signatures.
pub fn fold_volatile_goal_ids(dedup_key: &str) -> String;
```

| Volatile shape | Folded to |
| --- | --- |
| `simard-identity-<hash/slug>` | `simard-identity-*` |
| `goal-<n>` (positional slug) | `goal-*` |

> **Ownership.** `fold_volatile_goal_ids` lives in `src/overseer/observer.rs`
> alongside its only caller, `problem_to_run_brief`. It does **not** extend or
> shadow `dedup::normalize_for_signature`; if a future change instead wants the
> fold to apply to *every* `failure_signature` caller, it would extend
> `dedup::failure_signature` to normalize `failure_kind` — but that broader
> change is out of scope here.

Because `failure_kind` is now stable across re-observations of the same
underlying re-block cause, the stewardship dedup in `src/stewardship/dedup.rs`
collapses every recurrence onto **one** open issue via the existing
`find_existing` + `stewardship-signature:` mechanism, instead of one issue per
cycle.

## Invariants

- **Additive / non-breaking.** No `pub` item is removed; `resolution_for_why`
  gains a parameter and `NoProgressResolution` gains a variant (an intentional,
  compiler-checked exhaustiveness change). The PRD is preserved.
- **Never `evidence=[(none)]`.** `QuarantineTerminal` always renders the
  surfaced count as real evidence.
- **Injection-safe.** The quarantine marker keys on a fixed sentinel, never on
  goal-derived text.
- **Fail-closed & fail-visible.** A failed marker write surfaces the error and
  leaves the goal retriable; it never fakes a quarantine.
- **Reversible.** An operator un-block clears the marker and resets the
  surfaced-failure counter, granting a fresh bounded window.
- **Bounded, not rate-limited.** Quarantine is a hard terminal stop after a
  finite bound, not a backoff.
- **Scope-disjoint from admission.** No goal-admission-gate code (PRs
  #4939/#4941) is touched; this owns only residual re-schedule + reblock-dedup.
- **No `print!`/`println!`.** All audit output is structured tracing / OTel.
