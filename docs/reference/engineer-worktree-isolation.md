---
title: Per-Engineer Worktree Isolation
description: Reference for the EngineerWorktree allocator that gives each subordinate engineer its own git worktree, eliminating parallel-spawn races that the verification gate would otherwise reject.
last_updated: 2026-04-24
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ./engineer-loop-argv-sanitization.md
  - ../howto/run-ooda-daemon.md
---

# Per-Engineer Worktree Isolation

Every subordinate engineer agent that the OODA daemon spawns runs in its own
dedicated `git worktree` rooted under the supervisor state directory. The
worktree is allocated transactionally at spawn time, threaded into the
subordinate as its working directory, and removed deterministically after the
subordinate exits.

This isolates each engineer's mutations on disk so that the engineer-loop
verification gate (`src/engineer_loop/verification.rs`) only observes that
engineer's own diff, never side-effects from a sibling engineer running in the
same checkout.

## Background — why isolation is required

The engineer loop's verification gate (`verify_workspace_unchanged`,
`verification.rs:63-85`) snapshots the workspace before a non-mutating local
action and re-snapshots it afterward. If anything on disk changed between
the snapshots, the gate rejects the action with:

```
worktree state changed during a non-mutating local engineer action
```

When two or more engineer subprocesses share `/home/azureuser/src/Simard`,
sibling writes routinely race the gate's snapshot window, so the gate
rejects perfectly correct local actions. The historical effect was that the
daemon produced correct code on disk every cycle but shipped **0 PRs**
across many OODA cycles.

Per-engineer worktrees move the gate's observation scope from the shared
checkout to a path that only one engineer process can write to.

## Filesystem layout

Worktrees live under the supervisor state root:

```
$SIMARD_STATE_ROOT/engineer-worktrees/<goal-id>-<epoch_secs>-<6hex>/
```

| Component             | Source                                                           |
| --------------------- | ---------------------------------------------------------------- |
| `$SIMARD_STATE_ROOT`  | env var `SIMARD_STATE_ROOT`, defaulting to `~/.simard/`          |
| `<goal-id>`           | URL-safe slug of the goal id assigned by the supervisor          |
| `<epoch_secs>`        | seconds since UNIX epoch at allocation                           |
| `<6hex>`              | 6 random hex chars (collision guard for same-second allocations) |

The matching git branch is:

```
engineer/<goal-id>-<epoch_secs>-<6hex>
```

Every worktree gets its own branch — there is a strict 1↔1 mapping between
worktree directories and engineer branches. No two engineer processes ever
share a branch, ever.

### Base ref

Each worktree branches from the parent repo's current `main` (resolved via
`git rev-parse main`). If `main` is unresolvable in the parent repo, allocation
**fails loud** with `SimardError::ActionExecutionFailed`. There is no fallback
to `HEAD`; an unresolvable `main` is an environmental fault.

## API — `src/engineer_worktree.rs`

### `EngineerWorktree`

```rust
pub struct EngineerWorktree { /* fields private */ }

impl EngineerWorktree {
    /// Allocate a worktree for the given goal id, branched off the parent
    /// repo's `main`. Creates the directory under
    /// `<state_root>/engineer-worktrees/`, registers the worktree with git,
    /// and returns an owned handle. Failure is hard.
    pub fn allocate(
        parent_repo: &Path,
        state_root: &Path,
        goal_id: &str,
    ) -> Result<EngineerWorktree, SimardError>;

    /// Absolute path to the worktree's working directory.
    pub fn path(&self) -> &Path;

    /// Branch name registered for this worktree.
    pub fn branch(&self) -> &str;

    /// Idempotently remove the worktree:
    ///   1. `git worktree remove --force <path>`
    ///   2. best-effort `git branch -D engineer/<...>` (failures logged, not propagated)
    ///   3. canonical-prefix-checked `fs::remove_dir_all` of the worktree dir
    ///   4. mark the handle finalized via internal `AtomicBool`
    /// Safe to call multiple times; only the first call performs work.
    /// Returns `Err` only on real failures: the canonical-prefix guard
    /// rejects the path, or the on-disk dir removal fails.
    pub fn cleanup(&self) -> Result<(), SimardError>;
}

impl Drop for EngineerWorktree {
    /// RAII safety net: if `cleanup` was never called explicitly, Drop runs
    /// it best-effort. Drop never panics; failures are logged via `tracing`.
    fn drop(&mut self);
}
```

