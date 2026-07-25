---
title: Autonomous-merge review gate API reference
description: >
  The API surface of Simard's autonomous-merge review gate after the Design-(b)
  fix — verify() as a deterministic objective pre-filter (no review step), the
  merge-judge as the sole review authority in merge() step 3, the new
  OverseerError::NotMergeReady variant and its ActOutcome::Escalated mapping, the
  reviewer-free from_env()/new() constructors, the fail-closed RefusingMergeJudge
  path, and the full error/edge-case matrix.
last_updated: 2026-07-16
owner: simard
doc_type: reference
status: reference
related:
  - ../concepts/autonomous-merge-review-gate.md
  - ../concepts/autonomous-self-merge-sensor.md
  - ./cross-repo-merge-authority.md
  - ./ready-prs-sensor-api.md
  - ../howto/enable-autonomous-self-merge-canary.md
---

# Autonomous-merge review gate API reference

This reference documents the review gate in Simard's autonomous self-merge Act
path after the fix that made her actually merge eligible engineer PRs instead of
escalating all of them. For the *why* and the safety narrative, see
[the autonomous-merge review gate concept](../concepts/autonomous-merge-review-gate.md).

**One-line summary:** `verify()` is now a deterministic **objective pre-filter**
with no review step; the agentic **merge-judge** in
[`merge_authority`](./cross-repo-merge-authority.md) is the **sole** review
authority, invoked once in `merge()` step 3.

## What changed

| Before (broken) | After (this fix) |
|---|---|
| `verify()` ran 8 checks incl. **#7** "review (no Bug/Security ≥ High)" via an injected `DiffReviewer` | `verify()` runs **objective gates + diff-scans only** — no review step |
| Production `reviewer = None` → check #7 hardcoded `passed: false` → `verify().ready` **always false** | No `reviewer` field; `verify().ready` reflects only objective/deterministic checks |
| Authoritative merge-judge (step 3) **unreachable** | Merge-judge (step 3) is the **sole**, reachable review authority |
| Judge refusal would tally as an **error** | Judge refusal maps to `OverseerError::NotMergeReady` → **escalation** |
| `DiffReviewer` trait, `reviewer` arg, `FakeReviewer` | **Removed** from the merge path |

## `verify()` — deterministic objective pre-filter

`verify()` no longer performs any review. It runs the objective gates and the
additive deterministic diff-scans, and reports readiness as the conjunction of
those checks only.

```rust
impl PrOps for MergePrOps {
    /// Objective pre-filter: objective gates (CI green + MERGEABLE + base
    /// allow-list) plus the additive deterministic diff-scans. NO review step.
    ///
    /// `ready == true` means "eligible to proceed to the authoritative merge",
    /// NOT "approved to merge". The authoritative review runs later, in
    /// `merge()` step 3 (the agentic merge-judge). Safe because `merge()`
    /// re-verifies and judges before any squash_merge.
    fn verify(&self, repo: &str, pr: u32) -> Result<VerifyReport, OverseerError> {
        // #1–2 objective gates (evaluate_objective_gates)
        // #3–6, #8 additive deterministic diff-scans (run_diff_scans)
        // (no #7 review gate — removed)
        let ready = checks.iter().all(|c| c.passed);
        Ok(VerifyReport { ready, checks })
    }
}
```

`VerifyReport.ready` is now `true` for a genuinely merge-eligible engineer PR — a
green, `MERGEABLE`, allowlisted-base PR whose diff-scans pass — where before it was
always `false` because of the unwireable review check.

> **`ready` is eligibility, not approval.** The load-bearing review decision
> happens exactly once, downstream, in `merge()` step 3. `verify()` is
> deliberately review-free so there is no redundant LLM call.

