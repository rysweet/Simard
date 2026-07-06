---
title: RustyClawd Bash-tool idle-liveness
description: How the RustyClawd adapter replaces its wall-clock cap on Bash tool calls with idle-liveness reaping, so a still-producing command is never SIGKILLed on elapsed time and only genuinely-hung children are reclaimed (issue #2607).
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../index.md
  - ./base-type-adapters.md
  - ./terminal-session-idle-detection.md
  - ../howto/start-a-meeting.md
---

# RustyClawd Bash-tool idle-liveness

> **Status: implemented (issue #2607).** This reference describes the shipped
> behavior of the RustyClawd Bash-tool idle-liveness change in
> `src/base_type_rustyclawd/tool_executor.rs`. The wall-clock cap has been
> replaced with idle-liveness reaping; every section below describes current
> behavior.

When the RustyClawd base type (`SIMARD_BASE_TYPE=rusty-clawd`) drives an LLM
turn, the model can request a `Bash` tool call. Simard runs that command locally
in `execute_tool_locally` (`src/base_type_rustyclawd/tool_executor.rs`). A single
tool call can legitimately run for a very long time — a build, a test suite, a
long `git clone`, a data crunch — while streaming output the whole way.

**The Bash tool has no wall-clock cap (issue #2607).** A long-but-productive
command runs unbounded as long as it keeps producing output. Liveness is
governed by an **idle-liveness window**: the child is reaped only when it
produces *no* output (neither stdout nor stderr) for the whole window — a
genuinely hung or wedged command. Every output chunk resets the clock, so a
working command is never killed regardless of total runtime.

This mirrors the meeting agent proxy's idle-liveness contract
(`SIMARD_MEETING_IDLE_LIVENESS_SECS`, see
[How to start a meeting](../howto/start-a-meeting.md)) and the engineer
subprocess's unbounded natural wait. The three converge on one rule: **elapsed
time never kills in-flight agent/LLM work; only sustained silence does.**

## The rule (issue #2607)

> A long-but-productive agent turn or LLM subprocess must never be SIGKILLed by
> an elapsed-time cap. Any wall-clock timeout that can kill in-flight agent work
> is a bug; it is replaced with idle-liveness detection (reap only after a
> generous window of **no** output), with a `0` escape hatch for fully unbounded
> operation.

Previously the Bash arm wrapped `child.wait_with_output()` in a
`tokio::time::timeout(120s, …)` and spawned with `ProcessSpawnConfig::default()`
(no isolation). That was a wall-clock cap: a command still streaming output at
the 120-second mark was abandoned mid-flight and mapped to `ClientError::Timeout`.
Worse, because the child was neither isolated into its own process group nor
signalled when the timed-out future was dropped, the abandoned command was
**orphaned** — it kept running detached while the tool call reported failure. The
#2607 change fixed both defects: the cap became an idle window, and the reaper
kills the whole process group.

## Behavior

| Situation | Before (wall-clock cap) | Now (idle-liveness) |
|-----------|----------------------|---------------------|
| Command streams output for 10 min | **Killed at 120 s** → `Timeout`, orphaned child | Runs to completion, output returned |
| Command silent (hung) for the window | Killed at 120 s | Killed after the idle window, subtree reaped |
| Command produces a burst, then finishes in 3 s | Returned normally | Returned normally |
| Escape hatch set to `0` | n/a | Never reaped — fully unbounded |

The idle clock resets on **every** stdout/stderr chunk. Total runtime is
irrelevant; only the gap between chunks matters.

## Configuration

The fix introduces one new environment variable:

| Env var | Default | Effect |
|---------|---------|--------|
| `SIMARD_RUSTYCLAWD_IDLE_LIVENESS_SECS` | _(unset — see below)_ | Idle-liveness window in seconds — the maximum time with **no output** before a Bash-tool child is treated as hung and reaped. `0` disables idle detection entirely (fully unbounded escape hatch). Unset or malformed falls back to the per-call `timeout` input, which itself defaults to `120` s. |

The resolution order for the window:

1. `SIMARD_RUSTYCLAWD_IDLE_LIVENESS_SECS` when set to a valid integer wins.
   - `n > 0` → idle window of `n` seconds.
   - `0` → idle detection disabled (`None`, fully unbounded).
   - malformed (non-integer) → falls through to the per-call default.
2. Otherwise the tool call's own `timeout` input (milliseconds, supplied by the
   model) is reinterpreted as the idle window.
3. Otherwise `120` seconds.

> **Semantics change, not a knob rename.** The per-call `timeout` field still
> exists and is still honored, but it is now an **idle** window, not a total
> wall-clock budget. A command that keeps emitting output past its nominal
> `timeout` is not killed. This is intentional per #2607.

### Examples

Tighten the idle window to 10 minutes for a debugging session:

```bash
SIMARD_RUSTYCLAWD_IDLE_LIVENESS_SECS=600 simard bootstrap --base-type rusty-clawd
```

Disable idle reaping entirely (fully unbounded — use when you knowingly run a
tool that can be silent for a long stretch, e.g. a quiet long-running solver):

```bash
SIMARD_RUSTYCLAWD_IDLE_LIVENESS_SECS=0 simard bootstrap --base-type rusty-clawd
```

Leave it unset to inherit the model-supplied per-call `timeout` (default 120 s
of *silence* tolerance):

```bash
simard bootstrap --base-type rusty-clawd
```

## Mechanism

The Bash arm streams the child's output and reaps only on sustained silence,
mirroring `meeting_backend::agent_proxy`'s reaper. This replaced the former
`wait_with_output()` call (which consumes both pipes and offers no incremental
signal) with an explicit streaming loop over taken stream handles.

### Spawn with isolation

The child is spawned via `rustyclawd_tools::spawn_with_isolation` using
`ProcessSpawnConfig::with_isolation()` (setsid) instead of the former
`ProcessSpawnConfig::default()`, so the child leads its own session/process group
(`pid == pgid`). This lets the reaper signal the whole subtree — the shell plus
anything it forked — so no descendant is left holding the stdout/stderr pipes
open. `spawn_with_isolation` returns a `tokio::process::Child`, so the loop takes
ownership of the pipes via `child.stdout.take()` / `child.stderr.take()`.

### Streaming idle loop

1. The taken `stdout` and `stderr` handles are read incrementally
   (line-buffered) into an accumulator. Each read resets
   `last_activity = Instant::now()`.
2. The loop selects between "a chunk arrived" and "the idle deadline elapsed":
   - **Chunk arrived** → append it and reset the deadline.
   - **`last_activity.elapsed() >= idle_window`** → the command has been silent
     for the whole window; reap it (see below) and return an honest idle error.
   - **Both streams at EOF and the child reaped** → return the collected
     `stdout` / `stderr` / `exit_code`.
3. When the resolved window is `None` (escape hatch `0`), the idle branch is
   omitted and the loop waits for the child unbounded.

A command that keeps producing output never satisfies the idle branch, so it
runs to natural completion no matter how long that takes.

### Reaping a hung child

On idle reap, Simard kills the entire process group with a numeric-PID signal
(`libc::kill(-(pid), SIGKILL)`), matching the repo's shell-free signal policy
(the same `kill_process_group` pattern used by the meeting agent proxy), then
`start_kill()`/`wait()` as a fallback. No orphan survives.

## API surface

### `execute_tool_locally(tool_name, tool_input) -> Result<serde_json::Value, ClientError>`

**Module:** `src/base_type_rustyclawd/tool_executor.rs`

The `Bash` arm keeps the same JSON shape on success — the output contract is
unchanged, so callers (`execution.rs`) need no changes:

```json
{
  "stdout": "…",
  "stderr": "…",
  "exit_code": 0
}
```

On a genuine idle reap it returns `ClientError::Timeout` carrying a message that
clearly identifies an **idle** reap — i.e. that the child was killed after the
window elapsed with no output, not on a total-runtime budget. The exact wording
is an implementation detail; the requirement is only that the message distinguish
an idle-timeout from a productive kill. Illustrative form:

```rust
Err(ClientError::Timeout(format!(
    "bash tool idle for {window:?} with no output; reaped genuinely-hung \
     child (idle-liveness, {IDLE_LIVENESS_ENV})"
)))
```

This is an **honest idle-timeout**, never a productive kill: it fires only after
the window elapses with zero output.

### `resolve_idle_window(env_raw, per_call_ms) -> Option<Duration>`

**Module:** `src/base_type_rustyclawd/tool_executor.rs` (private helper)

The private helper `resolve_idle_window(env_raw, per_call_ms)` resolves the
window from `SIMARD_RUSTYCLAWD_IDLE_LIVENESS_SECS`, the per-call `timeout` input,
and the `120` s default, per the [Configuration](#configuration) resolution
order. It is pure and env-free so the resolution logic is unit-tested
deterministically:

| Input | Result |
|-------|--------|
| `Some("600")` | `Some(600 s)` |
| `Some("0")` | `None` (unbounded) |
| `Some("nonsense")` | fallback to per-call / `120 s` |
| `None` (unset) | fallback to per-call / `120 s` |

## Tests

Guard tests live in `tool_executor.rs` under `#[cfg(test)]`, using small
windows for speed and determinism. The change landed with these three scenarios
(the acceptance criteria for #2607):

1. **Productive-but-slow is never killed.** With a 1 s idle window, a command
   that runs ~1.5 s wall-clock while emitting a `tick` every 0.1 s (max idle
   0.1 s) completes with `exit_code == 0` and all ticks captured. Proves total
   runtime exceeding the window does not trigger a kill.
2. **Genuinely idle is reaped, no orphan.** With a sub-second window, `echo
   start; sleep 999; …` returns `ClientError::Timeout` (message identifying an
   idle reap) within the window, and no surviving PID retains the command's
   unique marker in its `/proc/<pid>/cmdline` argv — proving the whole process
   group was reaped and no orphan leaks.
3. **`0` disables reaping.** With `SIMARD_RUSTYCLAWD_IDLE_LIVENESS_SECS=0`, the
   window resolver returns `None`; an otherwise-idle command is not reaped
   within a bounded observation window, and a normal command still completes.

Plus pure-function cases for the window resolver (the table above).

## Audit: wall-clock caps on agent work (issue #2607)

The #2607 audit swept every `tokio::time::timeout` / `wait_timeout` /
`recv_timeout` on a child, agent, or LLM path and classified each as **replace**
(a wall-clock cap that can kill in-flight agent work) or **keep** (idle-based,
a poll tick, a remote-job poll, or a non-agent infra bound). Exactly one
remaining wall-clock kill on local agentic work was found; this doc specifies its
replacement.

| Location | Semantics | Verdict |
|----------|-----------|---------|
| `base_type_rustyclawd/tool_executor.rs` — Bash arm | (was) Wall-clock 120 s on a tool child; killed productive work and orphaned it | **REPLACE → idle-liveness** (this doc, shipped) |
| `meeting_backend/agent_proxy.rs` — idle window | Idle-liveness (resets per chunk; `0` = unbounded) | KEEP — canonical template |
| `meeting_backend/agent_proxy.rs` — `recv_timeout(200 ms)` | Poll tick, not a turn cap | KEEP |
| `signal_conversation/channel.rs` — `signal_agent_mode()` | Routes Signal turns through the Meeting idle-liveness proxy | KEEP (already merged) |
| `engineer_loop/agent_spawn.rs` — `wait_with_output()` | Unbounded natural wait | KEEP — already compliant |
| `agent_supervisor` heartbeat staleness | Idle-based (SIGTERM on stale heartbeat), not wall-clock | KEEP |
| `terminal_session` idle detection | Idle-based, work-process aware — see [Terminal session idle detection](./terminal-session-idle-detection.md) | KEEP |
| `meeting_backend/close_guard.rs` | Graceful post-turn meeting-close budget (shutdown, not a turn) | KEEP |
| `copilot_task_submit/orchestration.rs` | Bounded polling of a **remote** GitHub agent job | KEEP — remote poll, not local agent I/O |
| `operator_commands_dashboard/*` (distributed/tmux/agent-log) | VM/SSH/tmux/websocket infra ops | KEEP — non-agentic infra |
| `bridge_subprocess/subprocess.rs`, `update_check.rs` | Channel-framing / update-check reads | KEEP — non-agentic |

**Keeps are deliberate, not oversights.** The rule targets agent turns and LLM
subprocesses. A quick HTTP health probe, a channel-framing read, a remote-job
poll, or a graceful-shutdown budget is legitimately bounded and stays bounded.

## Invariants

The implementation upholds the following invariants (the acceptance guarantees,
covered by the tests above):

- A child that produces **any** output within the window is never killed by the
  idle path, regardless of total runtime.
- The idle reaper signals the child's **own** process group (negated group-leader
  PID). It can never reach Simard's own process — pathological PIDs `≤ 1` are
  refused.
- On idle reap the whole subtree is killed before returning; no orphan survives
  the tool call.
- The success JSON contract (`stdout` / `stderr` / `exit_code`) is unchanged, so
  the RustyClawd execution path is unaffected.
- Setting the window to `0` yields `None` and fully unbounded operation — the
  loop then never evaluates an idle deadline.

## Related reading

- [Base type adapters](./base-type-adapters.md) — where the `rusty-clawd`
  adapter and its Bash tool sit in the adapter hierarchy.
- [Terminal session idle detection](./terminal-session-idle-detection.md) — the
  sibling idle mechanism for PTY engineer sessions.
- [How to start a meeting](../howto/start-a-meeting.md) — the meeting agent
  proxy's `SIMARD_MEETING_IDLE_LIVENESS_SECS`, the pattern this fix mirrors.