Cleanup state is tracked via an internal `AtomicBool` so the explicit
reaper-driven `cleanup()` and the `Drop` safety net cannot collide.

### `sweep_orphaned_worktrees`

```rust
/// Report from a one-shot sweep, used by the daemon at boot to log
/// post-crash hygiene metrics.
pub struct SweepReport {
    /// Number of stale `git worktree` registrations that were pruned.
    pub pruned_registrations: usize,
    /// Filesystem directories under `engineer-worktrees/` that were removed
    /// because they no longer appeared in `git worktree list`.
    pub removed_orphan_dirs: Vec<PathBuf>,
}

/// One-shot startup sweep. Runs `git worktree prune` in the parent repo,
/// then removes any directory under `<state_root>/engineer-worktrees/`
/// that no longer appears in `git worktree list --porcelain`. Also
/// best-effort deletes leftover `engineer/*` branches whose worktrees
/// are gone (failures logged, not propagated).
pub fn sweep_orphaned_worktrees(
    parent_repo: &Path,
    state_root: &Path,
) -> Result<SweepReport, SimardError>;
```

Called once early in OODA loop initialization; the daemon logs the
returned `SweepReport` so operators can see how much was reaped after
a crash. Bounded one-shot cost; prevents disk-pressure accumulation
from prior crashes.

## Lifecycle

```
spawn flow                                  cleanup flow
──────────                                  ────────────
dispatch_spawn_engineer                     OODA reaper (subordinate exited)
  │                                           │
  ├─ EngineerWorktree::allocate()             ├─ validate_subordinate_artifacts
  │   ├─ git worktree add                     │
  │   │     -b engineer/<...> <path> <main>   ├─ if let Some(wt) = handle.owned_worktree.take():
  │   │     (where <main> = `git rev-parse main`)│      wt.cleanup()
  │   └─ returns EngineerWorktree handle      │        .inspect_err(|e| tracing::error!(...))
  │                                           │        // log-and-continue; never propagate
  ├─ SubordinateConfig.worktree_path = path   │        ├─ git worktree remove --force
  │                                           │        ├─ best-effort git branch -D engineer/<...>
  ├─ spawn_subordinate(...)                   │        └─ marks handle finalized
  │     │                                     │
  │     ├─ on success:                        └─ clears assigned_to on goal
  │     │   handle.owned_worktree =
  │     │     Some(EngineerWorktree)          Drop guard (panic / early-return)
  │     │                                       │
  │     └─ on failure:                          └─ best-effort cleanup() if not
  │         worktree.cleanup()?  // explicit       already finalized; logs errors
  │         return Err(...)
  │
  └─ subordinate runs `git`, `cargo`, etc. with cwd = worktree path
```

### Spawn-failure invariant

If `spawn_subordinate` returns `Err`, `dispatch_spawn_engineer` calls
`worktree.cleanup()?` **before** returning the error. There is no path that
leaves an allocated worktree without an owner.

### Reaper invariant

The OODA reaper site that runs `validate_subordinate_artifacts` and clears
`assigned_to` is the single explicit cleanup site. Cleanup runs *after*
artifact validation so the validator can read commit/PR artifacts from the
worktree.

A cleanup failure in the reaper is **logged via `tracing::error!` and the
reaper continues** — it does not propagate, does not block clearing
`assigned_to`, and does not fail the OODA cycle. Any directory left behind
will be reaped by the next boot's `sweep_orphaned_worktrees` call. This
matches the `inspect_err` style used elsewhere in the daemon for non-fatal
post-condition failures.

### RAII safety net

`Drop` is the secondary cleanup path. It exists for crash and early-return
paths the explicit reaper cannot reach. Drop:

- never panics
- logs success/failure via `tracing`
- is idempotent with the explicit `cleanup()` (the `AtomicBool` short-circuits)

## Subordinate environment

When `dispatch_spawn_engineer` builds the subordinate's env block it sets:

| Var                | Value                              | Condition                |
| ------------------ | ---------------------------------- | ------------------------ |
| `CARGO_TARGET_DIR` | `/tmp/simard-engineer-target`      | only if not already set  |

Without this, every per-engineer worktree would trigger a cold `lbug` build
(~40 min). Operators may override by exporting `CARGO_TARGET_DIR` in the
parent daemon environment.

## Configuration

| Env var                | Effect                                                   | Default        |
| ---------------------- | -------------------------------------------------------- | -------------- |
| `SIMARD_STATE_ROOT`    | Root for `engineer-worktrees/` subtree                   | `~/.simard/`   |
| `CARGO_TARGET_DIR`     | Inherited by subordinates if set; injected if unset      | injected value |

There are no other tunables. Worktree allocation is unconditional for
daemon-spawned engineers.

## Manual operator CLI is unchanged

The manual debug entry point `simard spawn_engineer <path>` (handled by
`dispatch_spawn_command` in `src/operator_cli.rs`) **does not** allocate a
worktree. It continues to use the caller-supplied path verbatim. Worktree
isolation applies only to the autonomous daemon path
(`dispatch_spawn_engineer` in `src/ooda_actions/advance_goal.rs`).

This preserves the original interactive-debug UX where an operator may want
to attach an engineer to an arbitrary checkout for forensic work.

## Verification gate behaviour

`verify_workspace_unchanged` (`src/engineer_loop/verification.rs:63-85`) is
**unchanged** by this feature. The gate continues to snapshot
`engineer_action.cwd` before and after a non-mutating local action.

The fix is purely a change of *observation scope*: with per-engineer
worktrees, `cwd` points into a directory only one engineer process is
allowed to write to. Sibling engineers in other worktrees cannot perturb
this engineer's snapshot.

## Error handling

All allocation and cleanup failures surface as
`SimardError::ActionExecutionFailed` with a message that names the failing
git or filesystem operation and the worktree path.

Per repo convention there are **no fallbacks**:

- allocation failure → hard error, no silent retry, no "use shared checkout"
- cleanup failure in the **spawn-failure rollback path** → hard error,
  propagated up (the engineer never started, so we must surface why)
- cleanup failure in the **reaper path** → logged via `tracing::error!`,
  reaper continues; the next boot's `sweep_orphaned_worktrees` reaps the
  leftover. This is *not* a fallback — the failure is surfaced loudly in
  logs and recovered deterministically at next startup.
- cleanup failure in `Drop` → logged at `error!` level, never panics
- best-effort `git branch -D` failure inside cleanup → logged, not propagated
  (a stale branch is harmless and reaped by the next sweep)
- unresolvable `main` base ref → hard error
- `main` rev-parse output that is not exactly 40 lowercase-hex → hard error

## Security

The allocator and sweep are hardened against a local attacker who can
influence `goal_id`, plant filesystem entries under the worktrees root, or
set environment variables in the daemon process. All checks fail loud — no
fallbacks, no silent skips beyond those explicitly noted.

| Defense                             | Mechanism                                                                                                                                                                          |
| ----------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `goal_id` validation                | Boundary check `^[A-Za-z0-9._-]{1,64}$` rejecting empty, leading `-` (argv injection / git ref-flag), and leading `.` (path traversal / hidden file). Fail-loud `ActionExecutionFailed`. |
| `main_sha` validation               | `git rev-parse main` output must match `^[0-9a-f]{40}$` before being passed to `git worktree add`. Empty or non-hex output → hard error.                                           |
| Canonical-prefix guard on deletion  | Every `fs::remove_dir_all` against an `EngineerWorktree.path` (failure-recovery path, `cleanup_inner`) canonicalizes the target and asserts `starts_with(canonical_worktrees_root)`. Refuses to operate otherwise — fail-loud, no silent fallback to the non-canonical path. |
| Sweep does not follow symlinks      | `sweep_orphaned_worktrees` uses `symlink_metadata` and skips any entry whose `file_type().is_symlink()`. A planted symlink under `engineer-worktrees/` is logged at WARN and left in place for an operator to investigate.                              |
| Sweep canonicalization is fail-loud | Both registered worktree paths and on-disk entries are canonicalized. A canonicalize failure aborts the sweep with `ActionExecutionFailed`; we never compare a non-canonical path against canonical peers (that would risk false-orphan deletion of a live worktree). |
| Restrictive perms on worktrees root | On Unix, the `engineer-worktrees/` directory is created with mode `0o700`. Worktrees may transiently hold credentials; do not expose them to other local users.                                                            |
| `git_capture` env isolation         | Every `git` subprocess starts from `Command::env_clear()` and re-injects only `PATH` and `HOME`. Inherited `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_CONFIG_GLOBAL`, `LD_PRELOAD` etc. cannot redirect or hijack git invocations.       |
| No shell                            | All git invocations use the argv-vector form (`Command::args(&[...])`). No string concatenation, no `sh -c`.                                                                       |
| Process-wide mutation lock          | A static `Mutex` serializes `git worktree add`/`remove`/`prune` against the parent repo's `.git/worktrees/` registry. Eliminates the racy registry corruption observed under parallel allocation. |

## Examples

### Reading the worktree path from a `SubordinateHandle`

```rust
if let Some(worktree) = subordinate.owned_worktree.as_ref() {
    info!(
        worktree_path = %worktree.path().display(),
        worktree_branch = %worktree.branch(),
        "subordinate is running in dedicated worktree",
    );
}
```

### Manual cleanup in the reaper

```rust
validate_subordinate_artifacts(&handle).await?;

if let Some(worktree) = handle.owned_worktree.take() {
    // Log-and-continue: a leftover worktree is reaped at next boot
    // by sweep_orphaned_worktrees. We must still clear assigned_to.
    let _ = worktree.cleanup().inspect_err(|err| {
        tracing::error!(
            ?err,
            worktree_path = %worktree.path().display(),
            "engineer worktree cleanup failed; will be reaped at next startup",
        );
    });
}

goal.assigned_to = None;
```

### Inspecting allocated worktrees from the shell

```bash
# All currently registered engineer worktrees
git -C /home/azureuser/src/Simard worktree list --porcelain \
  | grep -E '^worktree .*/engineer-worktrees/'

