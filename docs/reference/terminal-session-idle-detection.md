---
title: Terminal session idle detection
description: How Simard decides when a PTY terminal session is genuinely idle versus silently computing, and when to send SIGTERM.
last_updated: 2026-05-07
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../index.md
  - ./base-type-adapters.md
  - ./runtime-contracts.md
  - ../howto/view-agent-terminal-logs.md
---

# Terminal session idle detection

Simard runs engineer sessions inside a PTY terminal session (`PtyTerminalSession` in
`src/terminal_session/session.rs`). When the session finishes its work the child
process — `script -qefc ...` wrapping a shell — sometimes stays alive at a prompt
with no further work to do. Simard must detect this and send `SIGTERM` to reclaim
resources, but it must **not** fire prematurely during a long LLM computation that
produces no transcript output.

## Two-phase wait

The `finish()` method uses a two-phase strategy:

1. **Unlimited natural wait.** The loop calls `child.try_wait()` every second. If the
   child exits on its own, `finish()` returns immediately with the captured transcript.
   There is no wall-clock deadline for natural exit — agentic sessions can legitimately
   run for hours.

2. **Idle guard.** If the transcript file stops growing, a per-session idle timer starts.
   Once the timer exceeds `IDLE_TIMEOUT_SECS` (300 s) **and** the process tree contains
   no active work processes, Simard sends `SIGTERM` to the root child PID. A 2-second
   grace period follows; if the child still has not exited the session is treated as
   complete.

## Process-tree work detection

Long LLM calls (Copilot, amplihack) can compute silently for 10 + minutes without
writing a single byte to the transcript. A pure transcript-idle timer would kill these
runs prematurely.

Before firing `SIGTERM`, Simard walks the `/proc` filesystem to find every descendant
of the session's root PID and checks whether any of them are active work processes.

### `has_active_work_processes(root_pid: u32) -> bool`

**Location:** `src/terminal_session/session.rs` (private to the module)

**Algorithm:**

1. Read all numeric directories under `/proc/` and collect `(pid, ppid)` pairs by
   parsing the `PPid:` line in each `/proc/<N>/status` file. Entries that vanish
   mid-read are skipped gracefully.
2. DFS from `root_pid` (via a `Vec` stack with `pop()`) to build the full set of descendant PIDs.
3. For each descendant, read `/proc/<N>/comm`. If the trimmed comm matches any name in
   `WORK_PROCESS_NAMES`, return `true`.
4. If no match is found, or if all `/proc` reads fail, return `false`.

**Work process names** (`WORK_PROCESS_NAMES`):

| Name | What it represents |
|------|--------------------|
| `copilot` | GitHub Copilot CLI binary |
| `node` | Node.js process backing Copilot SDK |
| `amplihack` | amplihack orchestration binary |

This list is intentionally conservative: it only contains processes that indicate an LLM
call is in-flight. Generic shell processes (`bash`, `sh`, `script`) are not included.

**Platform:** Linux only (`#[cfg(unix)]`). On non-Unix targets the function always
returns `false`, preserving the original behaviour.

### SIGTERM gate

```text
transcript_idle >= IDLE_TIMEOUT_SECS  AND  NOT has_active_work_processes(child_pid)
  → send SIGTERM
```

The guard suppresses `SIGTERM` while any of `copilot`, `node`, or `amplihack` remains
in the process tree. Once the LLM call exits — and the shell wrapper is the only thing
left — the transcript-idle timer fires and the session is reaped.

## Constants

| Constant | Value | Meaning |
|----------|-------|---------|
| `IDLE_TIMEOUT_SECS` | `300` | Seconds of no transcript growth before the idle guard activates |

## Logging

When `SIGTERM` is sent, Simard logs to `stderr`:

```text
[simard] terminal session pid=<N> idle for 300s after copilot exit — sending SIGTERM
```

This message only appears when the process tree is clean (work processes have exited),
so it is safe to treat it as confirmation that the session completed normally rather than
a sign of premature termination.

## Transcript file

The transcript is a temporary file at `$TMPDIR/simard-terminal-shell-transcript-<uuid>.log`.
It is created with exclusive mode (0o600) on open and deleted on drop via `TranscriptGuard`.
The idle timer compares `std::fs::metadata(...).len()` every second.

## Invariants

- `SIGTERM` targets only `self.child.id()` (the root PTY wrapper PID), never a process
  group. Downstream processes receive the signal through normal parent-exit propagation.
- The 2-second grace window after `SIGTERM` is unconditional. If the child does not exit
  in 2 seconds the session is declared complete and `finish()` returns.
- `has_active_work_processes` is entirely read-only. It never writes to `/proc` or
  sends signals. Missing or vanishing `/proc` entries are ignored without error.

## Related reading

- [Base type adapters](./base-type-adapters.md) — Where `PtyTerminalSession` sits in the
  adapter hierarchy.
- [How to view agent terminal logs](../howto/view-agent-terminal-logs.md) — Inspecting
  transcript files while a session is running.
- [Runtime contracts](./runtime-contracts.md) — The broader contract that terminal
  sessions must honour.
