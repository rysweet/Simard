---
title: The subordinate reaper cross-checks the tmux pane before killing, so a reused PID is never the wrong process
description: Why kill_subordinate no longer sends SIGTERM to a cached PID blindly; how it now cross-checks the live tmux pane PID against the cached PID before calling libc::kill, refusing (with a structured warning) when they disagree — the OS has recycled the PID to an unrelated process — and falling back to the existing kill behaviour only when there is no pane identity to check (empty session_name or a failed pane query).
last_updated: 2026-07-17
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ../reference/subordinate-kill-pid-guard-api.md
  - ./stale-engineer-claim-reaper.md
---

# The subordinate reaper cross-checks the tmux pane before killing

> **Status: implemented (issue #4243).** `kill_subordinate` now cross-checks the
> subordinate's **live tmux pane PID** against its **cached PID** before issuing
> `libc::kill`, refusing to signal on a mismatch. Primary source:
> [`src/agent_supervisor/lifecycle/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/agent_supervisor/lifecycle/mod.rs).
> API details:
> [Subordinate kill PID-guard reference](../reference/subordinate-kill-pid-guard-api.md).

## The defect this fixes

When the supervisor reaps a subordinate it sends `SIGTERM` to a **cached** PID
captured at spawn time:

```rust
// BEFORE (#4243): trusts the cached PID unconditionally.
let ret = unsafe { libc::kill(handle.pid as libc::pid_t, libc::SIGTERM) };
```

PIDs are a finite, recycled resource. If the real subordinate exited and the OS
**reused** its PID for an unrelated process, this code sends `SIGTERM` to the
wrong process — a reliability and safety hazard in engineer lifecycle
management. `libc::kill` is an `unsafe` side effect with no identity check.

## The fix: gate the kill on pane identity

Each subordinate is launched inside a named tmux session/pane
(`SubordinateHandle::session_name`), and the live pane PID is queryable:

```rust
pub(super) fn query_pane_pid(session_name: &str) -> Option<u32>; // tmux list-panes -F '#{pane_pid}'
```

`kill_subordinate` now cross-checks before signalling:

1. If `session_name` is **non-empty** and `query_pane_pid` returns a PID:
   - **match** (`pane_pid == handle.pid`) → proceed with `libc::kill` as before.
   - **mismatch** (`pane_pid != handle.pid`) → **refuse**, emit a structured
     `warn!` (PID reuse detected), and do **not** signal. The handle is marked
     `killed = true` without any `libc::kill`: a recycled PID means the real
     subordinate has already exited, so the handle is retired rather than left
     to be re-reaped every pass.
2. If `session_name` is **empty**, or the pane query **errors/returns `None`**
   (no identity to check against) → fall back to the existing kill behaviour so
   normal teardown is never regressed; log at `debug`.

This makes the kill an **authorization control**: the PID/pane match gates the
unsafe `libc::kill`. It is deliberately conservative — it only *refuses* when it
has positive evidence of a mismatch, and preserves liveness (still kills) when no
pane identity is available.

The `reap_zombies` / `waitpid` path is untouched — it is already safe and does
not signal by cached PID.

## Why this shape

- **Fail toward not killing the wrong thing, but don't strand teardown.** A
  positive mismatch is the one case we must never signal through. Absence of a
  pane (empty name, tmux flake) falls back to prior behaviour so ordinary
  shutdown still works.
- **No shell.** The pane query uses argv-form `tmux list-panes`; no shell
  interpolation of `session_name`.
- **Structured logging only** — the refusal is a `tracing` `warn!` with the
  agent name, cached PID, and observed pane PID; no `print!/println!`.

## Verifying the behaviour

Tests in the lifecycle module assert:

- **Refuse on reuse** — with a non-empty `session_name` whose live pane PID
  differs from the cached PID, `kill_subordinate` does **not** call `libc::kill`
  and logs the refusal.
- **Kill on match** — with a matching pane PID, the correct subordinate is
  signalled as before.
- **Fallback on no identity** — empty `session_name` or a failed pane query falls
  back to the existing kill path.

See the [reference doc](../reference/subordinate-kill-pid-guard-api.md) for the
exact control flow and the test list.
