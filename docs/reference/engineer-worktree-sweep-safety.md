---
title: Worktree Reaping Safety Guards
description: Reference for the defense-in-depth guards that stop Simard's two worktree-deletion paths — the operator GC (simard worktree-gc) and the OODA daemon's engineer sweep — from removing in-use, out-of-scope, or work-carrying worktrees. Fixes the issue #2553 data-loss incident.
last_updated: 2026-07-04
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./engineer-worktree-isolation.md
  - ../howto/run-ooda-daemon.md
  - ../concepts/steerable-ooda-daemon.md
---

# Worktree Reaping Safety Guards

Simard deletes worktrees from **two independent code paths**. Issue #2553 was a
data-loss incident in which worktrees were removed out from under active
operations — including operator source-checkout worktrees under
`~/src/Simard/worktrees/` and an in-use worktree carrying uncommitted work. This
reference documents the guards that make **both** paths refuse to delete a
worktree that is in use, out of scope, or carrying unsaved/unpushed work. Every
guard is **fail-safe: on any uncertainty the worktree is kept**, never deleted.

## The two deletion paths

| Path | Entry point | Source | Reaches `~/src/Simard/worktrees/`? |
| ---- | ----------- | ------ | ---------------------------------- |
| **Operator GC** | `simard worktree-gc [--apply]` | `src/worktree_gc/` (`runner.rs`, `policy.rs`, `liveness.rs`) | **Yes** — `default_roots()` includes `<HOME>/src/Simard/worktrees` (this is the incident's deletion vector) |
| **Daemon engineer sweep** | `sweep_orphaned_worktrees` (boot + every `SIMARD_WORKTREE_SWEEP_INTERVAL_SECS`) | `src/engineer_worktree/sweep.rs` | **No** — structurally scoped to `<state_root>/engineer-worktrees/` |

Both paths already carried *some* protection; issue #2553 hardens the gaps and
makes every removal observable. The two paths share a single liveness primitive
(`worktree_gc::liveness`) rather than duplicating it.

## Background — the issue #2553 incident

```
error: failed to create file `.../output-test-lib-simard`
Unable to proceed. Could not locate working directory:
No such file or directory (os error 2)
```

An operator's warm build-target worktree (`meeting-ux-762`) and an in-use rebase
worktree under `~/src/Simard/worktrees/` were removed mid-`cargo build`. Of 35
git-tracked worktrees, ~33 directories had been deleted.

**Which path caused it.** The daemon engineer sweep is scoped to
`<state_root>/engineer-worktrees/` and **cannot** reach `~/src/Simard/worktrees/`
(canonical `starts_with` containment + symlink refusal + fail-loud
canonicalize). It was not the vector for the operator-worktree loss. The
**operator GC** path is: `worktree_gc::default_roots()` deliberately includes
`<HOME>/src/Simard/worktrees`, and `simard worktree-gc --apply` runs
`git worktree list --porcelain`, filters to those roots (`under_any_root`), and
prunes candidates whose branch is merged, deleted-from-origin, or idle.

**The gap.** `worktree_gc` already declines to prune a worktree that is the CWD
of a live process (`liveness.rs`, fail-closed). But a worktree can be **in use
without being any process's CWD** — a warm `CARGO_TARGET_DIR` target between
build invocations, or a rebase worktree the operator has not `cd`'d into. Its
branch may be merged and its tree "idle" by mtime, so the policy marked it a
candidate and `--apply` deleted it. `CandidateInputs` had **no field for
uncommitted or unpushed work**, so the policy could not defend it.

The fix is defense-in-depth across both paths:

1. **Uncommitted/unpushed-work guard** — new; the genuinely missing protection.
2. **CWD-liveness** — already present in `worktree_gc`; **reused** (not
   re-implemented) by the daemon sweep.
3. **Scope containment** — already present on both paths; made explicit and
   observable.
4. **Conservative, observable reaping** — every removal logs its reason and the
   guards that passed.

## Shared liveness primitive — reuse, do not re-implement

Both paths use the existing trait and probe in
`src/worktree_gc/liveness.rs`. There is **no** parallel liveness abstraction in
`engineer_worktree`.

```rust
/// "Is this worktree path the CWD of any live process on the host?"
/// Fail-closed: if the answer cannot be determined (non-Linux, /proc
/// unreadable, canonicalize failure), returns `true` (assume live → keep).
pub trait LiveProcessProbe {
    fn worktree_has_live_process(&self, dir: &Path) -> bool;
}

/// Production probe: scans `/proc/<pid>/cwd` symlinks; any resolving at or
/// under `dir` → true. Unreadable entries / mid-scan races / EPERM are
/// skipped; unreadable `/proc` or uncanonicalizable `dir` → true (fail-closed).
pub struct ProcfsLiveProcessProbe { /* proc_root */ }

/// Test double (cfg(test)): a fixed path → liveness map; unknown → false.
pub struct FakeLiveProcessProbe { /* live: Mutex<HashMap<PathBuf, bool>> */ }
```

The operator GC path already injects `ProcfsLiveProcessProbe` (see
`operator_cli/worktree_gc.rs`) and `run_gc` threads it through
`gather_inputs`. The daemon sweep gains the same seam (below), and both reuse
`FakeLiveProcessProbe` in tests.

## Front A — operator GC path (`src/worktree_gc/`)

This is the incident's deletion vector. It already has scope
(`under_any_root`), liveness (`LiveProcessProbe`, fail-closed), a real policy
(`PruneReason::{BranchMerged, BranchDeletedFromOrigin, IdleTooLong}`), and a
dry-run default (`--apply` is required for any mutation). The fix adds the
missing uncommitted/unpushed-work guard.

### New: uncommitted/unpushed-work guard

`CandidateInputs` gains one field; the policy gates on it before any prune
reason can make the worktree a candidate.

```rust
pub struct CandidateInputs {
    pub merged_prs: Vec<u32>,
    pub branch_on_origin: Option<bool>,
    pub last_activity: Option<SystemTime>,
    pub has_live_process: bool,

    /// NEW (#2553): the worktree has uncommitted changes
    /// (`git status --porcelain` non-empty), unpushed commits
    /// (`git rev-list --count @{u}..HEAD` > 0), or its work-state could not
    /// be proven safe (no upstream configured, or a git error). When set,
    /// `evaluate_candidate` returns `None` regardless of merged / deleted /
    /// idle signals — pruning would destroy unsaved or unpushed work.
    pub has_uncommitted_or_unpushed_work: bool,
}
```

`evaluate_candidate` checks it immediately after the existing
`has_live_process` short-circuit (both beat every prune reason):

```rust
if inputs.has_live_process { /* existing #1886 skip */ return None; }

if inputs.has_uncommitted_or_unpushed_work {
    tracing::info!(
        target: "simard::worktree_gc",
        worktree = %entry.path.display(),
        branch = entry.branch.as_deref().unwrap_or("<detached>"),
        "skipping prune: uncommitted or unpushed work in worktree (#2553)",
    );
    return None;
}
```

`gather_inputs` (in `runner.rs`) computes the field with the same env-cleared
`git` shellout discipline used elsewhere in the module, running inside the
candidate worktree:

- `git status --porcelain` — any output ⇒ dirty ⇒ has-work.
- `git rev-list --count @{u}..HEAD` — count > 0 ⇒ ahead of upstream ⇒ has-work.
- **No upstream configured** (`@{u}` fails) ⇒ cannot prove pushed ⇒ has-work.
- **Any git error** ⇒ has-work (fail-safe: keep).

This guard is purely additive: it can only turn a would-be candidate into a
keep. Existing GC behavior for clean, merged/deleted/idle worktrees is
unchanged.

`gather_inputs` evaluates the two cheap, local vetoes — live-CWD and this
uncommitted/unpushed-work check — **before** the upstream lookups. Because
either veto forces `evaluate_candidate` to return `None` regardless of the
upstream answer, a live or work-carrying worktree skips the `gh pr list` and
`git ls-remote` round-trips entirely. This is a performance-only
short-circuit: the prune decision is identical, but the network calls (which
dominate GC cost) are avoided for worktrees that are already disqualified.

### Reasons and logging (already present)

Requirement #4 (log every removal with its reason) is already satisfied on this
path: `render_reason` formats each `PruneReason` and the operator CLI prints
`candidate: <path> branch=<b> reason=<merged|deleted|idle>` per candidate, plus
`pruned:` / `FAILED:` lines under `--apply`. The new guard adds an `INFO` skip
line (above) so kept worktrees are observable too.

### Configuration (unchanged)

| Variable / flag | Default | Effect |
| --------------- | ------- | ------ |
| `--idle-days=N` | `DEFAULT_IDLE_DAYS` (`7`) | `IdleTooLong` threshold, in **days**. |
| `SIMARD_WORKTREE_GC_ROOTS` | `<HOME>/.simard/engineer-worktrees:<HOME>/src/Simard/worktrees` | Colon-separated scan roots (overrides `default_roots()`). |
| `--apply` | off (dry-run) | Required for any filesystem mutation. |

## Front B — daemon engineer sweep (`src/engineer_worktree/sweep.rs`)

The periodic daemon sweep is already scoped to `<state_root>/engineer-worktrees/`
and already skips worktrees whose `.simard-engineer-claim` names a live PID
(`claim_is_live`, issue #1213/#1238). It is **structurally incapable** of
touching `~/src/Simard/worktrees/`. Issue #2553 adds the same live-CWD and
work-state protections here as **defense-in-depth**, and makes each removal
observable.

### Guard pipeline

Guards run **cheapest-first, most-destructive-last**. The first guard that votes
"keep" short-circuits; the candidate is recorded in the corresponding skip
bucket of the [`SweepReport`](#sweepreport) and the sweep moves on.

```
candidate dir under <state_root>/engineer-worktrees/
        │
        ▼
┌───────────────────────────────────────────────────────────────┐
│ 1. SCOPE  (existing)                                           │
│    symlink_metadata → refuse symlinks (WARN)                   │
│    canonicalize → assert starts_with(<engineer-worktrees>/)    │
│    still registered in `git worktree list` → keep              │
│    canonicalize failure → FAIL LOUD (abort sweep)              │
└───────────────────────────────────────────────────────────────┘
        │ unregistered, in-scope
        ▼
┌───────────────────────────────────────────────────────────────┐
│ 2. LIVE_CLAIM  (existing, issue #1213/#1238)                   │
│    read .simard-engineer-claim → claim_is_live(pid,starttime)  │
│    live → keep  → SweepReport.skipped_live_dirs                │
└───────────────────────────────────────────────────────────────┘
        │ no live claim
        ▼
┌───────────────────────────────────────────────────────────────┐
│ 3. LIVE_CWD  (new #2553; reuses worktree_gc::liveness)         │
│    LiveProcessProbe.worktree_has_live_process(path)            │
│    any live proc CWD at/under path → keep                      │
│    probe cannot answer → true → keep (fail-closed)            │
│                → SweepReport.skipped_live_cwd_dirs             │
└───────────────────────────────────────────────────────────────┘
        │ no live CWD
        ▼
┌───────────────────────────────────────────────────────────────┐
│ 4. WORK_STATE  (new #2553)                                     │
│    NO .git at all → reapable junk (fall through; no work)      │
│    has .git:                                                   │
│      git status --porcelain → dirty → keep                     │
│      git rev-list --count @{u}..HEAD → ahead → keep            │
│      no upstream configured → keep (cannot prove pushed)      │
│      any git error → keep (fail-safe)                          │
│                → SweepReport.skipped_dirty_dirs                │
└───────────────────────────────────────────────────────────────┘
        │ reapable (junk, or clean+pushed git worktree)
        ▼
┌───────────────────────────────────────────────────────────────┐
│ 5. REAP  (existing basis, now guarded + logged)               │
│    remove_dir_all + record RemovalReason + INFO log           │
│                → SweepReport.removed_orphan_dirs               │
│                → SweepReport.removal_reasons                   │
└───────────────────────────────────────────────────────────────┘
```

> **Idle basis.** The engineer sweep's "orphaned" signal is a **dead or absent
> engineer claim on an unregistered directory** — not a wall-clock timer. A dead
> claim means the allocating engineer process has exited. This is a stronger and
> more precise signal than an mtime idle threshold, so the sweep does **not**
> introduce a separate seconds/days idle knob (see
> [Configuration](#configuration)). The GC path's day-based `--idle-days` is a
> different mechanism for a different (operator-invoked) tool.

### Test seam

```rust
/// Boot-time and periodic sweep of `<state_root>/engineer-worktrees/`.
/// Uses the production `worktree_gc::ProcfsLiveProcessProbe`.
pub fn sweep_orphaned_worktrees(
    parent_repo: &Path,
    state_root: &Path,
) -> Result<SweepReport, SimardError>;

/// Testable core: identical, but with an injected `LiveProcessProbe` so tests
/// can simulate "a live process is using this worktree" deterministically
/// without spawning anything. Reuses `worktree_gc::liveness::FakeLiveProcessProbe`.
pub fn sweep_orphaned_worktrees_inner(
    parent_repo: &Path,
    state_root: &Path,
    probe: &dyn crate::worktree_gc::liveness::LiveProcessProbe,
) -> Result<SweepReport, SimardError>;
```

The public signature is unchanged, so the two daemon call sites in
`operator_commands_ooda/daemon/mod.rs` (boot sweep and periodic sweep) compile
and behave identically.

### `SweepReport`

`SweepReport` gains three fields. All fields derive `Default`, so existing
callers that read only `removed_orphan_dirs` / `skipped_live_dirs` compile
unchanged.

```rust
#[derive(Debug, Default)]
pub struct SweepReport {
    /// Directories physically removed. Paired 1:1 with `removal_reasons`.
    pub removed_orphan_dirs: Vec<PathBuf>,

    /// Skipped: `.simard-engineer-claim` named a live PID (LIVE_CLAIM guard).
    pub skipped_live_dirs: Vec<PathBuf>,

    /// Skipped: a live process holds the dir as its CWD, or the liveness
    /// probe could not answer and fail-closed kept it (LIVE_CWD guard). (#2553)
    pub skipped_live_cwd_dirs: Vec<PathBuf>,

    /// Skipped: a git worktree with uncommitted changes, unpushed commits,
    /// no upstream, or a git-state check that could not prove it safe
    /// (WORK_STATE guard). (#2553)
    pub skipped_dirty_dirs: Vec<PathBuf>,

    /// One entry per removal, in `removed_orphan_dirs` order. (#2553)
    pub removal_reasons: Vec<(PathBuf, RemovalReason)>,
}
```

### `RemovalReason`

The engineer sweep reaps unregistered directories that are not live and carry no
recoverable work. `RemovalReason` records the observable basis so the log line
and tests can assert on it. This is **distinct** from the operator GC path's
`worktree_gc::policy::PruneReason` (merged-PR / branch-deleted / idle), which is
a real, separate policy engine used by `simard worktree-gc` — the engineer sweep
deliberately uses the narrower orphan-plus-dead-claim basis.

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemovalReason {
    /// Unregistered with the parent repo, no live engineer-claim, no live
    /// process CWD, and no uncommitted/unpushed work.
    OrphanedNoLiveNoWork {
        /// `true`  → a `.simard-engineer-claim` sentinel was present but named
        ///           a dead/recycled PID (a crashed engineer's leftover).
        /// `false` → no sentinel at all (plain leftover directory / junk with
        ///           no `.git` and nothing to lose).
        had_dead_claim: bool,
    },
}
```

### Logging

Every removal emits one structured `INFO` line at target
`simard::engineer_worktree`, confirming the guards that passed:

```
INFO simard::engineer_worktree: reaped orphaned engineer worktree
  worktree=/home/az/.simard/engineer-worktrees/meeting-ux-762-1751-9f3a1c
  reason=OrphanedNoLiveNoWork had_dead_claim=true
  scope_ok=true live_claim=false live_cwd=false has_work=false
```

`skipped_live_cwd_dirs` and `skipped_dirty_dirs` skips log at `DEBUG`, mirroring
the existing LIVE_CLAIM skip log. The daemon still emits its one-line summary
via `daemon_log` after each sweep (`swept N orphan engineer worktree(s)`).

### Test impact — existing sweep tests stay green

The design is chosen so the current immediate-reap tests in
`src/engineer_worktree/tests_more.rs` and `tests_extra.rs`
(`sweep_removes_orphan_dirs_and_preserves_live_worktrees`,
`sweep_removes_dir_with_recycled_pid_claim`,
`sweep_removes_unregistered_dir_with_dead_engineer_claim`) **continue to pass
without modification**:

- Those tests create **plain directories with no `.git`** (and mtime = now).
- WORK_STATE treats a dir with **no `.git` at all** as reapable junk (there is
  no working tree, so no uncommitted or unpushed work can exist) — it is *not*
  kept.
- The sweep introduces **no wall-clock idle gate**, so a freshly-created junk
  dir is still reapable immediately.
- The production `ProcfsLiveProcessProbe` reports no live CWD for a tempdir no
  process has entered, so LIVE_CWD does not keep them either.

Only the WORK_STATE keep behavior is new, and it fires **only** for directories
that are real git worktrees carrying uncommitted/unpushed work — which none of
the existing immediate-reap fixtures are.

## Fail-safe semantics

| Signal | Interpretation | Action |
| ------ | -------------- | ------ |
| CWD-liveness probe cannot answer (`/proc` unreadable, canonicalize fails, non-Linux) | assume **live** | keep |
| Directory has **no `.git`** at all | no working tree ⇒ **no work to lose** | reapable (subject to claim + liveness) |
| `.git` present but `git status` / `rev-list` errors | assume **has work** | keep |
| `.git` present, no upstream configured | cannot prove pushed | keep |
| `canonicalize` of a registered path fails | ambiguous scope | **abort sweep** (fail loud) |
| Entry under root is a symlink | suspicious | keep + WARN |

The two liveness checks deliberately have **opposite** error semantics, matching
what each protects:

- **LIVE_CLAIM** (`claim_is_live`): if the process exists but its
  `/proc/<pid>/stat` is unreadable, the claim is treated as **not live**
  (re-allocating a worktree is cheaper than a permanently-stale claim). Unchanged.
- **LIVE_CWD** (`LiveProcessProbe`): if the probe cannot read `/proc`, it treats
  the worktree as **live** (keep). A false "keep" only wastes disk; a false
  "remove" destroys a running operation — exactly the issue #2553 failure mode.

## Scope guarantee

- **Daemon engineer sweep** enumerates only
  `read_dir(<state_root>/engineer-worktrees/)`. Each candidate is
  `canonicalize`d and asserted to satisfy
  `starts_with(canonical(<state_root>/engineer-worktrees/))`; anything resolving
  outside is skipped with a `WARN`. Symlinks are never followed. Operator
  source-checkout worktrees under `~/src/Simard/worktrees/`, `$HOME`, and any
  path outside the engineer-worktrees root are **structurally unreachable** by
  this path.
- **Operator GC** intentionally scans `~/src/Simard/worktrees/` (it is the
  operator's tool for reclaiming space there). It is **not** removed from
  `default_roots()`; instead it is gated by dry-run-by-default (`--apply`
  required) plus the full guard set (scope prefix check, liveness, and the new
  uncommitted/unpushed-work guard) so `--apply` can no longer delete an in-use
  or work-carrying operator worktree.

## Configuration

| Variable / flag | Path | Default | Effect |
| --------------- | ---- | ------- | ------ |
| `SIMARD_STATE_ROOT` | daemon sweep | `~/.simard/` | Root of the `engineer-worktrees/` subtree the sweep is scoped to. The sweep never operates outside `<state_root>/engineer-worktrees/`. |
| `SIMARD_WORKTREE_SWEEP_INTERVAL_SECS` | daemon sweep | `1800` | Seconds between periodic sweeps (a boot sweep always runs first). |
| `--idle-days=N` | operator GC | `7` (`DEFAULT_IDLE_DAYS`) | `IdleTooLong` threshold, in days. |
| `SIMARD_WORKTREE_GC_ROOTS` | operator GC | `<HOME>/.simard/engineer-worktrees:<HOME>/src/Simard/worktrees` | Colon-separated scan roots. |

**No new idle knob is introduced.** The daemon sweep's reap basis is the dead /
absent engineer claim (not a timer), so it needs no seconds threshold; the
operator GC keeps its existing day-based `--idle-days`. This avoids two
divergent idle units across the two paths.

## Examples

### Injecting a fake liveness probe in an engineer-sweep test

```rust
use crate::worktree_gc::liveness::FakeLiveProcessProbe;

#[test]
#[serial_test::serial]
fn sweep_skips_worktree_that_is_a_live_process_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    // ... git init parent repo; create an unregistered orphan dir with a dead
    //     claim under <state_root>/engineer-worktrees/live-one ...
    let probe = FakeLiveProcessProbe::default();
    probe.mark_live(&live_one_path); // pretend a process has its CWD here

    let report = sweep_orphaned_worktrees_inner(&parent, &state_root, &probe).unwrap();

    assert!(report.removed_orphan_dirs.is_empty());
    assert_eq!(report.skipped_live_cwd_dirs.len(), 1);
}
```

### Asserting the reap reason

```rust
let probe = FakeLiveProcessProbe::default(); // nothing live
let report = sweep_orphaned_worktrees_inner(&parent, &state_root, &probe).unwrap();
assert_eq!(report.removed_orphan_dirs.len(), 1);
let (path, reason) = &report.removal_reasons[0];
assert_eq!(path, &report.removed_orphan_dirs[0]);
assert!(matches!(reason, RemovalReason::OrphanedNoLiveNoWork { had_dead_claim: true }));
```

### Operator GC: uncommitted-work is now kept

```rust
// worktree_gc/policy.rs — a merged branch with dirty tree is NOT a candidate.
let inputs = CandidateInputs {
    merged_prs: vec![42],
    branch_on_origin: Some(true),
    last_activity: Some(SystemTime::now()),
    has_live_process: false,
    has_uncommitted_or_unpushed_work: true, // dirty / ahead / no upstream
};
assert!(evaluate_candidate(&entry, &inputs, SystemTime::now(), 7).is_none());
```

### Operator: inspect before/after

```bash
# Dry-run the operator GC (default) to see what it *would* prune — never mutates:
simard worktree-gc --idle-days=7

# The only dir the daemon sweep can delete from:
ls -la "${SIMARD_STATE_ROOT:-$HOME/.simard}/engineer-worktrees/"

# Prove operator source-checkout worktrees are OUT of the daemon sweep's scope:
git -C /home/azureuser/src/Simard worktree list --porcelain \
  | grep -E '^worktree .*/src/Simard/worktrees/'

# Watch daemon reap decisions live (INFO reasons + DEBUG skips):
RUST_LOG=simard::engineer_worktree=debug simard ooda run --cycles=0 2>&1 \
  | grep -E 'reaped orphaned|skipping .* worktree'
```

## Testing

Coverage is offline, serial, sleep-free, and network-free. Tests build a real
repo with `git init`, plant directories under a `tempfile::tempdir` state root,
and inject fakes (`FakeLiveProcessProbe`; and, for the GC path, `GhClient` /
`CandidateInputs`).

| Test | Path | Asserts |
| ---- | ---- | ------- |
| live-CWD skip | sweep | probe reports in-use → kept (`skipped_live_cwd_dirs`) |
| liveness-error ⇒ keep | sweep | probe that cannot answer keeps the worktree (fail-closed) |
| out-of-scope skip | sweep | a dir canonicalizing outside the engineer-worktrees root is never removed |
| symlink skip | sweep | a symlink under the root is skipped + WARN, target untouched |
| uncommitted-work skip | sweep | a real git worktree with dirty `git status --porcelain` is kept (`skipped_dirty_dirs`) |
| unpushed / no-upstream skip | sweep | ahead of `@{u}`, or no upstream, → kept |
| genuine-orphan reap | sweep | unregistered, dead/absent claim, clean, no live CWD → removed |
| removal reason recorded | sweep | `removal_reasons` pairs each removed path with `RemovalReason::OrphanedNoLiveNoWork` |
| junk dir still reaped | sweep | plain dir with no `.git` (existing fixtures) still removed — no regression |
| GC uncommitted-work skip | GC | `has_uncommitted_or_unpushed_work` blocks a merged/deleted/idle candidate (`evaluate_candidate → None`) |
| GC guard is additive | GC | clean merged/deleted/idle candidates still prune as before |

No new crates are required — `tracing`, `libc`, `tempfile`, and `serial_test`
are already dependencies, and `worktree_gc::liveness` is reused rather than
duplicated.

## Related

- [Agentic disk reclamation](../concepts/agentic-disk-reclamation.md) — the
  disk-reclaim guard (`src/disk_reclaim/guard.rs`) **composes** the liveness and
  uncommitted/unpushed primitives documented here rather than duplicating them.
- [Per-engineer worktree isolation](./engineer-worktree-isolation.md) — how each
  worktree is allocated and cleaned up.
- [Run the OODA daemon](../howto/run-ooda-daemon.md)
- [Steerable OODA daemon](../concepts/steerable-ooda-daemon.md)
- Source (operator GC): `src/worktree_gc/policy.rs`,
  `src/worktree_gc/runner.rs`, `src/worktree_gc/liveness.rs`,
  `src/operator_cli/worktree_gc.rs`
- Source (daemon sweep): `src/engineer_worktree/sweep.rs`,
  `src/engineer_worktree/claim.rs`, `src/engineer_worktree/mod.rs`,
  `src/operator_commands_ooda/daemon/mod.rs`
