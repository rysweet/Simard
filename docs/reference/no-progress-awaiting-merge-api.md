---
title: No-progress awaiting-merge API reference
description: Reference for the OODA no-progress breaker's awaiting-merge branch (issue #4441) — the `EvidenceSource::open_mergeable_pr` signal and its fail-closed default, the `GhCliEvidenceSource` live `gh pr view` query with argument validation, the `CompletionEvidenceGate::awaiting_merge` pass-through, the `StuckGoalDisposition::AwaitingMerge` disposition, the non-terminal `NoProgressResolution::AwaitMerge` resolution, its no-op side effect, the `awaiting_merge` report field, and the structured suppression trace.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/no-progress-awaiting-merge-exemption.md
  - ../concepts/no-progress-root-cause-resolution.md
  - ../concepts/deploy-aware-done-gate.md
  - ./no-progress-breaker-api.md
  - ./no-progress-root-cause-resolution-api.md
  - ./completion-evidence-gate-api.md
  - ../howto/diagnose-an-awaiting-merge-idle.md
  - ../../src/goal_curation/completion_gate.rs
  - ../../src/goal_curation/no_progress_breaker.rs
  - ../../src/goal_curation/types.rs
  - ../../src/ooda_loop/no_progress.rs
---

# No-progress awaiting-merge API reference

> **Status: implemented (issue #4441).** The `open_mergeable_pr` evidence signal
> and the `awaiting_merge` pass-through live in
> [`src/goal_curation/completion_gate.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs).
> The `StuckGoalDisposition::AwaitingMerge` disposition, the
> `NoProgressResolution::AwaitMerge` resolution, and their wiring through
> `verify_stuck_goal` / `resolve_no_progress` live in
> [`src/goal_curation/no_progress_breaker.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/no_progress_breaker.rs).
> The no-op side effect and the `awaiting_merge` report field live in
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs).

This reference specifies the API of the awaiting-merge branch added to the OODA
no-progress breaker. For the rationale and the duplicate-PR incident it fixes,
see
[A goal with an open, mergeable PR is awaiting merge — never reaped](../concepts/no-progress-awaiting-merge-exemption.md).
It extends — and does not modify — the base
[no-progress breaker API](./no-progress-breaker-api.md) and the
[completion-evidence gate API](./completion-evidence-gate-api.md).

## Contents