> **Trait signature is intentionally unchanged.** `PrOps::verify` keeps its
> `(&self, repo: &str, pr: u32)` signature (defined in
> `src/overseer/capabilities.rs` and implemented by ~8 fakes across the overseer
> tests). The design spec's shorthand `fn verify(&self, pr: &Pr)` is **not**
> adopted: changing the trait signature would ripple through every implementor
> for no functional gain, and `verify()` still needs both `repo` and `pr` to call
> `view_pr`/`diff`. Only the *body* changes (drop check #7 and the `reviewer`
> branch); the signature stays.

## `merge()` — the merge-judge is the sole reviewer

`merge()` is unchanged in shape; it is now **reachable** in production because
`verify()` no longer fail-closes first.

1. **Step 0 — anti-recursion + author re-assert.** Refuse the Overseer's own PR
   (`RecursionGuard`); require the author to match the configured autonomous-merge
   identity (whole-login, case-insensitive). Both fail closed.
2. **Step 1 — objective pre-filter.** `verify()` must be `ready` (objective gates +
   diff-scans). A not-ready pre-filter returns `NotMergeReady` (escalate).
3. **Step 2 — poll-until-green.** Wait for required checks; escalate on red.
4. **Step 3 — authoritative agentic review + merge.**
   `merge_pr_if_merge_ready_with_judge(pr, repo, gh, &base_allowlist, judge)`
   re-runs the objective gates and then the **merge-judge**. Only `Ready` merges;
   `Refused` returns `NotMergeReady` (escalate). This is the single review call.
5. **Step 4 — notify the operator** on both channels (plain English).

## `OverseerError::NotMergeReady`

A new error variant distinguishes "evaluated, not ready now" from genuine
failures, so refusals are tallied as escalations rather than errors. The enum
lives in [`src/overseer/capabilities.rs`](./cross-repo-merge-authority.md); adding
the variant **also requires a new arm in its `impl fmt::Display`** (the match is
exhaustive, so the build fails without it).

```rust
// src/overseer/capabilities.rs
pub enum OverseerError {
    // ... existing variants: Capability, Gated, Budget, Recursion, Conflict ...

    /// The PR was evaluated but is not ready to merge right now: the merge-judge
    /// returned Refused/NotReady, the fail-closed RefusingMergeJudge fired (no LLM
    /// provider), or a re-verify came back not-ready. This is an EXPECTED,
    /// non-error outcome — the Act handler maps it to `ActOutcome::Escalated`, and
    /// a human is notified. It is NOT counted in `errors`.
    NotMergeReady { pr: u32, reason: String },
}
```

### Act-handler mapping (`mod.rs::act()`, not wiring.rs)

The `VerifyAndMergePr` handler is the `act()` arm in
[`src/overseer/mod.rs`](./cross-repo-merge-authority.md) (currently ~lines
804–812). It calls `verify()` first and, only if `ready`, calls `merge()`.
Post-fix, `merge()` can return `NotMergeReady`; the handler must translate that
into `ActOutcome::Escalated` instead of letting `?` propagate it as an error:

```rust
// src/overseer/mod.rs — impl OverseerAct::act()
Intervention::VerifyAndMergePr { repo, pr } => {
    let report = self.caps.prs.verify(repo, *pr)?;   // objective pre-filter
    if !report.ready {
        return Ok(ActOutcome::Escalated);            // pre-filter not ready
    }
    match self.caps.prs.merge(repo, *pr) {
        Ok(()) => Ok(ActOutcome::Merged),                    // prs_merged += 1
        Err(OverseerError::NotMergeReady { .. }) =>          // escalations += 1
            Ok(ActOutcome::Escalated),
        Err(other) => Err(other),                            // errors += 1
    }
}
```

The **tally** happens one layer up, in the tick loop in
[`src/overseer/wiring.rs`](./cross-repo-merge-authority.md): the existing
`ActOutcome::Escalated => report.escalations += 1` (wiring.rs ~405) and the
`Err(e) => report.errors += 1` (wiring.rs ~282) arms already do the right thing
**once** `act()` returns `Escalated` for `NotMergeReady`. No change to the wiring
tally is needed — only the `mod.rs` `act()` arm changes. (The design spec's
"Act handler (wiring.rs)" note misattributes the file: `wiring.rs` never calls
`caps.prs.merge` — that call site is `mod.rs:807`.)

