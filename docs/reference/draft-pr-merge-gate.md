---
title: Draft-PR merge gate reference
description: >
  API surface for the isDraft objective merge gate. Documents the is_draft field
  on PrSnapshot and OpenPrSummary, the serde contract, the three gh JSON queries
  that request isDraft, the draft gate in evaluate_objective_gates, and the
  defensive short-circuit in merge_pr_if_merge_ready_with_judge.
last_updated: 2026-07-20
owner: simard
doc_type: reference
status: reference
related:
  - ../concepts/draft-pr-merge-exclusion.md
  - ./cross-repo-merge-authority.md
  - ../concepts/autonomous-self-merge-sensor.md
  - ../concepts/autonomous-merge-review-gate.md
---

# Draft-PR merge gate reference

The **draft gate** makes a GitHub draft pull request (`isDraft == true`) an
unconditional non-merge-ready state throughout Simard's merge authority. This
reference documents the API surface in
[`src/stewardship/merge_authority.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs).
For the *why*, see the
[draft-PR merge exclusion concept](../concepts/draft-pr-merge-exclusion.md).

## Data model

### `PrSnapshot.is_draft`

```rust
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PrSnapshot {
    pub body: String,
    pub mergeable: String,
    pub review_decision: String,
    pub checks: Vec<CheckRollupEntry>,
    pub base_ref_name: String,
    pub labels: Vec<String>,
    /// `isDraft` from `gh pr view` — GitHub's author-set "not ready to merge"
    /// flag. `true` fails the draft objective gate; drafts are never auto-merged.
    pub is_draft: bool,
}
```

### `OpenPrSummary.is_draft`

```rust
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct OpenPrSummary {
    pub number: u32,
    pub title: String,
    pub head_ref_name: String,
    pub base_ref_name: String,
    pub mergeable: String,
    pub checks: Vec<CheckRollupEntry>,
    pub url: String,
    pub author: String,
    pub labels: Vec<String>,
    /// `isDraft` from `gh pr list`. Carried so the survey pre-filter and the
    /// dashboard Merge Readiness panel exclude drafts. Propagated into
    /// `PrSnapshot::is_draft` by `to_snapshot()`.
    pub is_draft: bool,
}
```

`OpenPrSummary::to_snapshot()` copies `is_draft` into the produced `PrSnapshot`, so
the survey pre-filter (`survey_ready_prs`) and the authoritative gate evaluate the
same draft status.

### Serde contract

Both raw deserialization structs (`Raw` for `parse_pr_view_json`, `RawPr` for
`parse_pr_list_json`) bind the field as:

```rust
#[serde(default, rename = "isDraft")]
is_draft: bool,
```

- **`rename = "isDraft"`** — matches GitHub's camelCase JSON key.
- **`default`** — a well-formed JSON object that omits `isDraft` yields
  `is_draft = false` (treated as **not** a draft). This preserves back-compat for
  any snapshot or fixture built without the field.
- **Strict `bool`** — no string-to-bool coercion. A non-boolean `isDraft` value is
  a parse error, not a silent `false`.

> **Fail-closed, not fail-open.** `default` only applies to *well-formed* JSON that
> is missing the key. The `gh` calls still fail hard on a non-zero exit or empty
> stdout **before** serde runs, so a truncated/errored response never degrades a
> draft into a mergeable snapshot.

## `gh` queries

`isDraft` is appended to all three JSON field lists:

| Function | Command (abridged) |
|---|---|
| `view_pr` | `gh pr view <PR> --repo <owner/repo> --json body,statusCheckRollup,mergeable,reviewDecision,baseRefName,labels,isDraft` |
| `list_open_prs` | `gh pr list --json number,title,headRefName,baseRefName,mergeable,statusCheckRollup,url,author,labels,isDraft` |
| `list_prs_by_author` | `gh pr list --author <login> --json number,title,headRefName,baseRefName,mergeable,statusCheckRollup,url,author,labels,isDraft` |

## The draft gate

`evaluate_objective_gates` gains a draft gate, evaluated **early** (after the
base-branch allowlist, before/with `MERGEABLE`):

```rust
pub fn evaluate_objective_gates(
    snapshot: &PrSnapshot,
    base_allowlist: &[String],
) -> Result<(), String> {
    // Gate 0: base-branch allowlist
    // ...

    // Gate 1: not a draft
    if snapshot.is_draft {
        return Err(
            "PR is a draft (isDraft=true). Draft PRs are never auto-merged. \
             Mark it ready for review before merging: `gh pr ready <PR>`."
                .to_string(),
        );
    }

    // Gate 2: mergeable   (mergeable == "MERGEABLE")
    // Gate 3: CI green     (every check SUCCESS/NEUTRAL/SKIPPED)
    Ok(())
}
```

> **Implementer note — renumber the existing comments.** Today the code numbers
> the gates `Gate 0: base-branch allow-list`, `Gate 1: mergeable`, `Gate 2: every
> check`. Inserting the draft gate between the allow-list and `mergeable` shifts
> the trailing gates: the draft check becomes `Gate 1`, `mergeable` becomes
> `Gate 2`, and the CI rollup becomes `Gate 3`. Renumber the `// Gate N` comments
> so they stay in sync — the reason strings are unchanged.

Returning `Err(reason)` here means the PR is **excluded** from every consumer of
the objective pass:

- the `ready_prs` survey pre-filter (`survey_ready_prs` in
  `src/overseer/merge_ops.rs`) — a draft is never a `PrReadyToMerge` candidate;
- the dashboard's Merge Readiness panel — a draft renders a **not-ready** verdict
  carrying the draft reason.

## Defensive short-circuit

`merge_pr_if_merge_ready_with_judge` short-circuits on a draft snapshot **before**
`evaluate_objective_gates` (and therefore before `squash_merge`), placed right
next to the existing creative-idea-label skip and reusing the same
`MergeOutcome::Refused` mechanism so no new public type is introduced:

```rust
let snapshot = gh.view_pr(repo, pr_number)?;

// (existing) creative-idea label skip runs here …

// Defensive: never call mergePullRequest on a draft. Placed BEFORE
// evaluate_objective_gates (which also refuses drafts via Gate 1) so this
// dedicated reason="draft" log line wins and the skip survives any future
// reordering of the objective gates.
if snapshot.is_draft {
    tracing::info!(
        target: "stewardship::merge_authority",
        pr_number,
        reason = "draft",
        "skipping merge: PR is a draft",
    );
    return Ok(MergeOutcome::Refused {
        pr_number,
        reason: "PR is a draft (isDraft=true); draft PRs are never auto-merged"
            .to_string(),
    });
}

if let Err(reason) = evaluate_objective_gates(&snapshot, base_allowlist) {
    return Ok(MergeOutcome::Refused { pr_number, reason });
}
```

> **This short-circuit is not the merge-prevention layer.** `evaluate_objective_gates`
> (invoked immediately below, and at survey time) already refuses drafts via its
> draft gate — that objective gate is what stops the merge. The short-circuit's job
> is the dedicated, greppable `reason="draft"` log line and resilience if the gate
> order ever changes. See [the concept doc's two-layer model](../concepts/draft-pr-merge-exclusion.md#the-fix-draft-status-is-a-first-class-objective-gate).

- Exactly **one** structured `tracing::info!` line per skip (fields `pr_number`,
  `reason="draft"`), emitted with OTel. No `print!`/`println!`.
- `mergePullRequest` / `gh pr merge` is **never** reached for a draft.
- The public surface is unchanged — no new `MergeOutcome` variant — so the
  `evaluate_objective_gates_pub_surface_matches_merge_pipeline` invariant holds.

## Behavior matrix

| `is_draft` | `mergeable` | CI | `evaluate_objective_gates` | `merge_pr_if_merge_ready_with_judge` |
|---|---|---|---|---|
| `true` | `MERGEABLE` | all green | `Err("… is a draft …")` | `Refused` (short-circuit, no `squash_merge`) |
| `false` | `MERGEABLE` | all green | `Ok(())` (proceeds to judge) | unchanged — merges if judge `Ready` |
| `false` | not `MERGEABLE` | — | `Err("… mergeable status …")` | unchanged |

## Test coverage

The following tests in `merge_authority.rs` guard the gate:

- **Draft excluded from ready set** — a snapshot shaped like PR #4336
  (`isDraft=true, mergeable=MERGEABLE, all checks SUCCESS`) returns `Err` from
  `evaluate_objective_gates`.
- **Draft short-circuits merge** — `merge_pr_if_merge_ready_with_judge` returns
  `MergeOutcome::Refused` for a draft and the spy `gh` client records **zero**
  `squash_merge` calls. This is the security-enforcement regression guard: assert
  `squash_merge` call-count `== 0` for a draft.
- **Non-draft unchanged** — an `isDraft=false` snapshot still passes the objective
  gates and merges on a `Ready` judge verdict.
- **Serde default** — `parse_pr_view_json` / `parse_pr_list_json` on JSON that
  omits `isDraft` yields `is_draft == false`.
- **Pub-surface invariant** — `evaluate_objective_gates_pub_surface_matches_merge_pipeline`
  stays green (no new public type).