- [`EvidenceSource::open_mergeable_pr`](#evidencesourceopen_mergeable_pr)
- [`GhCliEvidenceSource::open_mergeable_pr` (live query)](#ghclievidencesourceopen_mergeable_pr-live-query)
- [`CompletionEvidenceGate::awaiting_merge`](#completionevidencegateawaiting_merge)
- [`StuckGoalDisposition::AwaitingMerge`](#stuckgoaldispositionawaitingmerge)
- [`NoProgressResolution::AwaitMerge`](#noprogressresolutionawaitmerge)
- [Side effect: idle, no spawn, no reap](#side-effect-idle-no-spawn-no-reap)
- [Report field: `awaiting_merge`](#report-field)
- [Fail-closed table](#fail-closed-table)
- [Security](#security)
- [What is unchanged](#what-is-unchanged)

## `EvidenceSource::open_mergeable_pr`

A new **default-bodied** method on the injected
[`EvidenceSource`](./completion-evidence-gate-api.md) trait. It reports whether
the goal's PR is simultaneously `OPEN`, non-draft, and `MERGEABLE`. The default
returns `Ok(false)` (fail-closed) so every existing implementation and test
double keeps compiling unchanged and a source that cannot tell never suppresses
a reap.

```rust
pub trait EvidenceSource: Send + Sync {
    // … existing methods (any_pr_merged, issue_closed, is_deployed,
    //    repo_present, dependency_goal_state) …

    /// Does the goal's tracked PR satisfy `state == OPEN` ∧ `!isDraft`
    /// ∧ `mergeable == MERGEABLE`? Backs the `AWAITING-MERGE` branch of the
    /// no-progress breaker (issue #4441). Default `Ok(false)`: a source that
    /// cannot tell must never suppress a legitimate reap.
    fn open_mergeable_pr(&self, goal: &ActiveGoal) -> SimardResult<bool> {
        let _ = goal;
        Ok(false)
    }
}
```

The `&T` blanket impl forwards the method so an `&dyn EvidenceSource`
(e.g. `Arc::as_ref()`) still satisfies the `E: EvidenceSource` bound:

```rust
impl<T: EvidenceSource + ?Sized> EvidenceSource for &T {
    // … existing forwards …
    fn open_mergeable_pr(&self, goal: &ActiveGoal) -> SimardResult<bool> {
        (**self).open_mergeable_pr(goal)
    }
}
```

## `GhCliEvidenceSource::open_mergeable_pr` (live query)

The production source resolves the signal with a single read-only `gh` call. It
resolves the goal's PR number from the first `wip_ref` of kind `"pr"` (via
`first_ref_of_kind(goal, "pr")`) and the `owner/repo` slug via the existing
`repo_slug(goal)` helper (defaulting to `rysweet/Simard`).

```rust
impl EvidenceSource for GhCliEvidenceSource {
    fn open_mergeable_pr(&self, goal: &ActiveGoal) -> SimardResult<bool> {
        let Some(num) = first_ref_of_kind(goal, "pr") else {
            return Ok(false); // no tracked PR ⇒ nothing awaiting merge
        };
        let repo = self.repo_slug(goal);

        // Fail-closed argument validation BEFORE spawn (see Security).
        if !num.bytes().all(|b| b.is_ascii_digit()) || num.is_empty() {
            return Ok(false);
        }
        if !is_valid_repo_slug(&repo) {
            return Ok(false);
        }

        // Arg-vector spawn — never a shell string.
        let out = std::process::Command::new("gh")
            .args([
                "pr", "view", num, "--repo", &repo, "--json",
                "state,isDraft,mergeable",
            ])
            .output();

        // Any spawn/exit/parse failure ⇒ Ok(false), never an Err that would
        // block the whole cycle; the base done-gate methods keep their own
        // fail-closed-to-Err behaviour unchanged.
        let out = match out {
            Ok(o) if o.status.success() => o,
            _ => return Ok(false),
        };

        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct PrView {
            state: String,
            is_draft: bool,
            mergeable: String,
        }
        let pr: PrView = match serde_json::from_slice(&out.stdout) {
            Ok(pr) => pr,
            Err(_) => return Ok(false),
        };

        Ok(pr.state.eq_ignore_ascii_case("OPEN")
            && !pr.is_draft
            && pr.mergeable.eq_ignore_ascii_case("MERGEABLE"))
    }
}
```

The invoked command is exactly:

```console
gh pr view <num> --repo <owner>/<repo> --json state,isDraft,mergeable
```

`mergeable == MERGEABLE` is the positive value of GitHub's GraphQL `MergeableState`
enum (`gh pr view --json mergeable`), whose only values are `MERGEABLE`,
`CONFLICTING`, and `UNKNOWN`. It is a **distinct** field from `mergeStateStatus`
(`CLEAN`/`DIRTY`/`BLOCKED`/…), which this branch does not query. Any value other
than `MERGEABLE` — i.e. `CONFLICTING` or `UNKNOWN` — → `Ok(false)`.

> **Helpers.** `first_ref_of_kind` and `repo_slug` already exist in
> `completion_gate.rs`. `is_valid_repo_slug` is a **new** helper this change
> adds (byte-validation of `^[\w.-]+/[\w.-]+$`, no new regex dependency).
> `open_mergeable_pr` deliberately does **not** reuse the existing `gh_state`
> helper because it needs three JSON fields (`state,isDraft,mergeable`) in one
> call, whereas `gh_state` fetches only `.state`.

## `CompletionEvidenceGate::awaiting_merge`

A thin pass-through on the gate that delegates to the injected source. It is
**not** a new `CompletionVerdict` variant — the deploy-aware done-gate's verdict
semantics (`Complete` / `Blocked`) are byte-identical to before. The breaker
consults this method separately, only when a goal is otherwise `Blocked`.

```rust
impl<E: EvidenceSource> CompletionEvidenceGate<E> {
    /// Pass-through to `EvidenceSource::open_mergeable_pr`. Used by
    /// `verify_stuck_goal` to distinguish a completed-awaiting-merge goal from a
    /// genuinely-stuck one. Fail-closed: a source error resolves to `false`.
    pub fn awaiting_merge(&self, goal: &ActiveGoal) -> bool {
        self.source.open_mergeable_pr(goal).unwrap_or(false)
    }
}
```

## `StuckGoalDisposition::AwaitingMerge`

A new variant on the stuck-goal disposition. `verify_stuck_goal` returns it when
the done-gate says `Blocked`, the goal is **not** obsolete, **and**
`awaiting_merge()` is `true`:

```rust
pub enum StuckGoalDisposition {
    Done,
    Obsolete { reason: String },
    /// The goal's workstream has already delivered an open, non-draft,
    /// mergeable PR; it is awaiting an external merge, not stalled (issue #4441).
    AwaitingMerge,
    Unresolved,
}
```

```rust
pub fn verify_stuck_goal<E: EvidenceSource>(
    goal: &ActiveGoal,
    gate: &CompletionEvidenceGate<E>,
) -> StuckGoalDisposition {
    match gate.evaluate(goal) {
        CompletionVerdict::Complete(_) => StuckGoalDisposition::Done,
        CompletionVerdict::Blocked { .. } => {
            if let Some(reason) = obsolescence_reason(goal) {
                StuckGoalDisposition::Obsolete { reason }
            } else if gate.awaiting_merge(goal) {
                StuckGoalDisposition::AwaitingMerge
            } else {
                StuckGoalDisposition::Unresolved
            }
        }
    }
}
```

Ordering matters: a **merged** PR is caught earlier as `Complete` → `Done`;
`AwaitingMerge` is reached only for a not-yet-merged but landable PR. Obsolete
still wins over awaiting-merge (an out-of-scope goal is dropped even if it has an
open PR).

## `NoProgressResolution::AwaitMerge`

A new **non-terminal** resolution. Unlike every other threshold resolution, it
does not remove the goal from the no-action loop and does not carry a payload —
the goal stays tracked and is re-evaluated next cycle.

```rust
pub enum NoProgressResolution {
    Continue,
    MarkDone,
    Drop { reason: String },
    /// The goal has an open, mergeable PR awaiting an external merge action
    /// (issue #4441). NON-TERMINAL: the breaker idles the goal — it does NOT
    /// reap, escalate, or spawn an engineer, so no duplicate PR is created.
    /// Falls back to reap/escalate automatically on the next cycle if the PR
    /// closes, goes draft, or degrades below MERGEABLE.
    AwaitMerge,
    Heal { why: NoProgressWhy },
    Defer { blocking_ref: String, evidence: Vec<Evidence> },
    SpawnEngineer { task: String, why: NoProgressWhy },
    Escalate { blocked_reason: String, issue_title: String, issue_body: String },
    SurfaceInvestigationFailure { class: NoProgressClass, reason: String },
}
```

`resolve_no_progress` maps the disposition through:

```rust
match disposition() {
    StuckGoalDisposition::Done => NoProgressResolution::MarkDone,
    StuckGoalDisposition::Obsolete { reason } => NoProgressResolution::Drop { reason },
    StuckGoalDisposition::AwaitingMerge => NoProgressResolution::AwaitMerge,
    StuckGoalDisposition::Unresolved => { /* … Escalate as before … */ }
}
```

`AwaitMerge` must be added to `NoProgressResolution::is_terminal()` as a
non-terminal arm (alongside `Continue` and `SurfaceInvestigationFailure`), so it
reports `is_terminal() == false`: the goal remains tracked and its no-action
counter is **not** cleared as a terminal outcome, so a subsequent degradation
re-enters the ladder immediately.

```rust
pub fn is_terminal(&self) -> bool {
    !matches!(
        self,
        Self::Continue | Self::SurfaceInvestigationFailure { .. } | Self::AwaitMerge,
    )
}
```

## Side effect: idle, no spawn, no reap

`NoProgressResolution` is matched at **two** sites in
`src/ooda_loop/no_progress.rs`, and because Rust match exhaustiveness applies to
both, **each gains an idle-only `AwaitMerge` arm** (design components C9/C10):

1. `apply_no_progress_breaker_with_threshold` (the base path) — reached when
   `verify_stuck_goal` → `resolve_no_progress` yields `AwaitMerge`. Its existing
   arms are `Continue` / `MarkDone` / `Drop` / `Escalate` plus a defensive
   root-cause arm; the new `AwaitMerge` arm records `report.awaiting_merge` and
   emits the suppression trace, and takes no board action.
2. `apply_resolution_side_effects` (the investigated / root-cause path) — same
   idle-only behavior.

Both arms perform **no disruptive action** — critically, neither calls
`dispatcher.spawn_engineer(...)` (the source of the duplicate PR), neither reaps
the engineer or mutates the goal's status, and neither calls
`tracker.reset_count(...)` (the counter is preserved so a degraded PR falls back
to reap/escalate on the very next cycle):

Because `AwaitMerge` is **payload-free** (it carries no PR number), the arm
re-resolves the PR for the trace from the goal's refs via
`first_ref_of_kind(goal, "pr")`, falling back to `"?"` if none is found (the
disposition guarantees one exists, so this is defensive only):

```rust
NoProgressResolution::AwaitMerge => {
    // Completed-awaiting-merge: idle only. No spawn, no reap, no block,
    // no counter reset.
    report.awaiting_merge.push(goal_id.to_string());
    let pr_number = first_ref_of_kind(goal, "pr").unwrap_or("?");
    tracing::info!(
        target: "simard::ooda",
        goal = %goal_id,
        pr = %pr_number,
        pr_open = true,
        pr_draft = false,
        pr_mergeable = true,
        "no-progress breaker: goal has an open, mergeable PR — awaiting external \
         merge; suppressing reap/re-dispatch (no duplicate PR created)",
    );
}
```

The trace carries only the `goal_id`, the PR number, and the three decision
booleans — never tokens, `GH_TOKEN`, or raw `gh` stderr.

## Report field

`NoProgressBreakerReport` gains an additive, default-derived field for
observability and test assertions:

```rust
pub(crate) struct NoProgressBreakerReport {
    // … existing fields (marked_done, dropped, escalated, healed, deferred,
    //    engineer_spawned, auto_cleared, investigation_errors, reinvestigated,
    //    perpetual_idled) …

    /// Goals idled this cycle because they have an open, non-draft, mergeable
    /// PR awaiting an external merge (issue #4441). Informational only — an
    /// awaiting-merge idle is NORMAL, not a fault, so it never contributes to
    /// `fired()`.
    pub awaiting_merge: Vec<String>,
}
```

Like `perpetual_idled` and `auto_cleared`, `awaiting_merge` **does not** count
toward [`fired()`](./no-progress-breaker-api.md#noprogressbreakerreport): a cycle
whose only breaker activity was idling an awaiting-merge goal still reports
`fired() == false`.

For observability parity with `perpetual_idled`, `NoProgressBreakerReport::log_line()`
is extended to surface the count, appending `awaiting_merge={}` to the compact
cycle-log summary (component C11). The summary line is always rendered by the
caller regardless of `fired()`, so an awaiting-merge idle is visible in the
cycle log as `… perpetual_idled=0 awaiting_merge=1`.

## Fail-closed table

Every uncertain input resolves to "not awaiting merge", so the branch can only
ever fail to suppress a reap — never suppress one that should happen.

| Input | `open_mergeable_pr` result | Breaker outcome |
| --- | --- | --- |
| PR `OPEN` ∧ `!isDraft` ∧ `MERGEABLE` | `Ok(true)` | `AwaitMerge` — idle, no reap |
| PR `MERGED` | (caught earlier as `Complete`) | `MarkDone` |
| PR `CLOSED` (not merged) | `Ok(false)` | reap / escalate |
| PR `isDraft == true` | `Ok(false)` | reap / escalate |
| PR `mergeable == CONFLICTING`/`UNKNOWN` | `Ok(false)` | reap / escalate |
| no `wip_ref` of kind `pr` | `Ok(false)` | reap / escalate |
| `gh` spawn/exit failure | `Ok(false)` | reap / escalate |
| JSON parse failure | `Ok(false)` | reap / escalate |
| non-numeric PR num or invalid repo slug | `Ok(false)` (no spawn) | reap / escalate |

## Security

- **No shell.** The query uses `Command::new("gh").args([...])` (arg-vector),
  never `sh -c` or a `format!`-built command string.
- **Argument validation before spawn.** The PR number is validated against
  `^[0-9]+$` and the repo slug against `^[\w.-]+/[\w.-]+$`; a mismatch returns
  `Ok(false)` without spawning.
- **Least privilege.** Only the read-only `gh pr view` is used — no
  state-changing subcommand, no merge/close capability.
- **No secrets in logs.** The suppression trace emits the goal id, PR number,
  and three booleans only; `gh` stderr is never logged and `GH_TOKEN` is never
  read or passed explicitly (ambient `gh` auth is reused exactly as the base
  done-gate does).

## What is unchanged

- `NO_PROGRESS_BREAKER_THRESHOLD`, the sentinel constants, and
  `is_no_progress_marker` — unchanged.
- The base `EvidenceSource` done-gate methods (`any_pr_merged`, `issue_closed`,
  `is_deployed`) and their fail-closed-to-`Err` behaviour — unchanged.
- `CompletionVerdict` — no new variant; `awaiting_merge` is a separate
  pass-through query.
- The reap/escalate path for genuinely-stalled engineers and the *behavior* of
  the existing root-cause resolution rungs — unchanged. The additive edits are:
  one new `StuckGoalDisposition` variant, one new `NoProgressResolution` variant,
  the `is_terminal()` non-terminal arm, the `verify_stuck_goal` /
  `resolve_no_progress` mappings, one arm in each of the two `NoProgressResolution`
  match sites, the `awaiting_merge` report field, and the `log_line()` counter.

## See also

- [Concept: a goal with an open, mergeable PR is awaiting merge — never reaped](../concepts/no-progress-awaiting-merge-exemption.md)
- [No-progress breaker API reference](./no-progress-breaker-api.md) — the base breaker, threshold, sentinel, and report.
- [No-progress root-cause resolution API reference](./no-progress-root-cause-resolution-api.md) — the resolution ladder `AwaitMerge` precedes.
- [Completion-evidence gate API](./completion-evidence-gate-api.md) — the `EvidenceSource` trait and `CompletionEvidenceGate` this extends.
- [Diagnose an awaiting-merge idle](../howto/diagnose-an-awaiting-merge-idle.md) — the operator runbook.
