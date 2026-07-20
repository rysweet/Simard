---
title: Cross-repo merge authority reference
description: Simard's gated, repo-parameterized squash-merge authority — the objective-gates + merge-judge pipeline that lands a merge-ready PR in any repo Simard governs (default rysweet/Simard), and the `simard merge-pr <PR> --repo <owner/repo>` operator CLI that exposes it cross-repo.
last_updated: 2026-06-29
owner: simard
doc_type: reference
status: reference
related:
  - ../concepts/operational-autonomy-model.md
  - ./pr-finalization-pipeline.md
  - ./simard-cli.md
  - ../concepts/stewardship-mode.md
  - ../concepts/draft-pr-merge-exclusion.md
  - ./draft-pr-merge-gate.md
  - ../howto/edit-the-engineer-system-prompt.md
---

# Cross-repo merge authority reference

Simard's **merge authority** is her gated power to squash-merge a pull request
once it has independently demonstrated merge-readiness. The authority is
**repo-parameterized**: the same deterministic objective gates plus the agentic
merge-judge that land a `rysweet/Simard` PR can land a merge-ready PR in **any
repo Simard governs** — `rysweet/azlin`, `rysweet/gadugi-agentic-test`,
`rysweet/agent-kgpacks`, and the rest of the ecosystem.

The target repository is a **parameter** that **defaults to `rysweet/Simard`**
for backward compatibility. Cross-repo merges run through this **gated** authority
rather than a bare `gh pr merge`, so the same evidence and safety gates apply
everywhere Simard merges.

