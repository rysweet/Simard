---
title: Tool-executor Bash reaping under SIGCHLD=SIG_IGN (ECHILD tolerance)
description: >
  How the RustyClawd Bash tool-executor stays deterministic when the host
  process ignores SIGCHLD (`SIGCHLD=SIG_IGN`) and the kernel auto-reaps the
  child. `try_wait()`/`wait()` then return `ECHILD`; the executor treats that
  as a successful (exit 0) completion instead of panicking, which closed the
  deploy-gate canary's intermittent `cargo test` exit-101 (issue #4506). Also
  documents the deterministic, global-state-free regression test that pins
  the behaviour.
last_updated: 2026-07-23
review_schedule: when the Bash reaping path changes, when serial_test is upgraded, or when a new signal-disposition test is added
owner: simard
doc_type: reference
related:
  - ./deflaking-known-flaky-tests.md
  - ./cognitive-memory-serial-isolation.md
  - ./ci-resilient-test-patterns.md
  - ./hermetic-tests.md
  - ../reference/rustyclawd-bash-tool-idle-liveness.md
---

# Tool-executor Bash reaping under `SIGCHLD=SIG_IGN` (ECHILD tolerance)

> **Status: implemented (issue #4506).** This reference describes the shipped
> behavior of `execute_tool_locally`'s Bash arm in
> `src/base_type_rustyclawd/tool_executor.rs`. When the host process disposition
> for `SIGCHLD` is `SIG_IGN`, the kernel auto-reaps exited children and the
> executor's `try_wait()`/`wait()` return `ECHILD`. The executor now treats that
> specific errno as a **successful completion** (exit `0`) rather than surfacing
> an error that panics the test binary. Every section below describes current
> behavior — this is a **production-path** change, not a test-only one.

## TL;DR

> - **Symptom:** the deploy-gate canary went red for hours because
>   `cargo test --manifest-path Cargo.toml --target-dir <dir>` intermittently
>   exited **101** (a Rust panic), keeping the running binary commits behind
>   merged `main`. Required CI (`verify.yml`, `--all-features --locked
>   --no-fail-fast`) stayed green because it did not hit the same disposition.
> - **Root cause:** under `SIGCHLD=SIG_IGN` the kernel reaps the Bash child
>   before the executor can, so `try_wait()`/`wait()` fail with `ECHILD`
>   ("No child processes"). The old code mapped that error to
>   `ClientError::Unknown`; the awaiting test then unwrapped that `Err`, turning
>   it into a panic → cargo exit 101. (The executor returns `Err`; the panic
>   comes from the caller unwrapping it.)
> - **Fix:** match `ECHILD` **strictly by errno** (`raw_os_error() ==
>   Some(libc::ECHILD)`), log a structured `tracing::warn!`, and synthesize a
>   success `ExitStatus` (`ExitStatusExt::from_raw(0)`). The child *did* exit;
>   it was simply reaped where we couldn't observe its status.
> - **Scope:** additive and bounded to `tool_executor.rs`. The ECHILD branch is
>   only reachable under `SIG_IGN`; every other path — including genuine
>   non-zero exits and all other errors — is byte-for-byte unchanged
>   (fail-closed).
> - **Regression test:**
>   `echild_from_reap_is_treated_as_success_but_other_errno_is_not_4506`
>   asserts the fix's decision logic directly and deterministically: `ECHILD`
>   ⇒ already-reaped (success), every other errno ⇒ still an error
>   (fail-closed), and the synthesized `ExitStatus` decodes to exit code 0. It
>   deliberately does **not** install `SIGCHLD=SIG_IGN`, because that
>   disposition is process-global and would let the auto-reap window mask
>   genuine non-zero exits in the ~100+ sibling tests that spawn subprocesses
>   (`serial_test` cannot contain a process-global signal mutation — it only
>   serializes other tagged tests). The test needs no serial key and cannot
>   flake.
> - **Gate:** the deploy-gate canary passes deterministically (exit 0, no 101)
>   across repeated runs, and `cargo test --all-features` stays green.

---

## Why `exit 101`, and why it is *not* the glibc env race

`cargo test` exits **101** when a test **panics** — that is cargo's standard
panic exit code. This single fact rules out the competing hypothesis that the
failure came from the glibc `getenv`/`setenv` data race documented in
[`serial(cognitive_memory)` isolation](./cognitive-memory-serial-isolation.md):
an env-var data race manifests as `SIGSEGV` (exit **139**) or `abort()`
(exit **134**), not 101. A truncated `"... Drop t..."` fragment in an early
handoff was a **200-char stderr truncation artifact** from the deploy gate's
log capture (`src/self_relaunch/gates.rs`), not evidence of a failing `Drop`
or teardown test.

The real signature is a panic propagated out of the Bash tool call. That panic
originated in the executor's child-reaping path when `SIGCHLD` was ignored.

## Background: `SIGCHLD=SIG_IGN` and `ECHILD`

On Linux, a process's disposition for `SIGCHLD` controls who reaps children:

- **Default disposition:** the parent is responsible for reaping. `wait()` /
  `waitpid()` (and Tokio's `Child::try_wait`/`Child::wait`, which wrap them)
  return the child's exit status.
- **`SIGCHLD` set to `SIG_IGN`:** POSIX specifies the kernel **auto-reaps**
  exited children and does **not** leave zombies. A subsequent `wait()` then
  fails with **`ECHILD`** ("No child processes") because there is no child left
  to reap.

The RustyClawd tool-executor spawns the Bash child, streams its stdout/stderr
through an idle-liveness loop (see
[RustyClawd Bash-tool idle-liveness](../reference/rustyclawd-bash-tool-idle-liveness.md)),
and finally reaps the child to read its exit code. If some component in the
host process has installed `SIG_IGN` for `SIGCHLD`, the kernel wins the race:
the child is gone before the executor calls `try_wait()`/`wait()`, and those
calls return `ECHILD`.

**`ECHILD` here means "the child completed and was reaped elsewhere," not
"the command failed."** Treating it as a hard error was the bug.

## The rule (issue #4506)

> When reaping a Bash tool child, an `ECHILD` result from `try_wait()` or
> `wait()` must be treated as a **successful, already-reaped completion**
> (exit status `0`), because under `SIGCHLD=SIG_IGN` the kernel reaps the child
> before the executor can observe its status. Every non-`ECHILD` error remains
> a hard failure (fail-closed). The decision is made **solely** from the syscall
> errno — never from the command's stdout/stderr.

## Shipped behavior

The Bash arm of `execute_tool_locally` reaps the child in two places, and both
tolerate `ECHILD`:

1. **Streams-closed poll (`try_wait()` loop).** After both pipes close, the
   executor polls `child.try_wait()` until the child is reaped. A `Err(e)`
   where `is_child_already_reaped(&e)` is true resolves the loop with a
   synthesized success status instead of returning `ClientError::Unknown`.
2. **Final reap (`wait()`).** A defensive final reap: on the normal non-hung
   path the loop always breaks via `try_wait()` returning `Ok(Some(status))`, so
   `exit_status` is already set and this `wait()` is not reached. It is retained
   for safety, and its `ECHILD` error is likewise mapped to the synthesized
   success status so the fallback stays consistent with Site A.

Both sites emit exactly one structured log line and then continue:

```text
WARN bash child not observable via try_wait/wait: SIGCHLD=SIG_IGN (ECHILD);
     treating as successful completion  error=No child processes (os error 10)
```

The returned tool result is the normal JSON envelope with the captured output
and `exit_code: 0`:

```json
{
  "stdout": "…captured stdout…",
  "stderr": "…captured stderr…",
  "exit_code": 0
}
```

### Errno matching (strict, never string matching)

The decision is made only by comparing the OS error number to `libc::ECHILD`:

```rust
// Conceptual — the shipped helper lives in tool_executor.rs.
fn is_child_already_reaped(err: &std::io::Error) -> bool {
    err.raw_os_error() == Some(libc::ECHILD)
}
```

The success status is synthesized only via the Unix `ExitStatusExt` extension:

```rust
use std::os::unix::process::ExitStatusExt;

fn reaped_success_status() -> std::process::ExitStatus {
    std::process::ExitStatus::from_raw(0)
}
```

`raw_os_error()` compares an integer errno; it never inspects locale-dependent
message text, so it cannot be spoofed by crafted stderr. The synthesized status
is always exactly `0` — the code never fabricates an arbitrary or
attacker-influenced exit code.

## What is *not* changed

- **Genuine non-zero exits.** When the executor *can* observe the child
  (default disposition), the real exit code flows through unchanged. A command
  that exits `1` still reports `exit_code: 1`.
- **All other errors.** Any `try_wait()`/`wait()` error that is **not** `ECHILD`
  still returns `ClientError::Unknown("process error: …")`. The path is
  fail-closed for real failures.
- **Idle-liveness reaping.** The hung-child kill path
  (`ClientError::Timeout`, `SIMARD_RUSTYCLAWD_IDLE_LIVENESS_SECS`) is untouched;
  see [RustyClawd Bash-tool idle-liveness](../reference/rustyclawd-bash-tool-idle-liveness.md).
- **The deploy gate.** `src/self_relaunch/gates.rs` is a pure consumer of the
  canary result and is unchanged. Its contract is confirmed below.
- **The three existing Bash smoke tests.** No serial attribute was added to
  them; the env race is not the cause of exit 101, so that change is out of
  scope.

## The regression test

`echild_from_reap_is_treated_as_success_but_other_errno_is_not_4506`
deterministically pins the fix's decision logic without touching process-global
state:

- **`ECHILD` ⇒ success:** `is_child_already_reaped` returns `true` for an
  `io::Error` carrying `ECHILD` — the auto-reap signature the reap sites must
  tolerate.
- **Every other errno ⇒ error (fail-closed):** `EPERM`, `EINVAL`, `EIO`,
  `ENOMEM`, and a non-OS error all return `false`, proving genuine failures are
  never masked.
- **Synthesized status:** `reaped_success_status()` reports `success()` and
  decodes to `code() == Some(0)`, so downstream reporting yields `exit_code: 0`.

### Why not an end-to-end `SIG_IGN` reproduction?

An earlier iteration installed `SIGCHLD=SIG_IGN` process-wide, ran a real `echo`
tool call, and restored the disposition — grouped under
`#[serial_test::serial(cognitive_memory)]`. **That approach was rejected: it is a
suite-wide flake source.** The `SIGCHLD` disposition is process-global, so during
the `SIG_IGN` window *any* concurrently running test that spawns and reaps a
child would see `ECHILD` and — post-fix — have its genuine non-zero exit silently
masked as success. This was reproduced directly: with the end-to-end test present,
this module's own `execute_tool_locally_bash_failing_command_has_nonzero_exit`
intermittently observed `exit_code == 0`. `serial_test` **cannot** contain this:
its keys only serialize *other tagged* tests, and ~100+ tests across the crate
spawn subprocesses without that tag. Serializing them all would be fragile and
incomplete. The helper-level test above validates the exact same contract
(`ECHILD` ⇒ success, other errno ⇒ error) with zero global state, no serial key,
and no possibility of flake.

```rust
#[test]
fn echild_from_reap_is_treated_as_success_but_other_errno_is_not_4506() { /* … */ }
```

## Verification gate

The canary that self-deploy consults invokes the whole default test set — not
`--lib`, not `--all-features`:

```bash
# What src/self_relaunch/gates.rs::run_unit_test_gate runs (default features):
cargo test --manifest-path <manifest_dir>/Cargo.toml --target-dir <canary_target_dir>
```

Any non-zero exit ⇒ red canary ⇒ self-deploy aborts (fail-closed). The fix is
accepted when:

1. The deploy-gate invocation above exits `0` (no 101), run repeatedly →
   consistently green.
2. The required-CI invocation stays green:

   ```bash
   cargo test --all-features --locked --no-fail-fast
   ```

3. `echild_from_reap_is_treated_as_success_but_other_errno_is_not_4506` passes
   deterministically across repeated runs (it pins the `ECHILD` ⇒ success and
   other-errno ⇒ error contract directly, without mutating global signal state).

## Constraints honored

- **Additive / non-breaking.** The `ECHILD` branch is only reachable under
  `SIGCHLD=SIG_IGN`; normal execution is unchanged.
- **PRD preserved**; no Bridge naming introduced.
- **No stray `print!`/`println!`.** Diagnostics use `tracing::warn!` with a
  structured `error` field plus a static message — never the command's args,
  env, stdout, or stderr (which may carry secrets) — consistent with the OTel
  logging posture.
- **Unix-only constructs** (`ExitStatusExt::from_raw`, `libc::ECHILD`) stay
  within the crate's existing `cfg(unix)` posture; the daemon targets Unix.

## Out of scope

- amplihack-rs issue #978 (default-workflow Step 15 rebase).
- Host `/tmp` disk pressure (an operational capacity escalation, not a code fix).
- Adding `#[serial_test::serial(...)]` to the three existing Bash smoke tests.
- Any refactor beyond `tool_executor.rs` and this documentation set.