## Constructors — reviewer argument removed

`from_env()` and `new()` no longer take a `reviewer` parameter, and the stale
"operator will wire a reviewer" doc comment is gone. The `judge` field (used by
`merge()` step 3) stays.

```rust
impl MergePrOps {
    /// Production adapter: real `gh` client + diff source + the env merge-judge
    /// (`build_merge_judge()`). No reviewer parameter — the merge-judge is the
    /// sole review authority. With no LLM provider, `build_merge_judge()` returns
    /// `RefusingMergeJudge`, so the autonomous path fails CLOSED (escalates).
    pub fn from_env() -> Self { /* ... judge: build_merge_judge() ... */ }

    /// Explicit-injection constructor (tests). No `reviewer` argument.
    pub fn new(
        gh: Box<dyn PrGhClient>,
        source: Box<dyn PrSource>,
        judge: Box<dyn MergeJudge>,
        // ... notifier, clock, recursion, automerge_author, base_allowlist ...
    ) -> Self { /* ... */ }
}
```

## Fail-closed on provider outage — no new code

The fail-closed guarantee is already provided by
[`build_merge_judge()`](./cross-repo-merge-authority.md) → `RefusingMergeJudge`:

```
no LLM provider configured
  → build_merge_judge() returns RefusingMergeJudge
  → judge() always returns Verdict::NotReady
  → merge_pr_if_merge_ready_with_judge returns MergeOutcome::Refused
  → merge() returns OverseerError::NotMergeReady
  → Act handler → ActOutcome::Escalated  (never a merge)
```

No new code is required for this path; a test asserts it holds.

## Removed / retained symbols

| Symbol | Fate | Why |
|---|---|---|
| `DiffReviewer` trait | **Removed** | Never had a production impl; superseded by the merge-judge |
| `MergePrOps.reviewer` field | **Removed** | The merge-judge is the sole reviewer |
| `reviewer` arg on `from_env()` / `new()` | **Removed** | — |
| `FakeReviewer` (test) | **Removed** | No trait left to fake |
| merge-path use of `should_commit` | **Removed** | Only fed check #7 |
| `ReviewFinding`, `Severity`, `FindingCategory` | **Retained** | Still used by `review_pipeline`, `self_improve_executor`, `engineer_loop` — **not** the merge path |
| `MergePrOps.judge` field | **Retained** | Drives `merge()` step 3 (sole reviewer) |

Removal of the review types is scoped to the **merge path only** — the shared
finding types stay for their non-merge consumers.

## Error & edge-case matrix

| Condition | `verify().ready` | `merge()` result | Tally |
|---|---|---|---|
| Green, `MERGEABLE`, `engineer/`-branch, configured author, clean diff-scans, judge `Ready` | `true` | **Merged** (`squash --delete-branch`) | `prs_merged` |
| Same, but judge `NotReady`/`Unclear`/`Refused` | `true` | `NotMergeReady` | **escalations** |
| No LLM provider (`RefusingMergeJudge`) | `true` | `NotMergeReady` | **escalations** |
| CI red / `PENDING` | `false` | `NotMergeReady` (pre-filter) | **escalations** |
| `mergeable != "MERGEABLE"` (e.g. `CONFLICTING`) | `false` | `NotMergeReady` (pre-filter) | **escalations** |
| Base branch not in allowlist | `false` | `NotMergeReady` (pre-filter) | **escalations** |
| Diff-scan fails | `false` | `NotMergeReady` (pre-filter) | **escalations** |
| Author ≠ configured autonomous-merge identity | (short-circuits) | `Err` (author gate, fail-closed) | **errors** |
| PR authored by `simard-overseer[bot]` | (short-circuits) | `Err` (recursion guard) | **errors** |
| `gh` failure / malformed snapshot | — | `Err` (could-not-evaluate) | **errors** |
| `SIMARD_AUTOMERGE_REPOS`/`_AUTHOR` unset | (never surveyed) | never reached — no candidate | none |

