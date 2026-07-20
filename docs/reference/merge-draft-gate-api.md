---
title: Merge draft gate API reference
description: >
  The typed surface of the merge authority's draft-state gate (#4344 / #4145):
  the new `PrSnapshot.is_draft` field and its `isDraft` serde wiring in
  `src/stewardship/merge_authority.rs`, the fail-closed draft gate composed into
  `evaluate_objective_gates`, the `gh pr view --json` field-list change, and the
  single pre-merge snapshot the gate reasons against — making
  `merge_pr_if_merge_ready` decide against the freshly-fetched PR
  state instead of a stale-draft abort (no additional re-fetch).
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: reference
status: reference
related:
  - ../concepts/merge-draft-state-revalidation.md
  - ./cross-repo-merge-authority.md
  - ./autonomous-merge-review-gate.md
  - ../concepts/operational-autonomy-model.md
  - ../howto/diagnose-a-still-a-draft-merge-refusal.md
  - ../design/overseer.md
---

# Merge draft gate API reference

> **Status: implemented (#4344 / #4145).** The base merge authority
> (`PrSnapshot`, `view_pr`, `evaluate_objective_gates`, the single pre-merge
> fetch) and the draft-specific additions this reference describes — the
> `PrSnapshot.is_draft` field, the `isDraft` field-list entry, and the
> fail-closed Gate 0.5 — are all shipped in
> [`src/stewardship/merge_authority.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs).
> They close the "Pull Request is still a draft" self-merge stall in
> which the same GREEN, non-draft, MERGEABLE PRs were re-escalated to the operator
> on 13 consecutive Overseer ticks. For the rationale see
> [merge draft-state re-validation](../concepts/merge-draft-state-revalidation.md).

This reference documents the draft-gate additions to the
[cross-repo merge authority](./cross-repo-merge-authority.md). It does **not**
restate the full pipeline — read that reference first for the base-allowlist,
mergeable, CI, and merge-judge gates.

## `PrSnapshot.is_draft`

```rust
/// Snapshot of `gh pr view --json
/// body,statusCheckRollup,mergeable,reviewDecision,baseRefName,labels,isDraft`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PrSnapshot {
    pub body: String,
    pub mergeable: String,
    pub review_decision: String,
    pub checks: Vec<CheckRollupEntry>,
    pub base_ref_name: String,
    pub labels: Vec<String>,
    /// `isDraft` from `gh pr view` — `true` while the PR is a draft. Parsed with
    /// `#[serde(default, rename = "isDraft")]`, so absent or malformed JSON
    /// degrades to `false` rather than panicking the merge path. Gated by the
    /// draft gate in [`evaluate_objective_gates`].
    pub is_draft: bool,
}
```

- **`Default`-derived.** `PrSnapshot` derives `Default`, so `is_draft` defaults
  to `false` via `Default` and `serde(default)`. Note Rust struct literals are
  total: the ~17 existing `PrSnapshot { … }` fixture/caller sites (there is no
  `..Default::default()` shorthand in them today) must each add `is_draft: false`
  or migrate to `..Default::default()` — deriving `Default` does **not** let an
  existing literal omit the new field. This is a mechanical, semantics-preserving
  update (all default to non-draft), not a behavioural change.
- **serde-defaulted.** The parser (`parse_pr_view_json`) reads the field with
  `#[serde(default, rename = "isDraft")]`. A `gh` payload that omits `isDraft`, or
  supplies a non-boolean, yields `false` (a not-draft assumption is safe because
  the mergeable gate still independently guards readiness) instead of an `Err` or
  a panic. **No `unwrap`/`expect` on any `gh` JSON field.**

## `gh pr view` field list

`RealPrGhClient::view_pr` requests `isDraft` alongside the existing fields:

```text
gh pr view <PR> --repo <owner/repo> --json \
  body,statusCheckRollup,mergeable,reviewDecision,baseRefName,labels,isDraft
```

The idempotent read retains its transient-`gh` retry wrapper
(`retry_transient_gh`, `GH_READ_MAX_RETRIES = 3`) — see the
[resilience section](./cross-repo-merge-authority.md#resilience--transient-gh-retry)
of the merge-authority reference.

## The draft gate

`evaluate_objective_gates` gains a fail-closed draft gate, AND-composed into the
existing ordered gate chain:

```text
Gate 0    base-branch allowlist          (unchanged — evaluated first)
Gate 0.5  draft gate                      ← new: is_draft == false
Gate 1    mergeable == "MERGEABLE"        (unchanged)
Gate 2    every statusCheckRollup passing (unchanged)
```

```rust
// Gate 0.5: draft state. A draft PR is not merge-ready regardless of CI or
// mergeable status. Fail closed with a single actionable, PR-agnostic reason.
if snapshot.is_draft {
    return Err(
        "PR is still a draft and cannot be merged. Mark it ready first: \
         `gh pr ready <PR>`, then retry."
            .to_string(),
    );
}
```

Properties:

- **Fail-closed AND-composition.** The gate can only ever *add* a refusal; it
  never short-circuits or bypasses the base-allowlist, mergeable, or CI gates.
- **Deterministic, actionable reason.** The message is PR-agnostic and tells the
  operator exactly how to clear it (`gh pr ready <PR>`), so a `Refused` is
  self-explanatory in the activity feed.
- **Ordered after base-allowlist.** A PR targeting a wrong base is still refused
  first (the PR #1549 footgun), preserving the existing gate priority.

Because `evaluate_objective_gates` is `pub`, the operator dashboard's Merge
Readiness panel renders the draft verdict per open PR without invoking the
(expensive) judge.

## Pre-merge fetch (fresh-state gate evaluation)

`merge_pr_if_merge_ready_with_judge` fetches the PR snapshot **once, immediately
before** the gate evaluation and the merge mutation
([`merge_authority.rs:824`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs)),
and evaluates the objective gates against that same snapshot. No new re-fetch is
introduced by this change — the existing single pre-merge fetch already gives the
gate current state, so the draft decision and the `squash_merge` call read the
same `isDraft`. That is what makes the downstream `gh pr merge` "still a draft"
server-side abort **unreachable** for a PR the gate just confirmed non-draft: the
window between check and mutation is one already-tight in-tick fetch, not a stale
snapshot captured earlier.

```rust
// merge_authority.rs:824 — single pre-merge fetch; gates decide against it.
let snapshot = gh.view_pr(repo, pr_number)?;
// … creative-idea label gate …
if let Err(reason) = evaluate_objective_gates(&snapshot, base_allowlist) {
    return Ok(MergeOutcome::Refused { pr_number, reason });
}
// … agentic merge-judge, then gh.squash_merge(repo, pr_number) …
```

`squash_merge` (`gh pr merge`) remains a **single-attempt mutation** — its safe
retry boundary is the next gate-revalidating `merge_pr_if_merge_ready` cycle, not
a blind inner loop (unchanged from the base pipeline).

## Outcome mapping

| Fresh-snapshot condition        | Result                                       |
| ------------------------------- | -------------------------------------------- |
| `is_draft == true`              | `MergeOutcome::Refused { reason: "PR is still a draft …" }` |
| `mergeable != "MERGEABLE"`      | `MergeOutcome::Refused { reason: "mergeable status is …" }` |
| non-draft + mergeable + green   | proceeds to merge-judge, then `MergeOutcome::Merged` |
| `gh` failed / malformed JSON    | `Err(SimardError)` (could not evaluate)      |

A draft PR is a **`Refused`**, not an `Err` — an expected, quiet outcome that does
**not** trigger operator escalation. The Overseer escalates only on genuine
non-mergeability or an evaluation `Err`. See the
[autonomy model](../concepts/operational-autonomy-model.md) for the
escalation contract.

## Testing

The draft gate is pinned by regression tests in
`src/stewardship/merge_authority.rs` (inline `#[cfg(test)]`) and the stewardship
test modules:

- **Parser unit** — an `isDraft: true` payload parses to `is_draft == true`; a
  payload omitting `isDraft` parses to `false` without error.
- **Gate unit** — `evaluate_objective_gates` refuses a draft snapshot and passes
  an otherwise-identical non-draft snapshot.
- **Stale-draft regression** — a non-draft, mergeable, green PR **merges** and is
  no longer subject to a false-positive "still a draft" refusal; a genuine draft
  PR is `Refused` exactly once and does not `squash_merge`.

Adding the field touches the ~17 existing `PrSnapshot { … }` fixture/caller sites
(each gains `is_draft: false`, or migrates to `..Default::default()`); the
regression fixtures set `is_draft` explicitly.

## Invariants

- **`isDraft` requested and parsed.** Present in the `gh pr view --json` field
  list; parsed with `serde(default, rename = "isDraft")`; never `unwrap`ped.
- **Fail closed.** Merge proceeds only when `is_draft == false` **and**
  `mergeable == "MERGEABLE"` **and** all pre-existing gates pass.
- **Fresh-state evaluation.** Objective gates run against the existing single
  pre-merge snapshot (`merge_authority.rs:824`), fetched immediately before the
  merge mutation. No additional re-fetch is added.
- **Additive, ordered.** The draft gate is AND-composed after the base-allowlist
  and before the mergeable gate; it never bypasses an existing gate.
- **Draft ⇒ Refused, not Err.** A draft PR is a quiet `Refused` with an actionable
  reason and does not escalate to the operator.

## Related reading

- [Merge draft-state re-validation](../concepts/merge-draft-state-revalidation.md)
  — the *why*: the 13-tick escalation stall and the false-positive it fixes.
- [Cross-repo merge authority reference](./cross-repo-merge-authority.md) — the
  full pipeline this gate slots into.
- [Autonomous-merge review gate](./autonomous-merge-review-gate.md) — the agentic
  merge-judge that runs after the objective gates.
- [Diagnose a "still a draft" merge refusal](../howto/diagnose-a-still-a-draft-merge-refusal.md)
  — the operator playbook.
