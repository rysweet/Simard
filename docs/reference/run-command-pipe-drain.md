---
title: run_command_inner concurrent pipe drain
description: >
  How Simard's OODA-core subprocess helper (`run_command_inner`) captures child
  stdout and stderr without deadlocking on large output. Both pipes are drained
  concurrently on dedicated reader threads while the poll loop watches for exit
  and enforces the per-command timeout, so a child writing more than the OS pipe
  buffer (~64 KiB) can never stall the loop. Closes issue #4360.
last_updated: 2026-07-20
review_schedule: when the subprocess capture path changes or the timeout model changes
owner: ooda-core
doc_type: reference
related:
  - ../testing/scaling-and-cost-ledger-flake-fixes.md
  - ./cognitive-memory-client-helpers.md
---

# `run_command_inner` concurrent pipe drain

`run_command_inner` is the single subprocess helper used throughout the OODA
engineer loop to run `git` and `cargo` invocations (status, rev-parse, add,
commit, test, …). It lives in `src/engineer_loop/execution/mod.rs` and is reached
through the two public wrappers:

| Wrapper                        | Non-zero exit | Returns                              |
| ------------------------------ | ------------- | ----------------------------------- |
| `run_command`                  | → `Err`       | `Ok(CommandOutput)` on success only |
| `run_command_allow_failure`    | tolerated     | `Ok(CommandOutput)` with captured stdout even on non-zero exit |

Both delegate to `run_command_inner(cwd, argv, allow_nonzero_exit)`.

## What it guarantees

- **No pipe-buffer deadlock.** stdout and stderr are read continuously on
  separate threads for the entire lifetime of the child. A child that emits
  more than the OS pipe buffer (~64 KiB on Linux) to either stream — or to both
  — runs to completion; the full output is captured. This is the behavior that
  issue [#4360](https://github.com/rysweet/Simard/issues/4360) was filed to fix.
- **Bounded by the command timeout, not by output size.** The poll loop still
  enforces `timeout_for_command(argv)` (`CARGO_COMMAND_TIMEOUT_SECS` for `cargo`,
  `GIT_COMMAND_TIMEOUT_SECS` otherwise). On deadline the child is killed and a
  `SimardError::CommandTimeout` is returned.
- **Argv-only execution.** The command is always run as an argument vector via
  `Command::new(program).args(args)`. No `sh -c` string is ever constructed, so
  the drain refactor introduces no shell-injection surface.
- **Cleaned git environment.** Every variable in `CLEARED_GIT_ENV_VARS` is
  removed from the child environment before spawn, exactly as before.

## The deadlock this eliminates

### Mechanism

The child and parent share fixed-size OS pipes for stdout and stderr. When a
parent only reads *after* the child exits, the following can occur:

```text
child:  writes 1 MiB to stdout ─▶ fills the ~64 KiB pipe buffer ─▶ blocks on write
parent: loops on try_wait() + sleep ─▶ never drains the pipe ─▶ waits for exit forever
```

Neither side can make progress: the child is blocked writing, the parent is
blocked waiting for an exit that will never come. Any `git` or `cargo` command
with large output (a big `cargo test` log, a wide `git status`, a verbose diff)
could trip it.

### Why the old loop was vulnerable

The pre-fix loop polled `child.try_wait()` on a 50 ms sleep and only called
`child.wait_with_output()` **after** the child had already exited. Nothing read
the pipes while the child was still running, so the buffer could fill and stall
the child before it ever exited.

## How the drain works (finished behavior)

Immediately after spawn, `run_command_inner` takes the child's `stdout` and
`stderr` handles and moves each onto its own reader thread. Each thread reads its
stream to EOF into an owned buffer. The main thread then runs the existing
poll/timeout loop:

```text
spawn child (stdout=piped, stderr=piped)
  ├─ thread S: read child.stdout → Vec<u8>   (runs until EOF)
  ├─ thread E: read child.stderr → Vec<u8>   (runs until EOF)
  └─ main: loop { try_wait()?; if exited break; if past deadline kill+timeout; sleep }
        │
        └─ join(S), join(E)  → full stdout / stderr bytes
```

Because the reader threads drain both pipes concurrently, the child never blocks
on a full buffer, so it always reaches exit and the loop always terminates.

### Invariants preserved

The refactor is additive and non-breaking. Every one of these behaviors is
identical to the pre-#4360 code:

- `CommandOutput { stdout }` — the returned struct is unchanged; only the
  sanitized stdout string is exposed to callers.
- **stderr-in-error.** On a non-tolerated non-zero exit, the error message
  prefers sanitized `stderr` and falls back to sanitized `stdout` when stderr is
  blank — same wording, same `SimardError` variants.
- **`NotARepo` special case.** A failing `git rev-parse --show-toplevel` still
  maps to `SimardError::NotARepo { path, reason }`; every other failure maps to
  `SimardError::ActionExecutionFailed { action, reason }`.
- **Argv validation.** Empty argv, and any segment that is empty or contains a
  newline / carriage return, still returns `ActionExecutionFailed` before spawn.
- **Sanitization.** Captured stdout and stderr are passed through
  `sanitize_terminal_text` exactly as before.
- **Thread & handle hygiene.** Both reader threads are always joined and all
  handles are dropped on every exit path — success, non-zero exit, timeout, and
  poll error — so no file descriptor or thread is leaked.

## Error surface

| Condition                                   | Error                                        |
| ------------------------------------------- | -------------------------------------------- |
| Empty argv                                  | `ActionExecutionFailed { action: "<empty>" }`|
| Empty / multi-line argv segment             | `ActionExecutionFailed`                      |
| Spawn failure                               | `ActionExecutionFailed`                      |
| Deadline exceeded                           | `CommandTimeout { action, timeout_secs }`    |
| `git rev-parse --show-toplevel` non-zero    | `NotARepo { path, reason }`                  |
| Any other non-zero exit (not tolerated)     | `ActionExecutionFailed { action, reason }`   |

## Observability

The core fix is the pipe drain; it adds no logging of its own. `run_command_inner`
uses no `print!` / `println!`. **If** diagnostics are added around the capture
path, they must follow the P5 security constraint: use structured `tracing` /
OTel spans that record **metadata only** — argv, exit status, wall-clock
duration, captured byte counts — never raw child stdout/stderr at `info`. Raw
stream content, if surfaced at all, is gated behind `trace` level and redacted,
so large or sensitive command output is never emitted by default.

## Verification

A regression test spawns a child that emits ~1 MiB to stdout and/or stderr and
asserts that `run_command_inner` completes and captures the full output. Run it
with the CI command:

```bash
cargo test --all-features --locked --no-fail-fast \
  -- --skip install_packages_runs_and_self_installs
```

Before the fix this test hangs until the command timeout; after the fix it
returns the full captured output well under the timeout.