# Disk usage of the engineer-worktrees subtree
du -sh ~/.simard/engineer-worktrees/

# Manually prune orphans (also runs automatically at daemon startup)
git -C /home/azureuser/src/Simard worktree prune
```

## Operational notes

- **Disk pressure.** Each worktree is a full checkout. The startup orphan
  sweep plus deterministic reaper cleanup keep steady-state disk usage
  bounded by `(active engineers) × (repo size)`.
- **Build cache.** The shared `CARGO_TARGET_DIR` makes `cargo` builds across
  worktrees share one incremental cache. Without it, every engineer would
  trigger a cold `lbug` rebuild.
- **Nested engineers.** Engineers spawned by other engineers (depth ≥ 1)
  also allocate their own worktrees under the same managed root. The race
  is identical at any depth; the handling is uniform.
- **Pre-existing unstaged files** in the parent checkout (e.g., the 3 stale
  files in `/home/azureuser/src/Simard` at the time of initial rollout) are
  not migrated. They live in the shared checkout and are out of scope.

## Related

- [How OODA spawns engineer agents](../howto/spawn-engineers-from-ooda-daemon.md)
- [Engineer loop argv sanitization](./engineer-loop-argv-sanitization.md)
- [Run the OODA daemon](../howto/run-ooda-daemon.md)
- Source: `src/engineer_worktree.rs`,
  `src/ooda_actions/advance_goal.rs::dispatch_spawn_engineer`,
  `src/engineer_loop/verification.rs`
