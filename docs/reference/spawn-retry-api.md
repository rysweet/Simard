---
title: Spawn-retry API
description: >
  The util::spawn_retry helper — bounded-backoff retry for transient fork/exec
  failures (ETXTBSY, EAGAIN/EWOULDBLOCK, ENOMEM) that Simard hits when spawning
  gh and agent subprocesses under high host concurrency. Sync and async variants
  over one shared errno classifier.
last_updated: 2026-07-25
review_schedule: when a new real-subprocess spawn site is added
owner: simard
doc_type: reference
related:
  - ./durable-append-api.md
  - ../testing/resource-isolated-test-suite.md
  - ./recipe-context-file-transport.md
  - ./large-payload-spawn-api.md
---

# Spawn-retry API

`src/util/spawn_retry.rs` hardens every real-subprocess launch in Simard against
transient operating-system fork/exec failures. Under high host concurrency the
kernel intermittently rejects a spawn with a **transient** errno even though the
same call succeeds moments later; `spawn_retry` retries those (and only those)
with a bounded, capped backoff.

```rust
pub fn is_transient_spawn_error(e: &std::io::Error) -> bool;

pub fn retry_spawn_sync<T>(
    f: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T>;

pub async fn retry_spawn_async<T, Fut>(
    f: impl FnMut() -> Fut,
) -> std::io::Result<T>
where
    Fut: std::future::Future<Output = std::io::Result<T>>;
```

## Why this exists

