---
title: "Reference: Subordinate Kill PID-Guard API"
description: >
  The API contract for the PID-reuse guard in kill_subordinate: the
  query_pane_pid seam, the cross-check control flow (match → kill, mismatch →
  refuse + warn, no-identity → fall back to kill), the structured-trace refusal,
  the untouched reap_zombies/waitpid path, and the regression test list.
last_updated: 2026-07-17
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/pid-reuse-safe-subordinate-reaper.md
---

# Reference: Subordinate Kill PID-Guard API

> **Status: implemented (#4243).** Present-tense description of shipped
> behaviour. Primary source:
> [`src/agent_supervisor/lifecycle/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/agent_supervisor/lifecycle/mod.rs).
> Conceptual overview:
> [The subordinate reaper cross-checks the tmux pane before killing](../concepts/pid-reuse-safe-subordinate-reaper.md).

## Seam: `query_pane_pid`

```rust
/// Return the live PID owning the given tmux pane, or None if the session does
/// not exist or the query fails. Uses argv-form `tmux list-panes` (no shell).
pub(super) fn query_pane_pid(session_name: &str) -> Option<u32>;
```

## `kill_subordinate` control flow

```rust
pub fn kill_subordinate(handle: &mut SubordinateHandle) -> SimardResult<()>;
```

| Condition                                                   | Action                                             |
| ----------------------------------------------------------- | -------------------------------------------------- |
| `handle.killed`                                             | `Err(InvalidIdentityComposition)` (unchanged).     |
| `handle.pid == 0`                                           | No signal (unchanged).                             |
| `session_name` non-empty, `query_pane_pid == Some(pid)`, `pid == handle.pid` | Send `SIGTERM` via `libc::kill`.  |
| `session_name` non-empty, `query_pane_pid == Some(pid)`, `pid != handle.pid` | **Refuse**: `warn!`, no signal, mark handle killed without kill. |
| `session_name` empty **or** `query_pane_pid == None`         | Fall back to existing kill path; `debug!`.         |

`ESRCH` from `libc::kill` (process already exited) remains a benign success, as
before.

### Refusal log (structured only)

```rust
tracing::warn!(
    agent = %handle.agent_name,
    cached_pid = handle.pid,
    pane_pid = observed,
    "refusing SIGTERM: cached PID no longer owns the tmux pane (PID reuse)"
);
```

No `print!/println!` is introduced.

## Untouched: `reap_zombies` / `waitpid`

The zombie-reaping path does not signal by cached PID and is not modified by this
change; only the identity-gated `libc::kill` in `kill_subordinate` is guarded.

## Regression tests

| Test                                         | Asserts                                                          |
| -------------------------------------------- | --------------------------------------------------------------- |
| `kill_refuses_on_pid_reuse_mismatch`         | Non-empty session, pane PID ≠ cached PID ⟹ no `libc::kill`.      |
| `kill_proceeds_on_pane_identity_match`       | Pane PID == cached PID ⟹ correct subordinate signalled.         |
| `kill_falls_back_when_session_name_empty`    | Empty `session_name` ⟹ existing kill behaviour.                 |
| `kill_falls_back_when_pane_query_fails`      | `query_pane_pid == None` ⟹ existing kill behaviour.             |
| `refusal_is_logged_structured`              | Mismatch emits a `warn!` with agent name and both PIDs.         |