## Configuration

The review gate itself adds **no** new configuration. It relies on the existing
gates, all documented elsewhere:

| Variable | Effect | Reference |
|---|---|---|
| `SIMARD_AUTOMERGE_REPOS` | Repo allowlist; unset ⇒ OFF (no candidates) | [ready_prs sensor API](./ready-prs-sensor-api.md) |
| `SIMARD_AUTOMERGE_AUTHOR` | Own-PR identity; unset ⇒ OFF (fail-closed) | [ready_prs sensor API](./ready-prs-sensor-api.md) |
| `SIMARD_MERGE_BASE_ALLOWLIST` | Base-branch allowlist (default `["main"]`) | [cross-repo merge authority](./cross-repo-merge-authority.md) |
| *(LLM provider env — merge-judge)* | Absent ⇒ `RefusingMergeJudge` ⇒ escalate | [cross-repo merge authority](./cross-repo-merge-authority.md) |

## Examples

### A green engineer PR now merges autonomously

```text
# Overseer tick, canary repo enabled, engineer/ PR #4321 green + MERGEABLE:
survey_ready_prs: candidate rysweet/Simard#4321
Signal::PrReadyToMerge { repo: "rysweet/Simard", pr: 4321 }
Intervention::VerifyAndMergePr
  verify(): ready=true (objective gates + diff-scans)   # was false before the fix
  poll_until_green: green
  merge-judge: Ready — evidence sections present
  gh pr merge 4321 --squash --delete-branch
→ Merged. Operator notified.  (prs_merged += 1)
```

### A thin-evidence PR escalates (never merges)

```text
Intervention::VerifyAndMergePr
  verify(): ready=true
  poll_until_green: green
  merge-judge: NotReady — missing Quality-audit evidence
→ NotMergeReady → Escalated. Operator notified with the reason.  (escalations += 1)
```

### No LLM provider — fail-closed

```text
Intervention::VerifyAndMergePr
  verify(): ready=true
  merge-judge: RefusingMergeJudge → NotReady
→ NotMergeReady → Escalated. No merge.  (escalations += 1)
```

## Invariants

- **One reviewer.** The merge-judge (step 3) is the sole review authority. There
  is no second LLM review call, and no code/heuristic reviewer.
- **`verify()` = objective pre-filter.** Objective gates + deterministic
  diff-scans only. `ready` means *eligible*, not *approved*.
- **Refusals escalate.** `NotMergeReady` → `ActOutcome::Escalated`; it is never
  counted as an error. Only could-not-evaluate and safety-gate refusals are errors.
- **Fail-closed on provider outage.** No LLM provider ⇒ `RefusingMergeJudge` ⇒
  `NotReady` ⇒ `NotMergeReady` ⇒ escalate. A judge outage never defaults to merge.
- **All prior safety gates hold.** Fail-closed default, engineer-PR scoping,
  author re-assert + recursion guard, base allowlist, poll-until-green,
  creative-idea-label exclusion, squash + delete-branch only, no `--admin`/
  `--no-verify`, no wall-clock timeouts.
- **Removal is merge-path-scoped.** `ReviewFinding`/`Severity`/`FindingCategory`
  remain for their non-merge consumers.

## Related reading

- [Autonomous-merge review gate concept](../concepts/autonomous-merge-review-gate.md)
  — the bug and the Design-(b) rationale.
- [Cross-repo merge authority](./cross-repo-merge-authority.md) — the merge-judge
  pipeline that is now the sole reviewer.
- [ready_prs sensor API](./ready-prs-sensor-api.md) — the upstream candidate
  survey and its scoping gates.
- [Enable autonomous self-merge (canary)](../howto/enable-autonomous-self-merge-canary.md)
  — the operator runbook to turn it on one repo at a time.