Simard spawns `gh` constantly (issue reporting, issue listing) and spawns agent
subprocesses through `spawn_with_isolation` on the OODA hot path. Under
fork/exec load these fail transiently (issue
[#4577](https://github.com/rysweet/Simard/issues/4577)):

- **`ETXTBSY` (26, "Text file busy")** — observed spawning `gh` while its
  on-disk image is concurrently written/executed by many processes.
- **`EAGAIN` / `EWOULDBLOCK` (11)** — `fork`/`clone` hits the per-user process
  or memory limit momentarily.
- **`ENOMEM` (12)** — transient allocation failure during `fork`.

Before this helper, those errno values propagated straight to an
`.expect("tool execution should succeed")` or a bare `.spawn()?` and reddened
the self-deploy canary (exit 101), freezing self-deploy. They are not logic
errors — they are load artifacts that clear on a brief retry.

## Contract

### `is_transient_spawn_error(e)`

Returns `true` iff `e.raw_os_error()` is one of `{26, 11, 12}` (ETXTBSY,
EAGAIN/EWOULDBLOCK, ENOMEM). Classification is by **raw errno**, never by
matching the error message string — message text is locale- and
platform-dependent and must not gate retry logic. Every other error
(including `ENOENT` "binary not found", permission denied, and any non-OS
error) returns `false` and is treated as permanent.

### `retry_spawn_sync(f)` / `retry_spawn_async(f)`

- Calls the closure `f`, which **rebuilds and launches** the subprocess on each
  attempt. The closure factory pattern is required because `std::process::Command`
  and `tokio::process::Command` are not `Clone` — each attempt constructs a fresh
  `Command`.
- On `Ok(T)`, returns immediately.
- On `Err(e)` where `is_transient_spawn_error(e)` is `true`, sleeps a short,
  capped backoff and retries, up to a small **bounded** attempt count.
- On `Err(e)` where the error is **not** transient, returns `Err(e)` immediately
  — no retry, no masking. A genuine failure (missing binary, bad permissions)
  surfaces on the first attempt.
- On exhausting the bounded attempt budget, returns the **last** `Err` so the
  caller sees the real transient errno rather than a synthetic error.

Both variants share the exact same classifier (`is_transient_spawn_error`) and
the same bounded-attempt / capped-backoff policy; only the await/sleep mechanism
differs (blocking sleep vs `tokio` sleep).

### Boundaries

- Retries apply **only** to the subprocess *spawn/exec* result — never to a
  process that spawned successfully and then exited non-zero, and never to test
  assertions or races. A `gh` command that spawns and returns exit code 1 is a
  real result, returned to the caller unchanged.
- The backoff is bounded and capped. This is the task-sanctioned "bounded
  backoff for genuine transient OS spawn errors," **not** the prohibited
  practice of adding sleeps to loosen test timing.

## Usage

### Synchronous (`std::process::Command`)

```rust
use crate::util::spawn_retry::retry_spawn_sync;

// gh_client.rs — spawn the gh child, rebuilding Command per attempt
let mut child = retry_spawn_sync(|| {
    Command::new("gh")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
})?;

// gh_client.rs — the one-shot `gh issue list` output path
let output = retry_spawn_sync(|| {
    Command::new("gh").args(&list_args).output()
})?;
```

### Asynchronous (`tokio::process::Command` / `spawn_with_isolation`)

```rust
use crate::util::spawn_retry::retry_spawn_async;

// tool_executor.rs — wrap the external spawn_with_isolation helper
let child = retry_spawn_async(|| async {
    spawn_with_isolation(build_config()) // rebuilds config + Command each attempt
})
.await?;
```

> The external `rustyclawd-tools::spawn_with_isolation` is a pinned git
> dependency and is **not** patched. Resilience is added at the Simard call site
> by wrapping it in `retry_spawn_async`.

## Where it is wired

| Call site | Variant | What it launches |
|-----------|---------|------------------|
| `stewardship/gh_client.rs` (`.spawn()`) | `retry_spawn_sync` | `gh issue create` |
| `stewardship/gh_client.rs` (`.output()`) | `retry_spawn_sync` | `gh issue list` |
| `base_type_rustyclawd/tool_executor.rs` | `retry_spawn_async` | isolated Bash-tool child |

Any new site that spawns a real subprocess and needs its result must route
through one of these helpers rather than calling `.spawn()`/`.output()` and
unwrapping directly — see the sweep rule in
[Resource-isolated test suite](../testing/resource-isolated-test-suite.md).

## Targeted zombie reaping

`spawn_retry.rs` also owns Simard's detached-child reaper. Simard launches a
handful of **fire-and-forget** subprocesses (a non-tmux subordinate agent, and
two detached `simard safe-update` dispatches) that no code later `wait()`s on.
Left unreaped they become zombies, so the agent-supervisor lifecycle
periodically reaps them.

The reaper is **targeted**, not a `waitpid(-1)` hammer:

```rust
/// Record a detached child PID that the reaper owns.
pub fn register_reapable_child(pid: i32);

/// Reap ONLY registered PIDs (per-PID `waitpid(pid, WNOHANG)`); returns the
/// count actually reaped. Never touches a PID it did not register.
pub fn reap_registered_children() -> usize;
```

- The registry is a process-global `Mutex<BTreeSet<i32>>`. Each detached spawn
  site calls `register_reapable_child(child.id() as i32)` immediately after
  spawning.
- `reap_registered_children()` does a non-blocking `waitpid(pid, WNOHANG)` for
  each registered PID: a returned pid → reaped (count it, drop it from the set);
  `0` → still alive (keep it); `-1`/`ECHILD` → already gone (drop it, don't
  count).

**Why targeted, not `waitpid(-1)`:** a blanket `waitpid(-1, WNOHANG)` reaps the
FIRST available child of the process — including children that `gh`, `git`, the
Bash tool, or a `tokio` spawn are actively `wait()`ing on. Stealing another
waiter's child makes that waiter's own `wait` fail with `ECHILD`, which surfaced
as intermittent `execute_tool_locally_*` failures under load. Reaping strictly
by registered PID means the reaper can never race a legitimate waiter (issue
[#4577](https://github.com/rysweet/Simard/issues/4577)).

## Testing


Unit tests assert the classifier maps `{26, 11, 12}` → `true` and everything
else (e.g. `ENOENT`, non-OS errors) → `false`; that the retry helper retries a
simulated transient error and then succeeds; and that it gives up after the
bounded budget on a persistent transient error, returning the last `Err`. The
originally-flaky
`gh_client::create_issue_reports_nonzero_exit_and_stderr_without_body_content`
and the `tool_executor` Bash-tool `.expect(...)` tests pass on a saturated host
because the transient spawn errno is now retried rather than propagated.
