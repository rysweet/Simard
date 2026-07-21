---
title: "Reference: Process-Group Guard API (GroupChild)"
description: >
  The API contract for the process_group_guard brick: the GroupChild RAII wrapper
  that spawns a child as its own process-group leader and group-kills the whole
  subtree on Drop (error, ?, timeout, panic) unless disarmed or reaped; its
  spawn / spawn_with / child_mut / reap / disarm surface; the ProcessGroupProbe
  signalling seam (LibcSignaller in production, a recording double in tests); the
  SIGTERM -> bounded grace -> SIGKILL escalation; the pgid>1 and kill-before-reap
  safety invariants; the wired call site (engineer-loop command timeout); and the
  regression tests. Cross-links amplihack-rs#964.
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/nested-subprocess-orphan-guard.md
  - ../howto/add-a-process-group-guarded-spawn.md
  - ./self-deploy-api.md
---

# Reference: Process-Group Guard API (`GroupChild`)

> **Status: implemented.** Present-tense description of shipped behaviour.
> Primary source:
> [`src/process_group_guard/`](https://github.com/rysweet/Simard/blob/main/src/process_group_guard/mod.rs).
> Conceptual overview:
> [Nested-Subprocess Orphan Guard](../concepts/nested-subprocess-orphan-guard.md).
> Upstream companion fix:
> [`rysweet/amplihack-rs#964`](https://github.com/rysweet/amplihack-rs/issues/964).

## Overview

`process_group_guard` is a self-contained brick exposing one primary type,
`GroupChild`: an RAII wrapper around `std::process::Child` that

1. spawns its child as the **leader of a new process group** (Unix
   `process_group(0)`), so every descendant shares one PGID; and
2. **group-kills that entire subtree on `Drop`** — on the failure exit paths
   `Err`, `?` early-return, a timeout branch, and panic-unwind — using
   `libc::kill(-pgid, …)` with a `SIGTERM → bounded grace → SIGKILL` escalation,
   unless the guard was explicitly `disarm()`ed or already `reap()`ed.

The module is registered in
[`src/lib.rs`](https://github.com/rysweet/Simard/blob/main/src/lib.rs) as
`pub mod process_group_guard;` and depends only on `std`, `libc`, and `tracing`.

```
src/process_group_guard/
    mod.rs           # module root: re-exports the public surface
    group_child.rs   # GroupChild: spawn / spawn_with / child_mut / reap / disarm + Drop
    probe.rs         # ProcessGroupProbe seam + LibcSignaller (+ test RecordingProbe)
    tests.rs         # #[cfg(test)] contract + safety-invariant unit tests
```

## Type: `GroupChild`

```rust
/// An RAII handle over a child spawned as its own process-group leader.
///
/// Dropping an *armed* guard (not disarmed, not reaped, `pgid > 1`) group-kills
/// the child and every descendant (SIGTERM -> grace -> SIGKILL via
/// `libc::kill(-pgid, ...)`). Because the guarantee rides on Drop it also fires
/// on `?`/early-return/timeout/panic-unwind.
pub struct GroupChild { /* private */ }

impl GroupChild {
    /// Spawn `cmd` as the leader of a fresh process group with the production
    /// signaller ([`LibcSignaller`]) and [`DEFAULT_GRACE`]. On Unix applies
    /// `cmd.process_group(0)` before `spawn()`. The spawn error is surfaced,
    /// never swallowed.
    pub fn spawn(cmd: &mut std::process::Command) -> std::io::Result<GroupChild>;

    /// Spawn with an injected signaller + grace window. Internal seam for tests
    /// that need a real child but a recording signaller / short grace.
    pub fn spawn_with(
        cmd: &mut std::process::Command,
        signaller: std::sync::Arc<dyn ProcessGroupProbe>,
        grace: std::time::Duration,
    ) -> std::io::Result<GroupChild>;

    /// The child's process-group id (equals its PID). The teardown target is
    /// `-pgid`.
    pub fn pgid(&self) -> i32;

    /// Mutable access to the inner `Child` (e.g. `.try_wait()`, `.stdout`).
    /// `None` once disarmed. Reaping through this handle does NOT disarm; call
    /// [`reap`](Self::reap) or [`disarm`](Self::disarm) to suppress teardown.
    pub fn child_mut(&mut self) -> Option<&mut std::process::Child>;

    /// Wait for the child to exit and mark it reaped so Drop will NOT re-signal
    /// a (possibly recycled) pgid. Returns the exit status, or `None` when there
    /// is no owned child.
    pub fn reap(&mut self) -> std::io::Result<Option<std::process::ExitStatus>>;

    /// Relinquish ownership so the child survives Drop (the single intentional
    /// detached spawn). After this, Drop performs no teardown. Returns the raw
    /// `Child` when one is owned.
    pub fn disarm(&mut self) -> Option<std::process::Child>;
}

/// Default grace window between the group SIGTERM and the escalating SIGKILL.
pub const DEFAULT_GRACE: std::time::Duration = std::time::Duration::from_secs(5);
```

### Drop semantics

| Precondition at Drop | Action |
|---|---|
| Disarmed (`disarm()` called) | **No signal.** Ownership was relinquished. |
| Reaped (`reap()` completed) | **No signal.** PID/PGID may be recycled — never re-signalled. |
| Armed, `pgid > 1` | `kill(-pgid, SIGTERM)` → poll `group_alive` up to `grace` → `kill(-pgid, SIGKILL)` if still alive. |
| Armed, `pgid <= 1` (0/1/negative) | **No negative-target signal.** Skip (fail-closed). |
| SIGTERM returns `ESRCH` (group already gone) | Skip escalation; nothing to kill. |

Escalation to `SIGKILL` emits exactly one `tracing::warn!` so an operator can see
which subtree needed forcing.

## Tuning: `DEFAULT_GRACE`

`GroupChild::spawn` uses [`DEFAULT_GRACE`] (5s) between the group SIGTERM and the
escalating SIGKILL, polling `group_alive` every 50ms so a group that exits
promptly is never force-killed just for being slightly slow. Tests pass a
`Duration::ZERO` grace via `spawn_with` (or the crate-internal `from_parts`
constructor) so the escalation loop makes a single liveness check and never
sleeps.

## Injection seam: `ProcessGroupProbe`

To keep teardown tests **offline, serial, and sleep-free**, the two OS-signalling
operations sit behind a trait so a recording double can assert the escalation
without touching a real process group.

```rust
/// Abstraction over process-group signalling.
pub trait ProcessGroupProbe: Send + Sync {
    /// Send `signal` to the whole group led by `pgid` (`kill(-pgid, signal)`).
    /// MUST reject `pgid <= 1` with an error and issue no signal (REQ-V1).
    fn signal_group(&self, pgid: i32, signal: i32) -> std::io::Result<()>;

    /// Whether the group led by `pgid` still has a live member
    /// (`kill(-pgid, 0) == Ok`). MUST return `false` for `pgid <= 1`.
    fn group_alive(&self, pgid: i32) -> bool;
}

/// Production signaller: numeric-PID `libc::kill(-pgid, …)` only. Enforces the
/// `pgid > 1` guard itself (defence in depth).
pub struct LibcSignaller;
```

`GroupChild` holds an `Arc<dyn ProcessGroupProbe>` (production: `LibcSignaller`).
A `#[cfg(test)]` `RecordingProbe` records every `(pgid, signal)` pair and answers
`group_alive` from a scripted flag, so `spawn_with`/`from_parts` can inject it in
unit tests.

## Safety invariants (must hold)

Enforced in code and asserted by the unit tests:

| Invariant | Rule |
|---|---|
| **REQ-V1: `pgid > 1`** | Never call `kill(-pgid, …)` unless `pgid > 1`. `0`/`1`/negative are rejected → skip. Prevents self-group signalling and host-wide broadcast. Enforced in both `GroupChild::Drop` and `LibcSignaller`. |
| **REQ-V2: kill-before-reap** | Group-kill only while the child is un-`reap()`ed. After reap the pgid may be recycled — never signal it. |
| **Drop is panic-safe** | Kill errors are dropped with `let _ = …`; no `unwrap`/`expect`/re-panic in `Drop`. |
| **Numeric-PID only** | Signalling is `libc::kill(-pgid, …)` — never `pkill`/`killall`/name-based (repo shell policy). |
| **Escalate, never lead with SIGKILL** | `SIGTERM` → bounded grace → `SIGKILL`. |

## Instrumentation

- One `tracing::warn!` on SIGKILL escalation, carrying `pgid` and `grace_ms` —
  **never** the child's `argv`/env (they may carry tokens or paths).
- All other teardown outcomes (graceful SIGTERM, skip) are silent.
- **No `print!`/`println!`** anywhere in the module — structured `tracing` only.

## Wired call site

`GroupChild` replaces the immediate-child-only kill at the daemon's one
manual-spawn-with-timeout site. The adoption is additive — no signature or
step-semantics change.

| Site | File | What changed |
|---|---|---|
| **Engineer-loop command timeout** | [`src/engineer_loop/execution/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/engineer_loop/execution/mod.rs) | `run_command_inner` spawns each `git`/`cargo` command through `GroupChild`. On timeout the old code called `child.kill()` (immediate child only), orphaning `cargo`'s `rustc`/build-script grandchildren; now returning the `CommandTimeout` error drops the guard, which group-kills the whole subtree. On success the guard is `disarm()`ed and output is collected exactly as before. |

### Adoption candidates (not yet wired)

The following sites launch nested subtrees and are the natural next adopters,
documented in the how-to. They are **not** wired by this change (each carries
distinct semantics that need care):

- **`recipe-runner-rs` runs** in
  [`src/ooda_brain/recipe_brain.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_brain/recipe_brain.rs)
  use blocking `Command::output()`, which is spawn-and-wait atomic — there is no
  in-process early-return window between spawn and wait for `Drop` to close.
  Orphans from a **whole-daemon abort** (OOM/SIGKILL, where `Drop` cannot run)
  are covered instead by the OS-level reaper in
  [`self_deploy::orphan`](./self-deploy-api.md).
- **Agent-supervisor direct-exec arm** in
  [`src/agent_supervisor/lifecycle/spawn.rs`](https://github.com/rysweet/Simard/blob/main/src/agent_supervisor/lifecycle/spawn.rs)
  spawns a **long-running detached subordinate** meant to outlive the call, so it
  would need the `disarm()` (survivor) pattern, not armed teardown.
- **Safe-update detached handover** in
  [`src/ooda_actions/advance_goal/spawn.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs)
  is the canonical `disarm()` case: the detached `simard safe-update` child must
  survive the parent. See [Safe Self-Update](../safe-self-update.md).

> **The tmux path is deliberately excluded.** Where Simard delegates lifecycle to
> `tmux`, the session owns its own teardown; wrapping it in `GroupChild` would be
> redundant. `GroupChild` targets **direct-exec** spawns that otherwise have no
> subtree cleanup.

## Regression coverage

| Test | Location | Asserts |
|---|---|---|
| Escalation (survives SIGTERM) | `src/process_group_guard/tests.rs` | Armed drop records `signal_group(pgid, SIGTERM)` then `SIGKILL` when the group stays alive. |
| Graceful only | `src/process_group_guard/tests.rs` | A group that exits on SIGTERM is **not** escalated to SIGKILL. |
| Never leads with SIGKILL | `src/process_group_guard/tests.rs` | The first teardown signal is always SIGTERM. |
| REQ-V1 fail-closed | `src/process_group_guard/tests.rs` | `pgid <= 1` (0/1/-1/`i32::MIN`) ⇒ **no** negative-target signal, in the guard and in `LibcSignaller`. |
| REQ-V2 kill-before-reap | `src/process_group_guard/tests.rs` | A `reap()`ed guard is never re-signalled. |
| Disarm survivor | `src/process_group_guard/tests.rs` | A `disarm()`ed guard emits no signal; a real disarmed child is handed back and survives. |
| **Real-subtree end-to-end** | [`tests/process_group_orphan_reaping.rs`](https://github.com/rysweet/Simard/blob/main/tests/process_group_orphan_reaping.rs) | An armed guard dropped on a simulated failure path tears down a **real** child + grandchild subtree — the grandchild leaves no orphan. This is the load-bearing proof for amplihack-rs#964's bug class. |

Required gates (merge blockers): `cargo fmt`, `cargo clippy -D warnings`, and
`cargo test` must pass; the guard path carries **no** `unwrap`/`expect` on the
signal/Drop path and **no** `print!`/`println!`.

## Cross-reference: `amplihack-rs#964`

This Simard-side hardening is the deliverable;
[`amplihack-rs#964`](https://github.com/rysweet/amplihack-rs/issues/964) is the
**upstream companion fix** for the same bug class inside `recipe-runner-rs`
itself (a separate repo Simard only invokes). The PR that ships `GroupChild`
links #964 for traceability.

## Related

- [Nested-Subprocess Orphan Guard (concept)](../concepts/nested-subprocess-orphan-guard.md)
- [How to add a process-group-guarded spawn](../howto/add-a-process-group-guarded-spawn.md)
- [Self-Deploy API — engineer-orphan reaper](./self-deploy-api.md)
- [Safe Self-Update](../safe-self-update.md)
