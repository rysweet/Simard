---
title: ready_prs sensor API reference
description: >
  The Observe-path sensor that populates ObservedState.ready_prs — the survey
  seam on PrOps, the SIMARD_AUTOMERGE_REPOS allowlist and SIMARD_AUTOMERGE_AUTHOR
  identity resolvers, the OpenPrSummary.author field, the deterministic
  author + objective-gate pre-filter pipeline, fail-to-empty semantics, and the
  run_cycle enrichment call site.
last_updated: 2026-07-15
owner: simard
doc_type: reference
status: reference
related:
  - ../concepts/autonomous-self-merge-sensor.md
  - ./cross-repo-merge-authority.md
  - ./enrichment-observability-api.md
  - ./overseer-tick-details.md
  - ../howto/enable-autonomous-self-merge-canary.md
---

# `ready_prs` sensor API reference

This reference documents the deterministic sensor that populates
`ObservedState.ready_prs` in the acting Overseer's Observe pass, activating the
already-built autonomous self-merge path. For the *why* and the safety posture,
see [the autonomous self-merge sensor concept](../concepts/autonomous-self-merge-sensor.md).

The sensor produces a **candidate list only**. The authoritative merge decision
stays in [`merge_authority`](./cross-repo-merge-authority.md).

## Data types

### `PrRef`

Each entry in `ObservedState.ready_prs` is a `PrRef` — the minimal
`{ repo, pr }` reference the signal layer turns into a
`Signal::PrReadyToMerge { repo, pr }`.

```rust
pub struct PrRef {
    /// `owner/name`, e.g. "rysweet/Simard".
    pub repo: String,
    /// PR number.
    pub pr: u32,
}
```

### `OpenPrSummary.author`

