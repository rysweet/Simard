---
title: Draft-PR exclusion gate API reference
description: >
  The deterministic guardrail that excludes draft pull requests from the
  autonomous merge-queue's ready-PR candidate set (#4339). Documents the isDraft
  addition to the gh pr list / gh pr view --json field sets, the
  OpenPrSummary.is_draft and ProjectionCandidate.is_draft fields, the fail-closed
  Option<bool> semantics (admit ONLY Some(false); exclude Some(true) and None),
  and the exclusion placement in BOTH ready-PR producers — survey_ready_prs and
  project_ready_prs.
last_updated: 2026-07-20
owner: simard
doc_type: reference
status: reference
related:
  - ../concepts/draft-pr-merge-exclusion.md
  - ./ready-prs-sensor-api.md
  - ./agentic-merge-queue-reasoning-api.md
  - ./cross-repo-merge-authority.md
  - ../howto/enable-autonomous-self-merge-canary.md
---

# Draft-PR exclusion gate API reference

This reference documents the deterministic rail that keeps a **draft** pull
request out of `ObservedState.ready_prs`, closing the #4339 bug where a
`CLEAN`/`MERGEABLE` draft was admitted every tick and then failed `gh pr merge`
with `Pull Request is still a draft`. For the *why* and the safety posture, see
[the draft-PR merge exclusion concept](../concepts/draft-pr-merge-exclusion.md).

The rail is a **pure narrowing** of the candidate set produced by the two
ready-PR producers. It preserves every existing gate (G2 author, G3 engineer-PR,
objective gates, MergeJudge) and never touches `PrSnapshot` or
`evaluate_objective_gates`, so the #1880 dashboard Merge Readiness panel is
unaffected.

## The exclusion rule

Draft state is represented as `Option<bool>` and the admission predicate is
**exact**:

```
admit iff  is_draft == Some(false)
```

| `is_draft` | Meaning | Admitted? |
|---|---|---|
| `Some(false)` | known: not a draft | **yes** (subject to all other gates) |
| `Some(true)` | known: draft | **no** |
| `None` | unknown (field absent/null) | **no** — fail-closed |

The predicate is a whole-value equality against `Some(false)`. It deliberately
does **not** use `unwrap_or(...)` or `!= Some(true)`, either of which could fail
*open* on an unknown draft state. Only an explicit, known `false` is admitted.

## `gh` field sets

`isDraft` is appended to the `--json` token in the three `gh` call sites the
merge-queue consumes, in
[`src/stewardship/merge_authority.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs):

```
# list_open_prs — the dashboard path:
gh pr list --repo <owner/repo> --state open \
  --json number,title,headRefName,baseRefName,mergeable,statusCheckRollup,url,author,labels,isDraft \
  --limit <limit>

# list_prs_by_author — the survey path (author filtered server-side):
gh pr list --repo <owner/repo> --state open --author <login> \
  --json number,title,headRefName,baseRefName,mergeable,statusCheckRollup,url,author,labels,isDraft \
  --limit <limit>

# view_pr — the per-PR snapshot path:
gh pr view <pr> --repo <owner/repo> \
  --json body,statusCheckRollup,mergeable,reviewDecision,baseRefName,labels,isDraft
```

Because both `gh pr list` call sites deserialize through the shared
`parse_pr_list_json`, `isDraft` is added to **both** `--json` strings — otherwise
the survey path would silently parse `None` for every row and (fail-closed)
exclude everything. The operative gate relies on the list path that feeds
`OpenPrSummary`; `view_pr` gains the field only for snapshot completeness and
consistency — `parse_pr_view_json` does not surface it onto `PrSnapshot`, which
stays untouched.

## Data types

### `OpenPrSummary.is_draft`

The open-PR listing summary
([`src/stewardship/merge_authority.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs))
carries draft state so candidates can be filtered without a second `gh`
round-trip:

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
    /// Draft state, from `gh pr list --json ...,isDraft`. Fail-closed
    /// `Option<bool>`:
    ///   - `Some(false)` — a real, mergeable (non-draft) PR; the ONLY admitted
    ///     value.
    ///   - `Some(true)` — a draft; excluded, because `gh pr merge` on a draft
    ///     always fails server-side (`Pull Request is still a draft`).
    ///   - `None` — the field was absent/null in the listing; treated as draft
    ///     (excluded), mirroring the sensor's fail-closed posture.
    ///
    /// Read by the draft-exclusion rail in `survey_ready_prs` and carried onto
    /// [`ProjectionCandidate`] for `project_ready_prs`. Not read by the #1880
    /// dashboard panel.
    pub is_draft: Option<bool>,
}
```

`is_draft` deserializes via `#[serde(default, rename = "isDraft")]` in the
listing's raw row: a present boolean becomes `Some(bool)`, and an absent or null
field becomes `None` (never panics). The `Default` derive gives `None`, so any
`OpenPrSummary::default()` user is fail-closed by construction. `to_snapshot()`
is **unchanged** — draft state does not enter `PrSnapshot`.

### `ProjectionCandidate.is_draft`