This reference documents the library API
([`src/stewardship/merge_authority.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs))
and the operator CLI (`simard merge-pr`). For the *why* — when Simard merges
without waiting on a human approver — see the
[operational autonomy model](../concepts/operational-autonomy-model.md).

## Pipeline

For a given `(pr_number, repo)` the authority runs, in order:

1. **Snapshot** — `gh pr view <PR> --repo <owner/repo> --json
   body,statusCheckRollup,mergeable,reviewDecision,baseRefName,labels,isDraft`.
2. **Objective gates** (deterministic, never agentic):
   - **Base-branch allowlist** — `baseRefName` must be in the configured
     allowlist (default `["main"]`, overridable via `SIMARD_MERGE_BASE_ALLOWLIST`).
     This is the **first** gate, so a PR targeting a stale or wrong base branch is
     refused before anything else runs.
   - **Not a draft** — `isDraft == false`. A GitHub draft PR is never merge-ready
     even when `MERGEABLE` with all checks green, because `mergePullRequest`
     refuses a draft outright. See the
     [draft-PR merge gate reference](./draft-pr-merge-gate.md).
   - **Mergeable** — `mergeable == "MERGEABLE"`.
   - **CI green** — every `statusCheckRollup` entry is `SUCCESS`, `NEUTRAL`, or
     `SKIPPED`. Any `FAILURE`, `CANCELLED`, `TIMED_OUT`, `STARTUP_FAILURE`,
     `ACTION_REQUIRED`, `PENDING`, `QUEUED`, or `IN_PROGRESS` blocks the merge.
3. **Agentic gate (merge-judge)** — a prompt-driven judge reads the PR body and
   returns a structured verdict on whether the merge-ready evidence criteria are
   satisfied. Its prompt
   (`prompt_assets/simard/merge_readiness_judge.md`) is the single source of truth
   for the evidence criteria. Only a `Ready` verdict passes.
4. **Merge** — if every gate passes: `gh pr merge <PR> --squash --delete-branch
   --repo <owner/repo>`, returning `MergeOutcome::Merged { pr_number, repo }`.
5. **Refuse** — otherwise `MergeOutcome::Refused { pr_number, reason }`, where
   `reason` is the first failing objective gate, or the judge's blocker summary if
   every objective gate passed.

> **The `repo` parameter threads through the entire pipeline.** Every `gh`
> invocation receives `--repo <owner/repo>`, the merge-judge is told which repo it
> is judging, and the resulting `MergeOutcome::Merged { repo }` carries the slug
> back. There is no hardcoded `rysweet/Simard` in the pipeline — the home repo is
> only the **default**, not a baked-in constant.

## Library API

```rust
use simard::stewardship::{
    MergeOutcome, PrGhClient, RealPrGhClient, merge_pr_if_merge_ready,
};
```

### Entry points

| Function | Signature (abridged) | Use |
|----------|----------------------|-----|
| `merge_pr_if_merge_ready` | `(pr_number: u32, repo: &str, gh: &dyn PrGhClient) -> SimardResult<MergeOutcome>` | Production entry point. Reads the base-branch allowlist from `SIMARD_MERGE_BASE_ALLOWLIST`. |
| `merge_pr_if_merge_ready_with_allowlist` | `(pr_number, repo, gh, base_allowlist: &[String]) -> SimardResult<MergeOutcome>` | Explicit allowlist (tests / callers that bypass the env). |
| `merge_pr_if_merge_ready_with_judge` | `(pr_number, repo, gh, base_allowlist, judge: &dyn MergeJudge) -> SimardResult<MergeOutcome>` | Full control: inject a custom or stub `MergeJudge`. |

All three take `repo: &str` as the target-repo parameter. Callers that want the
home-repo default pass `"rysweet/Simard"` (the operator CLI does this when
`--repo` is omitted).

### `MergeOutcome`

```rust
pub enum MergeOutcome {
    /// The PR satisfied every gate and was squash-merged.
    Merged { pr_number: u32, repo: String },
    /// The PR did not satisfy a gate (single actionable sentence).
    Refused { pr_number: u32, reason: String },
}
```

`Refused` is an **expected** result, not an error — it means "evaluated, not
ready". An `Err` is returned only when the PR could not be *evaluated* at all
(`gh` failed to run, returned malformed JSON, the judge errored at the network
layer, or `gh pr merge` itself failed despite the gates passing).

### `PrGhClient` — the seam

```rust
pub trait PrGhClient {
    fn view_pr(&self, repo: &str, pr_number: u32) -> SimardResult<PrSnapshot>;
    fn squash_merge(&self, repo: &str, pr_number: u32) -> SimardResult<()>;
    fn list_open_prs(&self, repo: &str, limit: u32) -> SimardResult<Vec<OpenPrSummary>>;
}
```

`RealPrGhClient` is the production implementation; every method passes
`--repo {repo}` to `gh`. Tests drive an in-memory `PrGhClient` that records the
`repo` slug it was called with, so the cross-repo path is verified without a
network call.

### Resilience — transient `gh` retry

`gh` shells out to GitHub over the network, so its calls can fail for reasons
that have nothing to do with the PR: secondary rate limits (HTTP 429), GitHub
availability blips (502/503/504), DNS hiccups, connection resets, and TLS or
request timeouts. For an autonomous self-merging agent these transient blips
must not abort a merge-ready promotion.

`RealPrGhClient` therefore wraps its **idempotent read** calls — `view_pr`
(`gh pr view`) and `list_open_prs` (`gh pr list`) — in `retry_transient_gh`,
a bounded loop (`GH_READ_MAX_RETRIES = 3`) with a linear backoff
(`GH_RETRY_BACKOFF_MS = 500` × attempt). A failure is retried **only** when
`is_transient_gh_failure` matches a transient network/availability signature in
the error text; deterministic failures (auth, not-found, not-mergeable, bad
flags, gate refusals) surface immediately instead of looping. The classifier
mirrors the substring heuristic the OODA adaptive scaler already uses for 429
detection.

`squash_merge` (`gh pr merge`) is a **mutation and is deliberately
single-attempt**. Its safe retry boundary is the gate-revalidating
`merge_pr_if_merge_ready` cycle, which re-`view`s the PR and re-checks every
gate before any new merge attempt — rather than a blind inner loop that could
act on stale PR state. The read retries above already make that re-validation
resilient to transient flakiness.

## Operator CLI — `simard merge-pr`

```text
Usage: simard merge-pr <PR-number> [--repo <owner/repo>]

Squash-merges the given PR if it passes Simard's merge-readiness checks
(base-branch allowlist, mergeable, CI green, merge-judge verdict).

  <PR-number>          The pull-request number to evaluate and merge.
  --repo <owner/repo>  Target repository. Defaults to rysweet/Simard.
```

The `--repo` flag exposes the repo parameter at the command line. It **defaults
to `rysweet/Simard`**, so every pre-existing invocation (`simard merge-pr 1500`)
behaves exactly as before.

### Examples

Merge a home-repo PR (default target — unchanged behavior):

```bash
simard merge-pr 1500
# → merged: PR #1500 in rysweet/Simard (squash + delete-branch)
```

Merge a cross-repo PR through the **gated** authority:

```bash
simard merge-pr 42 --repo rysweet/azlin
# → merged: PR #42 in rysweet/azlin (squash + delete-branch)
```

A PR that is not ready is **refused** (printed to stderr, non-zero exit) with a
single actionable reason — the same on any repo:

```bash
simard merge-pr 7 --repo rysweet/gadugi-agentic-test
# → refused: PR #7 not merge-ready: CI check "build" is FAILURE
```

Refusal exits non-zero so a shell script can detect "blocked" without losing the
reason; the reason text is the first failing gate or the judge's blocker summary.

## How engineers use it

Engineers land their PRs through this authority rather than a bare `gh pr merge`.
The merge verb is repo-uniform:

- **Home-repo PR** → `simard merge-pr <PR>` (defaults to `rysweet/Simard`).
- **Cross-repo PR** in a governed repo → `simard merge-pr <PR> --repo <owner/repo>`.

This routes **every** merge — home and cross-repo — through the objective gates +
merge-judge, instead of letting cross-repo merges bypass the judge via an ungated
`gh pr merge`. A bare `gh pr merge` is the fallback only where `simard merge-pr`
is genuinely unavailable. See the
[PR-finalization review pipeline](./pr-finalization-pipeline.md), stage 4.

### Frozen-pin build-dependency repos need a follow-up

Three governed repos are also **build dependencies** Simard pins by exact git
rev in her own `Cargo.toml`: `rysweet/amplihack-rs`, `rysweet/amplihack-memory-lib`,
and `rysweet/RustyClawd`. `simard merge-pr --repo` operates on them like any other
governed repo, **but the cross-repo merge is not the finish line** — a fix only
reaches Simard's own build after the engineer **bumps the matching `rev = …` pin**
in `Cargo.toml` and re-verifies with `cargo build`. For that reason the examples
in this reference deliberately use non-pinned repos so the merge-and-done path is
unambiguous. See the pin-bump rule in the
[engineer system prompt](../howto/edit-the-engineer-system-prompt.md).

## Preserved gates

The cross-repo path does **not** weaken any gate. Applied to every repo:

- **Base-branch allowlist** (`SIMARD_MERGE_BASE_ALLOWLIST`, default `["main"]`) —
  evaluated first.
- **All required CI checks green** — any non-success rollup entry blocks.
- **Merge-judge verdict** — only `Ready` passes; `NotReady` / `Unclear` refuse.
- **Squash + delete-branch** merge shape — unchanged.

Autonomy means Simard does not wait for a **human** approver on a governed repo
that has none — it does **not** mean she skips these gates. See
[operational autonomy model](../concepts/operational-autonomy-model.md).

## Invariants

- **Repo-parameterized, home-default.** Every entry point and the CLI take the
  target repo as a parameter defaulting to `rysweet/Simard`; the slug threads
  through `view_pr`, the judge, `squash_merge`, and `MergeOutcome::Merged { repo }`.
- **Gates first, never bypassed cross-repo.** Objective gates run before the
  judge; the judge runs before the merge; this holds identically for every repo.
- **Refused ≠ Err.** A non-ready PR is `Refused` (expected output); `Err` is
  reserved for "could not evaluate".
- **Back-compatible.** `simard merge-pr <PR>` with no `--repo` targets
  `rysweet/Simard` exactly as before.
- **Resilient reads, single-attempt mutation.** Idempotent `gh` reads
  (`view_pr`, `list_open_prs`) retry transient network/availability failures
  with a bounded backoff; the `squash_merge` mutation is single-attempt, with
  the gate-revalidating `merge_pr_if_merge_ready` cycle as its retry boundary.

## Related reading

- [Operational autonomy model](../concepts/operational-autonomy-model.md) — when
  and why Simard self-merges without a human approver.
- [PR-finalization review pipeline](./pr-finalization-pipeline.md) — the bounded
  review→merge pipeline whose stage 4 invokes this authority.
- [Simard CLI reference](./simard-cli.md) — the full shipped command tree.
- [Goal stewardship mode](../concepts/stewardship-mode.md) — the ecosystem of
  repos Simard governs and merges across.
- [Engineer system prompt](../howto/edit-the-engineer-system-prompt.md) — the
  pin-bump follow-up required after a cross-repo merge in a frozen-pin
  build-dependency repo.