The existing open-PR listing summary
([`src/stewardship/merge_authority.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs))
gains an `author` field so candidates can be author-filtered without a second
`gh` round-trip:

```rust
pub struct OpenPrSummary {
    pub number: u32,
    pub title: String,
    pub head_ref_name: String,
    pub base_ref_name: String,
    pub mergeable: String,
    pub checks: Vec<CheckRollupEntry>,
    pub url: String,
    /// PR author login, from `gh pr list --json ...,author` (`author.login`).
    /// Added so the ready_prs sensor can keep Simard's own PRs and drop
    /// human-authored ones.
    pub author: String,
}
```

The backing `gh` command grows one JSON field:

```
gh pr list --repo <owner/repo> --state open \
  --json number,title,headRefName,baseRefName,mergeable,statusCheckRollup,url,author \
  --limit <limit>
```

`parse_pr_list_json` reads `author.login` into `OpenPrSummary.author`. A missing
or null `author` parses to an empty string, which the exact-equality filter
rejects (fail-closed).

## Configuration resolvers

Both resolvers follow the repo's established `*_from(lookup)` testable pattern in
[`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs):
a pure `_from` function takes an environment lookup closure (unit-testable), and
a thin production entry reads the real process environment.

### `SIMARD_AUTOMERGE_REPOS` — the allowlist (default OFF)

```rust
/// Parse the comma-separated allowlist. Unset/empty/whitespace-only ⇒ empty set
/// ⇒ autonomous self-merge OFF. Entries are trimmed; blank entries dropped.
pub fn automerge_repos_from(lookup: impl Fn(&str) -> Option<String>) -> Vec<String>;

/// Production entry: reads SIMARD_AUTOMERGE_REPOS from the process environment.
pub fn automerge_repos() -> Vec<String>;
```

| `SIMARD_AUTOMERGE_REPOS` | Eligible repos |
|---|---|
| unset | **none** — autonomous self-merge OFF |
| `""` / whitespace | **none** — OFF |
| `rysweet/Simard` | just `rysweet/Simard` (canary) |
| `rysweet/Simard,rysweet/azlin` | both listed repos |

A repo is eligible **only** on exact `owner/name` match. Unknown or unset ⇒ the
repo is not surveyed and contributes zero candidates.

### `SIMARD_AUTOMERGE_AUTHOR` — the own-PR identity

```rust
/// The gh login whose PRs Simard may auto-merge, read from
/// `SIMARD_AUTOMERGE_AUTHOR`. Pure env projection: `None` when unset.
pub fn automerge_author_from(lookup: impl Fn(&str) -> Option<String>) -> Option<String>;

/// Production entry. When SIMARD_AUTOMERGE_AUTHOR is unset, falls back to the
/// authenticated gh identity (`gh api user` → `.login`), resolved once and
/// cached. The `gh` side effect lives here (and/or in the survey impl), never
/// in the pure `_from` resolver.
pub fn automerge_author() -> Option<String>;
```

> **Design note.** The `gh api user` fallback cannot live inside
> `automerge_author_from`, whose only input is a pure environment lookup
> closure. Keep the identity fallback in the production entry (or the survey
> impl) so the `_from` function stays deterministic and unit-testable.

This is the **OODA/engineer** identity — the daemon's authenticated `gh` user —
and is **distinct** from `SIMARD_OVERSEER_AUTHOR_LOGIN`
(`simard-overseer[bot]`), which the `RecursionGuard` refuses. Keeping the two
identities separate is what lets Simard's own engineering PRs survive the guard
while the overseer-bot's PRs stay refused. If the author cannot be resolved, the
sensor returns an **empty** candidate list (fail-closed).

## The survey seam

A single trait method on the `PrOps` capability
([`src/overseer/capabilities.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/capabilities.rs))
carries the survey. It returns a **`Vec`, not a `Result`**, so the type system
forces fail-to-empty — there is no error variant that could accidentally be
mapped to a merge.

```rust
pub trait PrOps {
    // ... existing methods ...

    /// Survey allowlisted repos for Simard-authored, green + MERGEABLE PRs.
    /// Returns candidate references only — never merges. Any per-repo error
    /// is logged (`tracing::warn!`) and that repo contributes nothing;
    /// the method never returns an error and never panics.
    ///
    /// Default impl returns an empty Vec so existing fakes need no stub.
    fn survey_ready_prs(&self, _allowlist: &[String]) -> Vec<PrRef> {
        Vec::new()
    }
}
```

The production implementation lives on `MergePrOps`
([`src/overseer/merge_ops.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/merge_ops.rs)).

### Pipeline (`MergePrOps::survey_ready_prs`)

For each `repo` in the allowlist, in order:

1. **List** — `list_open_prs(repo, limit)`. On error: `warn!` and skip this repo.
2. **Author filter** — keep PRs where `summary.author == automerge_author()`
   (exact equality). No `contains` / prefix / regex — an empty or mismatched
   author is dropped.
3. **Objective pre-filter** — project each surviving `OpenPrSummary` to a
   `PrSnapshot` (via `to_snapshot()`) and run the existing
   `evaluate_objective_gates(&snap, &self.base_allowlist)`: base-branch
   allowlist, `mergeable == "MERGEABLE"`, and every `statusCheckRollup` entry in
   `{SUCCESS, NEUTRAL, SKIPPED}`. The base allowlist passed here **must be the
   same `self.base_allowlist` the authoritative gate uses** (seeded from
   `base_allowlist_from_env()`); reusing it is what guarantees the *additive
   strictness* invariant — a looser base list in the sensor could admit a
   candidate the gate rejects. The merge-judge is **not** run here.
4. **Collect** the survivors as `PrRef { repo, pr: number }`.

The result is the concatenation of survivors across all allowlisted repos. An
empty allowlist means the loop body never runs and the result is empty.

## Call site (enrichment)

`ObservedState.ready_prs` is populated in the acting `run_cycle` enrichment path
([`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)),
after `observed_from_snapshot` produces the side-effect-free projection:

```rust
// Enrichment (acting run_cycle), alongside blocked_goals / workstream_gaps / recall.
let allowlist = config::automerge_repos();
observed.ready_prs = caps.prs.survey_ready_prs(&allowlist); // empty allowlist ⇒ empty
```

`config::automerge_repos()` reads the process environment. Because a
systemd-managed daemon's environment is fixed for the life of the process,
changing `SIMARD_AUTOMERGE_REPOS` requires a daemon **restart** to take effect
regardless of whether the value is cached or re-read per cycle.

> **Observability requirement.** The survey impl **must** emit a structured
> `tracing` line (target `overseer::merge_ops`, e.g. an `info!` naming each
> included candidate and a `warn!` per skipped repo). The
> [canary runbook](../howto/enable-autonomous-self-merge-canary.md) relies on
> operators grepping this line to confirm the wire is live; without it, the only
> observable signal is the downstream `PrReadyToMerge`.

`observed_from_snapshot`
([`src/overseer/sensor.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/sensor.rs))
continues to set `ready_prs: Vec::new()`; its comment now points here rather than
describing an unfilled "M2 capability".

## Downstream (unchanged)

Once `ready_prs` is non-empty, the existing chain runs untouched:

| Stage | Location |
|---|---|
| `Signal::PrReadyToMerge { repo, pr }` per entry | `src/overseer/signal.rs` |
| `ProblemKind::DeliveryReady` | Orient |
| `Intervention::VerifyAndMergePr` (`allow_verify_merge = true`) | `src/overseer/mod.rs` |
| `caps.prs.merge()`: RecursionGuard admit → `verify()` checklist → poll-until-green | `src/overseer/merge_ops.rs` |
| Authoritative gate `merge_pr_if_merge_ready_with_judge` (objective gates + merge-judge) → `gh pr merge --squash --delete-branch` | `src/stewardship/merge_authority.rs` |

## Error & edge-case matrix

| Condition | Result |
|---|---|
| Allowlist unset/empty | `ready_prs = []` (OFF) |
| Author unresolved | `ready_prs = []` |
| `list_open_prs` errors for a repo | `warn!`, that repo skipped, others continue |
| PR authored by a human | excluded |
| PR authored by `simard-overseer[bot]` | excluded (wrong identity) + refused later by `RecursionGuard` |
| PR `mergeable != "MERGEABLE"` (e.g. `CONFLICTING`) | excluded |
| Any check `FAILURE`/`PENDING`/`IN_PROGRESS`/… | excluded |
| PR base branch not in allowlist | excluded |
| Green + `MERGEABLE` + own-author + allowlisted repo | **included** as candidate |

## Invariants

- The sensor **never merges**. It returns `Vec<PrRef>` and nothing else.
- Fail-closed and fail-visible: every failure yields an empty list plus a log
  line, never a silent wrong merge.
- The authoritative merge gate in `merge_authority` is unchanged and still
  never uses `--admin` or `--no-verify`.
- The pre-filter is additive strictness only — it cannot admit a merge the
  authoritative gate would refuse.