The projection candidate consumed by the agentic merge-queue's second producer
([`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs))
carries the same value, sourced 1:1 from `OpenPrSummary.is_draft` so both
producers share the invariant:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionCandidate {
    pub reasoned: ReasonedPr,
    pub author_login: String,
    pub head_ref: String,
    pub snapshot: PrSnapshot,
    /// Draft state from `gh` (`isDraft`), carried 1:1 from
    /// [`OpenPrSummary::is_draft`] so `project_ready_prs` shares the same
    /// fail-closed draft exclusion the sensor applies. `Some(false)` is the only
    /// admitted value; `Some(true)` and `None` are both excluded.
    pub is_draft: Option<bool>,
}
```

## Enforcement in both producers

The exclusion is applied in **both** places that produce `ready_prs`, so the
invariant cannot be bypassed by whichever producer is active.

### Producer #1 — `survey_ready_prs` (the deterministic sensor)

In [`src/overseer/merge_ops.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/merge_ops.rs),
the draft rail runs **after** the G3 engineer-PR block and **before** the
objective pre-filter, in the existing per-candidate loop where G2/G3 already
`continue` on exclusion:

```rust
// Draft rail (#4339): a draft can NEVER be merged — `gh pr merge` on a draft
// fails server-side ("Pull Request is still a draft"). Admit ONLY a known
// non-draft; a draft (`Some(true)`) or unknown draft state (`None`) is
// excluded (fail-closed), mirroring the author/label exclusions above. Pure
// narrowing — this can only remove candidates.
if pr.is_draft != Some(false) {
    // Optional single visible `[simard]` note that a draft was skipped.
    continue;
}
```

Placing it with the other narrowing gates guarantees it can only *remove* a PR
G2/G3 already admitted, never add one.

### Producer #2 — `project_ready_prs` (the agentic-reasoning projection)

In [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs),
the same exclusion is one more `.filter` in the projection chain, added after the
engineer-PR filter and before the objective-gate filter:

```rust
candidates
    .iter()
    .filter(|c| c.reasoned.disposition == PrDisposition::ReadyForMerge)
    .filter(|c| !c.author_login.eq_ignore_ascii_case(overseer_login))
    .filter(|c| {
        c.snapshot.labels.iter().any(|l| config::is_engineer_pr_label(l))
            || config::is_engineer_branch(&c.head_ref)
    })
    // Draft rail (#4339): admit ONLY a known non-draft; Some(true)/None excluded.
    .filter(|c| c.is_draft == Some(false))
    .filter(|c| evaluate_objective_gates(&c.snapshot, base_allowlist).is_ok())
    .map(|c| PrRef { repo: c.reasoned.repo.clone(), pr: c.reasoned.pr })
    .collect()
```

The `project_ready_prs` doc comment gains a fifth authorization criterion:

```
/// 5. the PR is NOT a draft — `is_draft == Some(false)` (a draft, or unknown
///    draft state, can never be merged, so it is excluded fail-closed).
```

> **Doc-comment lint note.** Any wrapped/continuation line of a `///` list item
> is indented 2+ spaces (as in criterion 5 above), so
> `cargo clippy --all-targets --all-features -- -D warnings` stays clean under
> `clippy::doc_lazy_continuation`.

## Error & edge-case matrix

| Condition | Result |
|---|---|
| `isDraft == false` in the listing | not excluded by this rail (other gates still apply) |
| `isDraft == true` in the listing | **excluded** (draft) in both producers |
| `isDraft` absent/null in the listing | `is_draft = None` ⇒ **excluded** (fail-closed) |
| Draft + author + label/branch + CI + `MERGEABLE` all otherwise passing | **excluded** — a draft is never a candidate |
| Non-draft with the same passing fields | **included** as candidate |
| Only `list_open_prs` (dashboard) requested `isDraft`, survey path not | N/A — both `--json` strings include `isDraft`; the survey path would otherwise fail-closed to exclude everything |

## Invariants

- **Pure narrowing.** The rail can only *remove* PRs from `ready_prs`; it never
  admits a PR the previous gates would reject and never broadens auto-merge
  eligibility.
- **Fail-closed on unknown.** Admission requires `is_draft == Some(false)`; both
  `Some(true)` and `None` are excluded. No `unwrap_or` and no `!= Some(true)`
  that could fail open.
- **Enforced in both producers.** `survey_ready_prs` and `project_ready_prs`
  each apply the exclusion; `ProjectionCandidate.is_draft` is sourced 1:1 from
  `OpenPrSummary.is_draft` so a missing filter in either producer would be a
  defect, not a silent divergence.
- **Objective gate untouched.** `PrSnapshot`, `to_snapshot()`, and
  `evaluate_objective_gates` are unchanged, so the #1880 dashboard is unaffected.
- **Server-side authority preserved.** GitHub's `mergePullRequest` still refuses
  a draft; this rail is defense-in-depth that saves the wasted attempt, not a
  replacement for the server check. `merge_authority` still never uses `--admin`
  or `--no-verify`.

## Tests

Focused unit tests follow the existing style in
[`src/overseer/tests_merge_queue_reasoning.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/tests_merge_queue_reasoning.rs)
and
[`src/overseer/tests_selfmerge_fix.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/tests_selfmerge_fix.rs):

- **`survey_ready_prs`** — a draft PR (`is_draft = Some(true)`) is excluded even
  when author + label/branch + CI + `MERGEABLE` all pass; an otherwise-identical
  non-draft (`Some(false)`) is included; a `None` draft state is excluded
  (fail-closed).
- **`project_ready_prs`** — the same three assertions on the projection path.
- **Fixtures** (`open_pr`, `candidate` / `green_engineer_snapshot`) default
  `is_draft` to `Some(false)` so every pre-existing green case stays green.
