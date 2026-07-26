---
title: "Reference: Engineer-Inspect Worktree Resolution"
description: >
  How the engineer-loop inspect phase resolves the engineer's real worktree
  before probing it, the additive SimardError::MissingWorktree variant that
  distinguishes an absent worktree from a NotARepo failure, and the fail-closed
  guarantee that a valid-but-idle engineer is never NOT_A_REPO reaped
  (issue #4744).
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./claim-reaper-api.md
  - ./investigate-stale-engineer-api.md
  - ./engineer-worktree-isolation.md
  - ./engineer-claim-release-api.md
  - ../howto/inspect-and-clean-engineer-worktrees.md
  - ../howto/investigate-a-stale-engineer-before-reap.md
---

# Reference: Engineer-Inspect Worktree Resolution

> **Status: implemented.** Present-tense description of shipped behaviour.
> Primary sources:
> [`src/engineer_loop/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/engineer_loop/mod.rs),
> [`src/engineer_worktree/claim.rs`](https://github.com/rysweet/Simard/blob/main/src/engineer_worktree/claim.rs),
> [`src/error/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/error/mod.rs).
> Tracked by [issue #4744](https://github.com/rysweet/Simard/issues/4744).

## Overview

The engineer-loop **inspect phase** examines an engineer's working tree to
decide whether the engineer is making progress. Previously the phase could probe
a synthetic, non-repository path (for example a bare `/tmp` directory that is not
a git worktree). `git` returned exit code 128 (`fatal: not a git repository`),
the inspection surfaced [`SimardError::NotARepo`], and the engineer was recorded
as producing nothing. A healthy-but-idle engineer was then **false-stale reaped**,
discarding whole engineering loops (the goal-board blocker behind `7f5afcca` and
the repeated no-action/blocked cycles observed in `simard status`).

Inspect now **resolves the engineer's real worktree** at the probe seam before
running any `git` command, and distinguishes three outcomes:

| Situation | Outcome | Reap? |
| --- | --- | --- |
| Valid worktree, engineer idle | Healthy inspection, `worktree_dirty` reflects real state | **No** |
| Worktree directory genuinely absent | [`SimardError::MissingWorktree`] | Handled distinctly — not a `NotARepo` false positive |
| A path that exists but is not a git repo | [`SimardError::NotARepo`] | Only genuine non-repos |

The invariant: **a valid engineer worktree never yields `NotARepo`.**

## Worktree resolution seam

The inspect phase no longer accepts an arbitrary caller-supplied path as the repo
root. It resolves the worktree the engineer loop already tracks through
`engineer_worktree`:

```rust
// src/engineer_worktree/claim.rs
/// Resolve the on-disk worktree path for the engineer holding `claim_key`.
///
/// Returns the canonicalized worktree root when the directory exists and lives
/// under the managed engineer-worktree root. Returns `SimardError::MissingWorktree`
/// when the claim is known but its worktree directory is absent (reaped, swept,
/// or never allocated) — a distinct, fail-closed signal, never `NotARepo`.
pub fn resolve_engineer_worktree(claim_key: &str) -> SimardResult<PathBuf>;
```

`inspect_workspace` (in `engineer_loop/mod.rs`) is driven from the resolved path:

```rust
// src/engineer_loop/mod.rs
pub fn inspect_workspace(workspace_root: &Path, state_root: &Path) -> SimardResult<RepoInspection>;
```

The caller resolves the worktree first, so the `workspace_root` passed to
`inspect_workspace` is always the engineer's real, canonicalized tree — never a
synthetic `/tmp` default.

### Path safety

Resolution is defensive by construction:

- the resolved path is **canonicalized** (`fs::canonicalize`), collapsing `..`
  and resolving symlinks;
- the canonical path is confirmed to live **within the managed engineer-worktree
  root**; a symlink that escapes the root is rejected;
- no engineer-controlled path fragment is ever interpolated into a shell — all
  `git` invocations use argv arrays.

## API

### `SimardError::MissingWorktree`

An additive variant of the crate error enum
([`src/error/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/error/mod.rs)).
It is **non-breaking**: existing `match` arms that already handle `NotARepo`
continue to compile because `MissingWorktree` is a new, separate arm.

```rust
pub enum SimardError {
    // ...
    /// The path is a real git repository but could not be inspected.
    NotARepo { path: PathBuf, reason: String },

    /// A known engineer claim's worktree directory is absent.
    ///
    /// Distinct from `NotARepo`: the engineer is not "not a repo", the worktree
    /// simply does not exist on disk (reaped, swept, or never allocated). The
    /// reaper treats this as a genuinely-missing worktree, NOT as a healthy
    /// engineer producing nothing, so it never triggers a false-stale reap of a
    /// live-but-idle engineer.
    MissingWorktree { claim_key: String, expected_path: PathBuf },
    // ...
}
```

Its `Display` renders a log-safe, PII-free line (claim key + expected path, no
secrets, no raw subprocess output).

### Inspection outcomes

| Return | Meaning | Reaper interpretation |
| --- | --- | --- |
| `Ok(RepoInspection)` | Worktree resolved and inspected | Idle ≠ dead; `worktree_dirty` reflects real changes |
| `Err(MissingWorktree { .. })` | Worktree directory genuinely absent | Distinct missing-worktree signal (fail-closed) |
| `Err(NotARepo { .. })` | Path exists but is not a git repo | Only genuine non-repos |

## Configuration

No new configuration knobs. Behaviour is additive and on by default; the managed
engineer-worktree root is the existing one used by
[`engineer_worktree`](./engineer-worktree-isolation.md).

## Examples

### A valid, idle engineer is inspected — not reaped

```text
inspect: claim=engineer:goal-7f5afcca worktree=/…/worktrees/eng-7f5afcca
inspect: worktree_dirty=false  (engineer idle, checkpoint resumable)
reaper : verdict=still-alive  (idle ≠ dead) → claim KEPT
```

Before this change the same engineer produced:

```text
inspect: NOT_A_REPO (git exit 128) path=/tmp/…
reaper : engineer produced nothing → FALSE-STALE REAP
```

### A genuinely-missing worktree is reported distinctly

```text
inspect: MissingWorktree claim=engineer:goal-abc expected=/…/worktrees/eng-abc
```

This is surfaced as its own outcome rather than being conflated with a
`NotARepo` failure of a healthy engineer.

## Fail-closed guarantees

- A valid engineer worktree **never** yields `NotARepo`.
- An **idle** engineer (no new files, resumable checkpoint) is distinguished from
  a **dead** one; idleness alone never reaps.
- A genuinely absent worktree is a distinct, explicit signal
  (`MissingWorktree`), keeping the reap decision honest.

## Regression tests

Co-located in
[`src/engineer_loop/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/engineer_loop/mod.rs)
and [`src/error`](https://github.com/rysweet/Simard/tree/main/src/error):

- `inspect_on_valid_worktree_is_not_not_a_repo` — inspecting a real engineer
  worktree returns `Ok(..)`, never `NotARepo`.
- `inspect_resolves_engineer_worktree_not_synthetic_tmp` — the probe targets the
  resolved worktree, not a `/tmp` default.
- `missing_worktree_is_distinct_from_not_a_repo` — an absent worktree yields
  `MissingWorktree`, and the two variants are not equal.
- `valid_idle_engineer_is_not_false_stale_reaped` — an idle-but-live engineer is
  kept, closing the #4744 false-reap chain.

## Related

- [Stale-Engineer-Claim Reaper API](./claim-reaper-api.md)
- [Investigate-Before-Reap API](./investigate-stale-engineer-api.md)
- [Engineer-Worktree Isolation](./engineer-worktree-isolation.md)
- How-to: [Inspect and clean engineer worktrees](../howto/inspect-and-clean-engineer-worktrees.md)
