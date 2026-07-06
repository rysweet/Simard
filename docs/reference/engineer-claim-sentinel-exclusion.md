---
title: Engineer Claim-Sentinel Exclusion
description: Reference for the two-layer defense that keeps Simard's private per-worktree liveness sentinel (.simard-engineer-claim) from ever registering as an uncommitted change, so the engineer-loop pre-mutation guard cannot trip on it in a target repo that does not gitignore it.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./engineer-worktree-isolation.md
  - ./engineer-worktree-sweep-safety.md
  - ./engineer-loop-argv-sanitization.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
---

# Engineer Claim-Sentinel Exclusion

Simard writes a private liveness sentinel — `.simard-engineer-claim` — into
every engineer worktree it allocates (issue #1213, refined in #1238). The
sentinel records the `(pid, starttime)` of the process that owns the worktree
so the reaper and startup sweep can distinguish a live claimant from a
recycled PID.

The sentinel is **Simard infrastructure, not a user change**. This page
documents the two-layer defense (issue #2621) that guarantees the sentinel is
never counted as an uncommitted change by any `git status` consumer in the
engineer loop, in any governed repo — including external repos that have no
reason to gitignore a Simard-private file.

## Background — the pre-mutation guard deadlock

The engineer loop runs a **pre-mutation guard** before spawning a coding agent
for any mutating objective (issue #2082):

```rust
// src/engineer_loop/mod.rs
let analyzed = analyze_objective(objective);
if analyzed.is_mutating() && inspection.worktree_dirty {
    // abort with SimardError::DirtyWorktree { changed_files }
    return Err(err);
}
```

`inspection.worktree_dirty` is derived from `git status --short
--untracked-files=all`. Because Simard drops `.simard-engineer-claim` into the
worktree at allocation time, a freshly allocated worktree in a repo that does
**not** gitignore the sentinel reports exactly one untracked change:

```
?? .simard-engineer-claim
```

Before the fix, that single untracked sentinel made `worktree_dirty == true`,
so the guard aborted **every** mutating engineer with `DirtyWorktree`
*before the coding agent was ever spawned*. The daemon then re-dispatched the
same goal, allocated another worktree, tripped the same guard, and repeated —
an **infinite engineer-dispatch loop** that racked up consecutive-failure
demotions on the goal board and blocked all `agent-kgpacks-rs` workstream goals
(#12/#16/#17/#18/#19/#20/#21).

## The sentinel constant

There is a single source of truth for the sentinel filename, shared by both
defense layers:

```rust
// src/engineer_worktree/mod.rs
pub const ENGINEER_CLAIM_FILE: &str = ".simard-engineer-claim";
```

All loop logic references this constant. There are no literal
`".simard-engineer-claim"` strings in the guard or filter paths.

## Two-layer defense

The sentinel is hidden by two independent, mutually-reinforcing layers. Either
layer alone is sufficient to prevent the deadlock; together they are
belt-and-suspenders so a failure in one is transparently covered by the other.

```
allocation time                         inspection time
───────────────                         ───────────────
Worktree::allocate()                    inspect_workspace()
  │                                       │
  ├─ git worktree add …                   ├─ git status --short --untracked-files=all
  │                                       │
  ├─ Layer 1:                             ├─ Layer 2:
  │   exclude_engineer_claim(dir)         │   strip_claim_sentinel(parse_status_paths(...))
  │   → append `/.simard-engineer-claim`  │   → drop exact-root ".simard-engineer-claim"
  │     to the worktree's info/exclude    │
  │   (git status no longer lists it)     ├─ worktree_dirty = !changed_files.is_empty()
  │                                       │
  └─ write .simard-engineer-claim         └─ pre-mutation guard sees a clean tree
```

### Layer 1 — creation-time exclude (`git status` never lists it)

At worktree allocation, immediately after `git worktree add`, Simard appends a
root-anchored exclude entry for the sentinel to the worktree's git exclude
file so `git status` in the target repo never reports it as untracked.

```rust
// src/engineer_worktree/mod.rs
/// Append an anchored exclude entry for [`ENGINEER_CLAIM_FILE`] to
/// `worktree_dir`'s git exclude file so the Simard-managed sentinel is never
/// reported as an untracked change by `git status` in the target repo.
fn exclude_engineer_claim(worktree_dir: &Path) -> Result<(), String>;
```

Behavior:

| Property                | Detail                                                                                                   |
| ----------------------- | -------------------------------------------------------------------------------------------------------- |
| Exclude-path resolution | `git rev-parse --git-path info/exclude`, run **inside the worktree** via the `git_capture` helper. Never hardcodes `.git/info/exclude` — for a linked worktree the real path lives in the shared common git dir. |
| Written pattern         | **Root-anchored**: `/.simard-engineer-claim` (leading `/`). Excludes only the worktree-root sentinel, never a same-basename file nested in a subdirectory. |
| Idempotency             | Reads the existing exclude first; appends only if neither the anchored line nor a legacy bare line is already present. Repeated allocations against the same parent repo never duplicate the entry. |
| Newline hygiene         | Ensures a trailing newline before appending so it never concatenates onto a prior unterminated line. |
| Parent creation         | Creates the `info/` directory if absent.                                                                 |
| Concurrency             | Serialized under `worktree_mutation_lock()` — the same lock guarding `git worktree add`. Because `info/exclude` lives in the shared common git dir for linked worktrees, concurrent allocations against the same parent repo would otherwise race a non-atomic read-modify-write and could clobber the repo's pre-existing exclude entries. |
| Failure mode            | **Non-fatal.** Logged at `tracing::warn!` (target `simard::engineer_worktree`) and allocation continues. Layer 2 still hides the sentinel if this append fails. |

The git exclude file is repo-local and never committed — and for a linked
engineer worktree it lives in the parent repo's shared common git dir (which is
why `git rev-parse --git-path` is used to locate it, and why the write is
serialized under `worktree_mutation_lock`) — so excluding a Simard-private
filename there has no effect on the target repo's history.

### Layer 2 — read-time filter (defense in depth)

Every `git status` consumer in the engineer loop routes its parsed path list
through a single shared filter before using it:

```rust
// src/engineer_loop/mod.rs
/// Drop the Simard-managed claim sentinel from a parsed `git status` path list.
fn strip_claim_sentinel(paths: Vec<String>) -> Vec<String> {
    paths
        .into_iter()
        .filter(|path| path != crate::engineer_worktree::ENGINEER_CLAIM_FILE)
        .collect()
}
```

Match semantics: **exact root-path equality**. `parse_status_paths` yields
root-relative paths, so the sentinel appears as exactly
`.simard-engineer-claim`. The filter removes that and only that:

- A same-basename file under a subdirectory (`subdir/.simard-engineer-claim`)
  is a real change and is **kept** — no suffix or substring matching.
- Every other path is preserved, in order.

Two consumers share this filter, so they can never drift:

1. **`inspect_workspace`** — the pre-mutation-guard input. The filter is
   applied before `worktree_dirty` is computed:

   ```rust
   // src/engineer_loop/mod.rs — inspect_workspace()
   let changed_files = strip_claim_sentinel(parse_status_paths(&status_output.stdout));
   let worktree_dirty = !changed_files.is_empty();
   ```

2. **`verify_agent_spawn_artifacts`** — the post-session evidence report. The
   same filter runs on the post-status path list so a no-op agent session
   whose only on-disk side-effect is the sentinel is **not** falsely reported
   as `"verified"`. This matters specifically on the *degraded* path where the
   Layer 1 exclude append failed and raw `git status` still lists the sentinel.

## Result — the guard sees a clean tree

With either layer active, a worktree whose only change is the claim sentinel
yields `worktree_dirty == false`. The pre-mutation guard therefore emits no
`DirtyWorktree`, the coding agent spawns normally, and the re-dispatch loop is
broken.

| Worktree state                                   | `changed_files`             | `worktree_dirty` | Mutating guard |
| ------------------------------------------------ | --------------------------- | ---------------- | -------------- |
| Only `.simard-engineer-claim`                    | `[]`                        | `false`          | passes         |
| `.simard-engineer-claim` + `user_change.txt`     | `["user_change.txt"]`       | `true`           | aborts (correct) |
| `subdir/.simard-engineer-claim` (real, nested)   | `["subdir/.simard-engineer-claim"]` | `true`   | aborts (correct) |

## Configuration

There is nothing to configure. Both layers are unconditional for every
daemon-allocated engineer worktree. The sentinel filename is fixed by the
`ENGINEER_CLAIM_FILE` constant. Governed target repos do **not** need to add
`.simard-engineer-claim` to their `.gitignore` — Simard never depends on the
target repo excluding its private infra file.

## Security

| Defense                             | Mechanism                                                                                                                                                     |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Exact-equality filtering            | `strip_claim_sentinel` matches the exact root path only. An attacker who plants a same-name file in a subdirectory cannot have it masked from the guard or from the verification evidence. |
| Root-anchored exclude pattern       | The Layer 1 pattern is `/.simard-engineer-claim`; a bare unanchored entry would (per gitignore semantics) also suppress same-basename files in subdirectories. Anchoring keeps exclude semantics identical to the exact-root filter. |
| Path resolution via git only        | The exclude path is resolved with `git rev-parse --git-path`, never string-concatenated from a crafted worktree name. No path traversal via crafted names.    |
| No shell, no env passthrough        | All git calls use the argv-vector form through `git_capture`, which starts from `Command::env_clear()`. No `sh -c`, no inherited `GIT_*`/`LD_PRELOAD`.         |
| Serialized exclude write            | The read-modify-write of the shared `info/exclude` is serialized under `worktree_mutation_lock`, closing the TOCTOU window against the repo's pre-existing entries. |
| Fail-loud-but-non-fatal on write    | An exclude write failure logs the path and error at `warn!` (no environment dump) and continues; Layer 2 remains the backstop.                                |

## Regression tests

The invariants are pinned by hermetic tests that `git init` a fresh repo which,
like an external governed repo, does **not** gitignore the sentinel.

`src/engineer_loop/tests_claim_sentinel.rs`:

| Test                                                          | Asserts                                                                                 |
| ------------------------------------------------------------ | --------------------------------------------------------------------------------------- |
| `inspect_workspace_treats_claim_only_worktree_as_clean`      | The exact #2621 repro (`?? .simard-engineer-claim`) yields `worktree_dirty == false` and empty `changed_files`. |
| `inspect_workspace_still_flags_real_changes_alongside_claim` | Sentinel + a real edit ⇒ still dirty; only the real file is reported (over-filter guard). |
| `strip_claim_sentinel_removes_only_the_root_sentinel`        | Only the root sentinel is dropped; all other paths preserved in order.                  |
| `strip_claim_sentinel_keeps_subdir_same_basename`            | A same-basename file under a subdirectory is kept.                                       |
| `verify_agent_spawn_artifacts_ignores_claim_only_no_op_session` | Degraded path (sentinel on disk, no exclude entry): a claim-only session is `"unverified"`. |
| `verify_agent_spawn_artifacts_still_verifies_real_change_alongside_claim` | A genuine agent-created file still flips the report to `"verified"`.         |

`src/engineer_worktree/tests_extra.rs` covers the Layer 1 append: the exclude
entry is written and root-anchored, is idempotent across re-allocation, and a
nested same-basename file is still surfaced by `git status`.

## Examples

### Confirm the exclude entry on an allocated worktree

```bash
# Resolve the real exclude path (works for linked worktrees too) and show
# just the sentinel line. `cat` alone would also print git's default exclude
# template and any pre-existing user entries, which are preserved untouched.
wt=~/.simard/engineer-worktrees/<goal-id>-<epoch>-<hex>
git -C "$wt" rev-parse --git-path info/exclude | xargs grep -F .simard-engineer-claim
# → /.simard-engineer-claim
```

### Confirm a claim-only worktree is clean

```bash
git -C "$wt" status --short --untracked-files=all
# (no output — the sentinel is excluded; the tree is clean)
```

## Related

- [Per-Engineer Worktree Isolation](./engineer-worktree-isolation.md)
- [Engineer Worktree Sweep Safety Guards](./engineer-worktree-sweep-safety.md)
- [Engineer-Loop argv Sanitization](./engineer-loop-argv-sanitization.md)
- [How OODA spawns engineer agents](../howto/spawn-engineers-from-ooda-daemon.md)
- Source: `src/engineer_worktree/mod.rs` (`ENGINEER_CLAIM_FILE`,
  `exclude_engineer_claim`, `Worktree::allocate`),
  `src/engineer_loop/mod.rs` (`strip_claim_sentinel`, `inspect_workspace`,
  `verify_agent_spawn_artifacts`, pre-mutation guard)
