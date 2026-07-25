---
title: Engineer Worktree Presence Guard
description: Reference for the cycle-start worktree presence check (EngineerWorktree::is_present) that closes the TOCTOU between engineer discovery/reuse and the worktree reaper, fixing the missing-workspace goal-session fault (issue #4578).
last_updated: 2026-07-24
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./engineer-worktree-isolation.md
  - ./engineer-worktree-sweep-safety.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ../howto/run-ooda-daemon.md
  - ../howto/diagnose-a-deferred-engineer-spawn.md
---

# Engineer Worktree Presence Guard

Before the OODA daemon reuses an existing engineer worktree — or reports an
already-assigned / already-running engineer as done — it re-verifies that the
worktree directory the engineer was allocated (`<goal-id>-<rand>`) still exists
on disk. If the worktree has been reaped or removed between the check and the
use, the guard **re-provisions a fresh worktree** instead of returning a stale
success or crashing with a missing-workspace fault.

This closes a time-of-check/time-of-use (TOCTOU) window between worktree
discovery/reuse and the concurrent worktree GC/reaper (see
[engineer-worktree-sweep-safety](./engineer-worktree-sweep-safety.md)) that
previously aborted goal-session cycles with a bare "missing workspace"
error.

## Background — the fault this fixes

`find_live_engineer_for_goal()`
(`src/ooda_actions/advance_goal/spawn.rs`) discovers a running engineer by
scanning `engineer-worktrees/<goal-id>-*` and returning the **first** worktree
whose liveness sentinel points at a still-alive PID. Separately, `advance_goal`
inserts each freshly allocated worktree into `state.engineer_worktrees`
(`typed_goal_session.rs:432`); later cycles reuse those entries via
`subordinate.rs:33` (the heartbeat/artifact path) and `cycle.rs:1613` (the
`worker_present` predicate). A third path short-circuits even earlier: when a
goal is already `assigned_to` an engineer, `advance_goal` returns immediately
(`typed_goal_session.rs:340`) without touching the filesystem at all.

Both discovery and stored-map reuse historically returned or trusted a worktree
**path** without re-checking that the directory still existed at the moment of
use, and the `already_assigned` path never checked the filesystem. The reaper
subsystem runs concurrently and can remove a worktree between the check and the
use, so a cycle could:

1. discover / look up a worktree path,
2. have the reaper delete it,
3. proceed to use the now-missing path,
4. abort the whole goal-session cycle with a missing-workspace crash.

Symptom (issue #4578): goal-session cycles fail end-to-end with a
missing-workspace / no-worktree fault, blocking engineer execution.

The presence guard moves the existence check to the **last step before use**,
and turns a hard crash into a logged, safe re-provision.

## API

### `EngineerWorktree::is_present`

```rust
impl EngineerWorktree {
    /// Returns `true` iff this worktree still holds a readable engineer-claim
    /// sentinel (`.simard-engineer-claim`) on disk.
    ///
    /// Implemented as a single fail-closed claim read
    /// ([`read_engineer_claim_full`](../../src/engineer_worktree/claim.rs)): if
    /// the worktree directory was reaped, the read errors and the method
    /// returns `false`. No separate `lstat` is needed — the claim read is the
    /// one syscall that already distinguishes "present" from "gone", so this is
    /// the smallest sufficient seam.
    ///
    /// Scope: this checks **worktree presence**, not engineer liveness. It does
    /// not re-probe the claim PID with `kill(pid, 0)` — discovery
    /// (`find_live_engineer_for_goal`) owns the liveness check. Reuse sites that
    /// need both run discovery first, then presence-check at the point of use.
    ///
    /// Goal identity is bound by `self` (this `EngineerWorktree` was allocated
    /// for one goal, and its directory name is `<goal-id>-<rand>`) and upstream
    /// by `find_live_engineer_for_goal`'s `<goal-id>-*` glob — **not** by the
    /// sentinel contents. The claim sentinel records only `<pid>\n<starttime>`
    /// (see [`format_engineer_claim`](../../src/engineer_worktree/claim.rs)); it
    /// proves an engineer *claimed* the dir, not which goal owns it.
    ///
    /// This is the single TOCTOU seam for worktree reuse: call it immediately
    /// before using `path()`, with no intervening `.await` / yield that would
    /// widen the check-to-use window.
    pub fn is_present(&self) -> bool;
}
```

| Property | Value |
| --- | --- |
| Cost | one claim-sentinel read (`read_engineer_claim_full`); the read fails when the dir is gone |
| Blocking | non-blocking; no git subprocess, no lock acquired |
| Idempotent | yes — pure read, safe to call repeatedly |
| Checks | worktree presence — a readable `.simard-engineer-claim`; absent/unreadable → `false` |
| Does not check | engineer PID liveness — that is discovery's responsibility |
| Goal binding | via `self` / the `<goal-id>-<rand>` dir name — the sentinel carries no goal-id |

`is_present()` sits next to the existing accessors:

- `path(&self) -> &Path` — the worktree location on disk.
- `branch(&self) -> &str` — the branch checked out in the worktree.
- `cleanup(&self) -> Result<(), SimardError>` — idempotent removal.

The discovery path only holds a `PathBuf` (not an `EngineerWorktree`), so it
uses the same `read_engineer_claim_full(&path)` primitive that `is_present()`
wraps, rather than the method.

### `find_live_engineer_for_goal` (unchanged; callers hardened)

```rust
pub fn find_live_engineer_for_goal(
    state_root: &std::path::Path,
    goal_id: &str,
) -> Option<std::path::PathBuf>;
```

Discovery already reads the claim sentinel and re-checks PID liveness
synchronously immediately before returning `Some(path)` (`spawn.rs:822-847`), so
it never returns a path whose sentinel was already gone at scan time. The
residual TOCTOU is **caller-side**: the reaper can delete the directory between
this return and the caller's use of the path. The feature closes that window by
having every reuse/report site presence-check the path at the moment of use (see
the table below). The signature and body of `find_live_engineer_for_goal` are
unchanged.

## Behaviour at a goal cycle

The guard is applied at the three sites where `advance_goal` and its per-cycle
consumers reuse or report an existing engineer worktree. In each case absence is
**not** an error — it is a logged, safe fall-through to a fresh allocation.

| Reuse/report site | Location | On present | On absent (reaped) |
| --- | --- | --- | --- |
| Assigned-engineer short-circuit | `typed_goal_session.rs:340` (`already_assigned`) | keep the existing engineer (no re-spawn) | `tracing::warn!` + clear `goal.assigned_to` + drop the stale map entry + `allocate()` |
| Discovery reuse | `typed_goal_session.rs:362` (`find_live_engineer_for_goal`) | report the existing engineer as `Succeeded` | `tracing::warn!` + fall through to `allocate()` |
| Stored-map reuse | `subordinate.rs:33` (heartbeat/artifact path) and `cycle.rs:1613` (`worker_present`) | reuse the stored worktree | `tracing::warn!` + drop the stale `engineer_worktrees` entry + `allocate()` |

Re-provisioning always produces a **clean** worktree via
`EngineerWorktree::allocate()` — it never partially reuses the reaped
directory's contents.

### Structured logging

Absence is surfaced through structured `tracing` (and OTel spans) only — there
are no `print!`/`println!` calls, per the repo's structured-observability
convention. Representative fields on the warning:

```text
level=WARN
event="engineer_worktree.reaped_before_reuse"
goal_id=<goal id>
worktree=<absolute path that was expected>
action="reprovision"
```

Operators can alert on `engineer_worktree.reaped_before_reuse` to track how
often the reaper races cycle reuse. A low, non-zero rate is expected and
healthy; a spike indicates the reaper is too aggressive relative to cycle
cadence.

## Guarantees and limits

- **Fail closed, visibly.** Absence never silently degrades into reusing the
  wrong resource. It always becomes a `warn!` plus a clean re-provision.
- **No confused-deputy reuse.** Goal binding is enforced by
  `find_live_engineer_for_goal`'s `<goal-id>-*` scan and by the fact that each
  `EngineerWorktree` is allocated for exactly one goal — a cycle never adopts a
  directory belonging to a different goal. (Note: the claim sentinel itself
  carries no goal-id, so it is not the ownership authority — see
  [Design decisions](#design-decisions).)
- **Residual window is bounded, not eliminated.** A worktree can still be
  removed after the presence check returns `true` but before the engineer starts
  writing. That remaining window is the engineer runtime's existing
  fail-loud responsibility; the common reaped-before-reuse case is now a clean
  re-provision instead of a crash.
- **No double-allocation storms.** Re-provision runs under the state lock: the
  `assigned_to` field is cleared and re-set atomically, so only one cycle can win
  the re-provision for a given goal and two cycles cannot both observe "absent"
  and both allocate. Re-provision is not retried in a tight loop, avoiding a
  reap ↔ re-provision livelock.

## Configuration

The presence guard has **no new configuration knobs** — it is always on and
additive. It reuses the existing worktree layout and env vars documented in
[engineer-worktree-isolation](./engineer-worktree-isolation.md):

| Setting | Effect on the guard |
| --- | --- |
| `SIMARD_STATE_ROOT` | root under which `engineer-worktrees/<goal-id>-*` (and the checked claim sentinel) live |
| worktree reaper / sweep cadence | how often a worktree can disappear between cycles; see [sweep-safety](./engineer-worktree-sweep-safety.md) |

## Examples

### Re-provisioning an already-assigned engineer whose worktree was reaped

```rust
// Assigned-engineer short-circuit (replaces the bare permanent error at
// typed_goal_session.rs:340).
if already_assigned {
    let present = guard
        .engineer_worktrees
        .get(goal_id)
        .map(|w| w.is_present())
        .unwrap_or(false);
    if present {
        // Genuinely running engineer — nothing to re-provision this cycle.
        return Ok(/* existing-engineer outcome */);
    }
    // Assigned but reaped: clear the stale assignment and fall through.
    tracing::warn!(
        event = "engineer_worktree.reaped_before_reuse",
        goal_id,
        action = "reprovision",
    );
    goal.assigned_to = None;
    guard.engineer_worktrees.remove(goal_id);
    // ...fall through to allocate() a clean replacement.
}
```

### Reusing a discovered engineer worktree safely

```rust
// Discovery reuse path — we only hold a PathBuf here, so use the same
// claim-read primitive that is_present() wraps.
if let Some(path) = find_live_engineer_for_goal(&state_root, goal_id) {
    if read_engineer_claim_full(&path).is_some() {
        // Directory still present: report the existing engineer.
        return Ok(EffectResult::Succeeded { /* existing engineer evidence */ });
    }
    // Reaped between discovery and reuse: warn and fall through to allocate.
    tracing::warn!(
        event = "engineer_worktree.reaped_before_reuse",
        goal_id,
        worktree = %path.display(),
        action = "reprovision",
    );
}
let worktree =
    EngineerWorktree::allocate(&parent_repo, &state_root, goal_id)?;
```

### Guarding a stored worktree before reuse

```rust
// Stored-map path (subordinate.rs / cycle.rs consumers).
if let Some(worktree) = guard.engineer_worktrees.get(goal_id) {
    if worktree.is_present() {
        // Safe: the worktree directory still holds a claim sentinel.
        reuse(worktree);
    } else {
        tracing::warn!(
            event = "engineer_worktree.reaped_before_reuse",
            goal_id,
            worktree = %worktree.path().display(),
            action = "reprovision",
        );
        guard.engineer_worktrees.remove(goal_id); // drop the stale entry
        // ...then allocate() a clean replacement.
    }
}
```

## Verifying the guard

```bash
# Unit test: is_present() is true after allocate, false after cleanup.
cargo test engineer_worktree::

# Regression tests: reuse-after-reap and stored-map staleness re-provision
# cleanly instead of crashing with a missing-workspace fault.
cargo test ooda_actions::advance_goal
```

Expected results:

- `is_present()` returns `true` immediately after `allocate()` and `false`
  after `cleanup()` (or any external removal of the directory).
- A cycle whose worktree is reaped between discovery and reuse emits
  `engineer_worktree.reaped_before_reuse` and completes by allocating a fresh
  worktree — no missing-workspace error, issue #4578 no longer reproduces.

## Design decisions

These record why the guard is shaped the way it is. They are settled decisions
that the implementation follows — not open questions — and each is grounded in
the current code.

1. **Presence-check the `already_assigned` short-circuit; don't keep the bare
   error.** `advance_goal` (`typed_goal_session.rs:340`) currently returns a
   **permanent** error (`"goal already has an assigned engineer"`) *before*
   `find_live_engineer_for_goal` or any presence check runs. A goal whose
   `assigned_to` is still set but whose worktree was reaped therefore fails
   permanently instead of re-provisioning — the exact reaped-before-reuse case
   this guard targets. The feature replaces that short-circuit with a presence
   check: present → keep the engineer; absent → `warn!`, clear `assigned_to`,
   drop the stale map entry, and re-provision.

2. **Guard the real stored-map consumers, not the insert site.** In
   `advance_goal` the `state.engineer_worktrees` map is **insert-only**
   (`typed_goal_session.rs:432`). The actual per-cycle consumers of a stored
   worktree are `advance_goal_with_subordinate` (`subordinate.rs:33`, which reads
   `engineer_worktrees.get(goal_id)` for the heartbeat/artifact path and today
   falls back to `"."`) and `cycle.rs:1613` (`worker_present =
   engineer_worktrees.contains_key(goal_id)`). The stored-map presence check is
   therefore sited there, not in the `advance_goal` insert branch. Note that
   `cycle.rs:1613` currently only tests map-key membership (`contains_key`); the
   guard upgrades that to an on-disk presence check so a reaped worktree no
   longer counts as a live worker.

3. **Do not extend the sentinel format.** The claim sentinel stays
   `<pid>\n<starttime>` (`claim.rs:37-45`). Goal ownership is already enforced by
   the `<goal-id>-<rand>` directory name and the `<goal-id>-*` discovery glob;
   adding a goal-id to the sentinel would duplicate that binding for no gain.
   `is_present()` therefore verifies presence, not goal ownership.

4. **`is_present()` is a single claim read, not lstat + read.**
   `read_engineer_claim_full` already fails closed when the directory is gone
   (its `read_to_string` errors → `None`), so an explicit extra `lstat` before
   the claim read buys nothing. The single claim read is the smallest sufficient
   seam (ruthless simplicity).

## See also

- [Per-Engineer Worktree Isolation](./engineer-worktree-isolation.md) — the
  allocator, filesystem layout, and claim sentinel this guard builds on.
- [Engineer Worktree Sweep Safety](./engineer-worktree-sweep-safety.md) — the
  reaper that races reuse and the safety guards it already honors.
- [Diagnose a deferred engineer spawn](../howto/diagnose-a-deferred-engineer-spawn.md).
