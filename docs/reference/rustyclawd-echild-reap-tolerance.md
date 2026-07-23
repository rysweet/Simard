---
title: RustyClawd tool-executor ECHILD reap tolerance
description: How execute_tool_locally tolerates an externally-reaped Bash child (ECHILD / errno 10) as a logged success instead of a spurious error, so fast/empty commands no longer intermittently fail under the daemon and the unit-test deploy-gate stops red-canarying Simard's self-deploy (issue #4506).
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../index.md
  - ./base-type-adapters.md
  - ./rustyclawd-bash-tool-idle-liveness.md
---

# RustyClawd tool-executor ECHILD reap tolerance

> **Status: implemented (issue #4506).** This reference describes the shipped
> behavior of the ECHILD reap-tolerance change in
> `src/base_type_rustyclawd/tool_executor.rs`. The `Bash` arm's two reap points
> now tolerate an externally-reaped child (`ECHILD`) as a logged success;
> every section below describes current behavior.

When the RustyClawd base type (`SIMARD_BASE_TYPE=rusty-clawd`) drives an LLM
turn, the model can request a `Bash` tool call, which Simard runs locally in
`execute_tool_locally` (`src/base_type_rustyclawd/tool_executor.rs`). After the
streaming loop finishes, the tool-executor **reaps** the child to read its exit
status — either optimistically via `try_wait` once both pipes have closed, or in
the terminal `wait` once the streams are at EOF.

**Reaping is now resilient to an externally-reaped child (issue #4506).** In the
daemon/canary environment the child can be collected out from under this reap by
another waiter in the process — tokio's own signal-driven child reaper or any
process-wide `waitpid(-1)`/`SIGCHLD` handler running in the daemon — a
legitimate race the tool executor does not own. When that happens,
`try_wait`/`wait` return **`ECHILD` (errno 10, "No child processes")** because
the kernel has already collected the child. Previously both reap arms mapped any
error — including this benign `ECHILD` — to `ClientError::Unknown("process error:
…")`, so a perfectly successful command (most visibly a fast/empty `sh -c ""`)
would intermittently fail. That single intermittent failure was enough to red the
`unit-test` deploy-gate and stall self-deploy one commit behind main.

The #4506 change makes `ECHILD` a **tolerated, logged success**: the child has
provably already exited (its status is simply unrecoverable), so the tool
executor synthesizes `exit_code: 0`, emits a `tracing::warn!`, and returns the
output it already collected. Any **other** errno is still a real failure and is
mapped to `ClientError::Unknown` exactly as before.

## The rule (issue #4506)

> A child that has been externally reaped (`ECHILD`) has already exited
> successfully from the tool executor's point of view — its status is merely
> unrecoverable. Treating that unrecoverable-status race as a tool failure is a
> bug. `ECHILD` is synthesized to `exit_code: 0` and **logged** (never
> silenced); every other reap error remains a genuine `ClientError::Unknown`.

`ECHILD` strictly means "no child processes" — the kernel has already collected
the child, so there is no exit status left to read. It never indicates that the
command itself failed; the only defensible synthesized status is success
(`exit_code: 0`). Because the collected `stdout`/`stderr` buffers are already
complete (both pipes reached EOF before the reap), no output is lost.

## Behavior

| Situation | Before | Now (ECHILD-tolerant) |
|-----------|--------|-----------------------|
| `sh -c ""` (empty/fast command), child externally reaped (tokio child-reaper / process-wide `waitpid(-1)`) before this reap | `ClientError::Unknown("process error: … (os error 10)")` — intermittent failure | `exit_code: 0`, collected output returned, one `tracing::warn!` logged |
| Normal command, executor reaps it itself | `exit_code` from the real status | Unchanged — real status returned |
| Command exits non-zero, executor reaps it | `exit_code` = real non-zero | Unchanged — real non-zero returned |
| Reap fails with any non-`ECHILD` errno (e.g. `EPERM`) | `ClientError::Unknown("process error: …")` | Unchanged — `ClientError::Unknown("process error: …")` |
| Idle-liveness reap of a genuinely hung child | `ClientError::Timeout` | Unchanged — see [idle-liveness](./rustyclawd-bash-tool-idle-liveness.md) |

Only the `ECHILD` reap arm changes. Every other path — real exit codes, non-zero
exits, non-`ECHILD` errors, and idle-liveness reaping — is byte-for-byte
identical to before.

## Configuration

**None.** This change introduces no new environment variable, flag, or config
key. `ECHILD` tolerance is unconditional and always on; there is nothing to tune.
The idle-liveness window
([`SIMARD_RUSTYCLAWD_IDLE_LIVENESS_SECS`](./rustyclawd-bash-tool-idle-liveness.md#configuration))
is unrelated and unaffected.

## Mechanism

Both reap points route their error through one private helper that classifies the
errno.

### `status_from_reap_error(e, reap_point) -> Result<ExitStatus, ClientError>`

**Module:** `src/base_type_rustyclawd/tool_executor.rs` (private helper)

A pure errno-to-status mapper used by both reap arms:

- **`e.raw_os_error() == Some(libc::ECHILD)`** → log a `tracing::warn!` (structured
  fields: `reap_point` and `error` only) and return `Ok(ExitStatus::from_raw(0))`
  — the externally-reaped child is treated as a successful exit.
- **any other errno** → return `Err(ClientError::Unknown(format!("process error:
  {e}")))`, preserving the previous behavior exactly.

The `ECHILD` match is **exact** (`Some(libc::ECHILD)`) — never a range or a
message-substring match — so it cannot accidentally swallow an unrelated error.
`libc::ECHILD` is referenced fully-qualified (matching the existing `libc::kill`
usage), and `ExitStatus::from_raw` is brought in via a function-scoped
`use std::os::unix::process::ExitStatusExt`. This is Unix-only, which is
consistent with the module's existing Unix assumptions (`setsid`, `libc::kill`).

### Reap point A — optimistic `try_wait` (streams closed)

Once both pipes have closed but the child may still be running, the loop polls
`child.try_wait()`. On `Err(e)` it now calls the helper:

```text
Err(e) => {
    exit_status = Some(status_from_reap_error(e, "try_wait")?);
    break;
}
```

An `ECHILD` here yields `exit_code: 0`; any other errno propagates the original
`ClientError::Unknown` via `?`.

### Reap point B — terminal `wait`

When the loop exits without having captured a status, the terminal `wait` reaps
the child. Its `Err(e)` arm routes through the same helper:

```text
None => match child.wait().await {
    Ok(status) => status,
    Err(e) => status_from_reap_error(e, "wait")?,
},
```

Identical semantics to reap point A: `ECHILD` → synthesized success; anything
else → `ClientError::Unknown`.

### Logging, not silencing

Every `ECHILD` synthesis emits a `tracing::warn!` carrying only the `reap_point`
(a static `"try_wait"` / `"wait"` marker) and the kernel `io::Error` display.
This upholds the zero-BS / no-silent-degradation policy: the tolerated race is
always visible in structured logs and OTel, never swallowed. The log
deliberately excludes `out_buf`/`err_buf`, the command string, and the
environment, so tool output and secrets are never leaked into logs. No
`print!`/`println!` is used (structured tracing + OTel only).

## API surface

### `execute_tool_locally(tool_name, tool_input) -> Result<serde_json::Value, ClientError>`

**Module:** `src/base_type_rustyclawd/tool_executor.rs`

The `Bash` arm's success JSON shape is **unchanged** — callers
(`execution.rs`) need no changes:

```json
{
  "stdout": "…",
  "stderr": "…",
  "exit_code": 0
}
```

On an `ECHILD` reap, the tool executor returns exactly this shape with
`exit_code: 0` and the `stdout`/`stderr` already collected before the reap. On a
non-`ECHILD` reap error it returns `ClientError::Unknown("process error: …")`,
unchanged from before. Idle-liveness reaps still return `ClientError::Timeout`.

The `exit_code` field is emitted by the unchanged
`status.code().unwrap_or(-1)` line, so the synthesized status must yield
`code() == Some(0)` — not `None` (which would surface as `-1`). This is exactly
why the helper returns `ExitStatus::from_raw(0)`: a **raw wait status of `0`**
decodes as `WIFEXITED` with exit code `0`, so `.code()` is `Some(0)`. Test
`status_from_reap_error_synthesizes_success_on_echild` pins this invariant.

## Tests

Guard tests live in `tool_executor.rs` under `#[cfg(test)]`. They exercise the
errno mapping deterministically — they do **not** rely on the environment's
`SIGCHLD` reaping race, which would be flaky:

1. **`status_from_reap_error_synthesizes_success_on_echild`** — feed the helper an
   `io::Error::from_raw_os_error(libc::ECHILD)` and assert it returns
   `Ok(status)` with `status.code() == Some(0)`.
2. **`status_from_reap_error_preserves_other_errors`** — feed the helper a
   non-`ECHILD` errno (e.g. `EPERM`) and assert it returns
   `Err(ClientError::Unknown(_))`, pinning the errno match so it can never be
   over-broadened.
3. **`execute_tool_locally_bash_missing_command_runs_empty_string`** — the
   previously-failing test: a missing command runs `sh -c ""` and now reliably
   yields a result with an `exit_code`, whether or not the child was externally
   reaped.

Acceptance: `cargo test --lib` (esp. `base_type_rustyclawd::tool_executor::tests`)
is green, the previously-failing unit test passes, and the `unit-test`
deploy-gate goes green so self-deploy no longer red-canaries.

## Invariants

The implementation upholds the following invariants:

- **`ECHILD` → success, always logged.** An externally-reaped child yields
  `exit_code: 0` with the already-collected output, and every synthesis emits a
  `tracing::warn!`. Nothing is silenced.
- **Exact errno match.** Only `Some(libc::ECHILD)` is tolerated; every other
  errno remains `ClientError::Unknown("process error: …")`, byte-for-byte as
  before.
- **Real statuses untouched.** When the executor reaps the child itself, the real
  `exit_code` (including non-zero) is returned unchanged — the tolerance path is
  never taken.
- **Idle-liveness unchanged.** The `kill_process_group` reap of a genuinely hung
  child and its `ClientError::Timeout` are unaffected.
- **Success JSON contract unchanged.** `stdout` / `stderr` / `exit_code` shape is
  identical, so the RustyClawd execution path needs no changes.
- **No new configuration.** The behavior is unconditional; no env var, flag, or
  config key is added.

## Related reading

- [RustyClawd Bash-tool idle-liveness](./rustyclawd-bash-tool-idle-liveness.md) —
  the sibling reaping mechanism in the same `Bash` arm; idle-liveness governs
  *when* a hung child is killed, while this doc governs *how a benign
  externally-reaped child is tolerated* on the reap.
- [Base type adapters](./base-type-adapters.md) — where the `rusty-clawd`
  adapter and its Bash tool sit in the adapter hierarchy.
