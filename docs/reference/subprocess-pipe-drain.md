---
title: Subprocess pipe-drain execution API reference
description: >
  Reference for Simard's OODA-core subprocess execution helper
  `run_command_inner` — how it concurrently drains a child's stdout and stderr
  while polling for exit or timeout so a child emitting more than a pipe buffer
  (>64 KB) can never deadlock. Specifies the concurrent reader-thread design,
  the timeout/kill/join lifecycle, preserved `CommandOutput` semantics, error
  mapping (including `NotARepo`), and sanitization (#4360).
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../reference/terminal-failure-diagnosis-api.md
  - ../reference/engineer-loop-argv-sanitization.md
  - ../architecture/engineer-agent-orchestration.md
  - ../howto/diagnose-and-recover-ooda-step-failures.md
  - ../../src/engineer_loop/execution/mod.rs
  - ../../src/engineer_loop/tests_issue_4360_pipe_drain.rs
---

# Subprocess pipe-drain execution API reference

> **Status: implemented.** The executor lives in
> [`src/engineer_loop/execution/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/engineer_loop/execution/mod.rs)
> as `run_command_inner`, reached through the `run_command` and
> `run_command_allow_failure` wrappers. It drains stdout and stderr on
> dedicated reader threads while polling `try_wait()`, so a child producing
> more than one pipe buffer of output no longer deadlocks. Closes
> [#4360](https://github.com/rysweet/Simard/issues/4360).

`run_command_inner` is the shared OODA-core helper that every engineer-loop
`git`/`cargo` invocation runs through. It spawns a child with piped stdout and
stderr, waits for it to finish (or times out), and returns the captured stdout.
This page specifies how it drains those pipes without deadlocking.

## Contents

- [The deadlock it fixes](#the-deadlock-it-fixes)
- [Execution model](#execution-model)
- [`CommandOutput`](#commandoutput)
- [Public entry points](#public-entry-points)
- [Lifecycle](#lifecycle)
- [Timeout handling](#timeout-handling)
- [Capture cap](#capture-cap)
- [Preserved semantics](#preserved-semantics)
- [Observability](#observability)
- [Configuration](#configuration)
- [Security](#security)
- [Tests](#tests)

## The deadlock it fixes

A child process writing to a pipe blocks once the OS pipe buffer (commonly
64 KB on Linux) fills and no one is reading the other end. The pre-#4360
executor polled `child.try_wait()` in a loop and only called
`wait_with_output()` — which drains the pipes — **after** the child had
already exited. A child that emitted more than ~64 KB on stdout (or stderr)
therefore blocked on its write, never exited, and the poll loop spun until the
command timeout: a classic reader/writer pipe deadlock.

The fix is to **drain the pipes concurrently with the wait**, so the child can
always make forward progress regardless of output volume.

## Execution model

```text
                 ┌────────────────────────┐
   child stdout ─┤ stdout reader thread    ├─▶ Vec<u8> (≤16 MiB retained; rest drained)
                 └────────────────────────┘
                 ┌────────────────────────┐
   child stderr ─┤ stderr reader thread    ├─▶ Vec<u8> (≤16 MiB retained; rest drained)
                 └────────────────────────┘
   main thread: poll try_wait() until exit or deadline
                 └─ on deadline: child.kill() → wait() → join threads
```

On spawn, `run_command_inner` takes ownership of the child's `stdout` and
`stderr` handles and moves each into its own `std::thread`. Each reader thread
reads into a `Vec<u8>` up to a per-stream [capture cap](#capture-cap), so both
pipes are drained continuously and the child never blocks on a full buffer. The
main thread polls `try_wait()` for exit (or the timeout deadline) and then
**joins** both reader threads to collect the captured output. `wait_with_output()`
is deliberately not called — the reader threads own the pipe handles.

## `CommandOutput`

```rust
pub(crate) struct CommandOutput {
    pub(crate) stdout: String,
}
```

`CommandOutput` is **unchanged** — callers still receive sanitized stdout as a
`String`. stderr is captured (to keep the pipe drained and to build error
messages) but, as before, is not surfaced on the success struct.

## Public entry points

The two wrappers and their behaviour are unchanged:

```rust
/// Run a command; error on spawn failure, empty argv, timeout, OR non-zero exit.
pub(crate) fn run_command(cwd: &Path, argv: &[&str]) -> SimardResult<CommandOutput>;

/// Like `run_command` but tolerates a non-zero exit code: returns `Ok` with
/// whatever stdout was captured. Still errors on spawn failure, empty argv, or
/// timeout.
pub(crate) fn run_command_allow_failure(cwd: &Path, argv: &[&str]) -> SimardResult<CommandOutput>;
```

Both delegate to `run_command_inner(cwd, argv, allow_nonzero_exit)`.

## Lifecycle

1. **Validate argv.** Empty argv, or any segment that is empty or contains a
   newline/carriage return, is rejected with
   `SimardError::ActionExecutionFailed` (argv-only, single-line contract —
   unchanged).
2. **Spawn** with `stdout(piped())`, `stderr(piped())`, `current_dir(cwd)`, and
   the standard `CLEARED_GIT_ENV_VARS` removed.
3. **Start reader threads.** Move the child's `stdout`/`stderr` into two threads
   that read into buffers up to the per-stream [capture cap](#capture-cap).
   `wait_with_output()` is **not** used — the reader threads now own the pipe
   handles, so calling it would double-drain (and panic on the moved handles);
   the child is reaped with `wait()` instead.
4. **Poll to completion.** Loop on `try_wait()` with a short sleep, checking the
   `timeout_for_command(argv)` deadline each iteration.
5. **Join readers.** After the child exits, join both reader threads to obtain
   the captured stdout/stderr byte buffers (each truncated at the cap).
6. **Map result.** On non-zero exit (and not `allow_nonzero_exit`), build the
   error from the drained stderr/stdout; otherwise return sanitized stdout.

## Timeout handling

The command timeout (`timeout_for_command`: `CARGO_COMMAND_TIMEOUT_SECS` for
`cargo`, else `GIT_COMMAND_TIMEOUT_SECS`) still bounds total runtime. On
deadline the executor performs a full, leak-free teardown:

- `child.kill()` — terminate the child,
- `child.wait()` — reap it (no zombie),
- **join both reader threads** — the killed child closes its pipe write ends,
  so the readers hit EOF and return, preventing a thread or file-descriptor
  leak,
- return `SimardError::CommandTimeout { action, timeout_secs }`.

Because the pipes are drained throughout, the timeout now only fires for a
genuinely slow/hung child — never for the "too much output" case that #4360
reported. Note the timeout bounds **runtime**, not bytes: a fast child can emit
a large volume well inside the deadline, which is why captured volume is bounded
separately by the [capture cap](#capture-cap).

## Capture cap

Each reader thread caps its buffer at a per-stream limit
(`MAX_CAPTURED_BYTES`, default 16 MiB per stream). This bounds captured volume
directly, independent of the timeout: a fast child spewing gigabytes (e.g. a
runaway `cargo` build log) cannot exhaust memory because each reader stops
appending once the cap is reached.

On reaching the cap, the reader thread **keeps draining and discarding** the
remaining bytes so the child never blocks on a full pipe — only the retained
buffer is bounded, not the drain. The retained buffer is marked truncated, and
the returned/embedded text carries a trailing
`… [output truncated at 16 MiB]` marker so callers and error messages signal
that capture was clipped rather than silently losing the tail.

The cap applies to both stdout and stderr independently. It exists purely as a
memory-safety backstop; normal `git`/`cargo` output is far below it, so typical
runs are unaffected.

## Preserved semantics

This is an additive/non-breaking fix. The following are all preserved exactly:

- **Signatures & return type** — `run_command`, `run_command_allow_failure`,
  `CommandOutput`.
- **Output ordering** — stdout and stderr are captured on independent streams;
  each returned buffer preserves the child's own byte order for that stream.
- **Sanitization** — returned stdout (and stderr used in error text) still
  passes through
  [`sanitize_terminal_text`](https://github.com/rysweet/Simard/blob/main/src/sanitization.rs).
- **Error mapping** — a failed `git rev-parse --show-toplevel` still maps to
  `SimardError::NotARepo { path, reason }`; other non-zero exits map to
  `SimardError::ActionExecutionFailed`. The `reason` string still prefers
  stderr, falling back to stdout when stderr is empty.
- **`allow_nonzero_exit`** — still returns captured stdout on a non-zero exit.

## Observability

- The executor emits no stray `print!`/`println!`. Diagnostic context is
  carried through the returned `SimardError` variants (and structured `tracing`
  at the call sites), consistent with the terminal-failure diagnosis surface —
  see [Terminal failure diagnosis API](../reference/terminal-failure-diagnosis-api.md).

## Configuration

No new configuration. The existing per-command timeout constants
(`CARGO_COMMAND_TIMEOUT_SECS`, `GIT_COMMAND_TIMEOUT_SECS`) continue to bound
runtime, and the per-stream `MAX_CAPTURED_BYTES` cap bounds captured volume.

## Security

- **Terminal-injection defense.** Concurrently drained buffers still pass
  through `sanitize_terminal_text` before they are returned or embedded in error
  messages, so escape sequences from subprocess output cannot reach a terminal
  unfiltered.
- **Resource exhaustion.** The timeout plus `child.kill()` + `wait()` + thread
  join bounds runtime, threads, file descriptors, and PIDs; no zombie processes
  or leaked reader threads survive a timeout. Captured **memory** is bounded
  separately by the per-stream `MAX_CAPTURED_BYTES` cap, so a fast, high-volume
  child cannot exhaust memory even well inside the timeout window.
- **No new attack surface.** argv is still validated (non-empty, single-line
  segments), no `Command` arguments are constructed from untrusted data, and the
  git env-scrubbing (`CLEARED_GIT_ENV_VARS`) is unchanged.

## Tests

- **`src/engineer_loop/tests_issue_4360_pipe_drain.rs`** — spawns children that
  write well over 64 KB (~200 KiB) on stdout, on stderr, and on **both** streams
  concurrently, asserting that each call completes promptly (does not stall
  until timeout) and captures the full output on each stream. These tests
  deadlock against the pre-#4360 executor and
  passes against the concurrent-drain implementation.
- **Existing execution tests** — the `git`/`cargo` happy-path, non-zero-exit
  (`allow_nonzero_exit`), `NotARepo` mapping, and timeout tests continue to
  pass unchanged, proving the fix is non-breaking.
- **Capture-cap test** — a child that emits more than `MAX_CAPTURED_BYTES` on a
  stream returns promptly with a buffer clipped to the cap and the trailing
  `… [output truncated at 16 MiB]` marker, confirming memory stays bounded and
  the drain continues past the cap.
