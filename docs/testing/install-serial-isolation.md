---
title: install test isolation — superseded by the ETXTBSY-retry root-cause fix
description: >
  The install-module unit tests were briefly serialized with a dedicated
  `#[serial(install)]` key (issue #4536) to dodge a fork/exec race. That
  serialization has been REMOVED (issue #4558): the underlying transient
  `ETXTBSY` race is now fixed at its root by a bounded retry in
  `version_banner_is_ours`, so the install tests are deterministic while running
  fully in parallel. This page is kept as a durable record of that evolution.
last_updated: 2026-07-25
review_schedule: when a new install unit test spawns a child process, or when the ETXTBSY-retry in src/install/entrypoint.rs changes
owner: simard
doc_type: reference
related:
  - ./deflaking-known-flaky-tests.md
  - ./cognitive-memory-serial-isolation.md
  - ../reference/deterministic-canary-unit-test-gate.md
  - ./hermetic-tests.md
  - ./ci-resilient-test-patterns.md
---

# install test isolation — the fork/exec `ETXTBSY` race (superseded)

> **Status: superseded.** The `#[serial(install)]` group and its
> `serial_isolation_guard` meta-test described by earlier revisions of this page
> have been **removed**. The race they papered over is now fixed at its root; the
> install tests run in parallel and deterministically. See
> [the deterministic canary gate reference, §C5](../reference/deterministic-canary-unit-test-gate.md)
> for the current design.

## The race (still accurate)

The install unit tests in `src/install/entrypoint.rs` and `src/install/paths.rs`
run concurrently in one `cargo test --lib` process. Classifying an on-disk
`simard` candidate means **`exec`ing it** — `version_banner_is_ours` runs
`<path> --version` to read its banner. In a multithreaded process a *sibling*
thread's `fork` can transiently inherit a write file descriptor to that same
file and hold it open across the `exec`, so the kernel rejects the `exec` with
**`ETXTBSY`** ("text file busy", `os error 26`). The classifier then mis-reads
the candidate as `Foreign`, flipping
`reconcile_replaces_ours_marker_at_entrypoint` red. This was one of the flakes
behind the self-deploy exit-101 red-canary crash-loop
([#4536](https://github.com/rysweet/Simard/issues/4536)).

Ground-truthing (#4558) confirmed the race is **real, not a test artifact**:
running every `install::` test at `--test-threads=14` under 64-way CPU load
reproduced the flip at ~1 in 150 runs. Temp-rooting each test under a unique
`TempDir` does **not** close the window — the shared resource is the process
fork/exec fd table, not the filesystem.

## The fix (current)

The interim fix serialized every child-spawning / `flock` install test behind a
dedicated `install` serial key so their `fork`+`exec` windows never overlapped.
That was a **band-aid**: correct, but it hid a real race behind reduced
concurrency.

The root-cause fix retries the classification `exec` on the transient condition,
mirroring the established `retry_on_etxtbsy` pattern from
[Fix #4523](./deflaking-known-flaky-tests.md):

- `version_banner_is_ours` retries `Command::new(path).arg("--version").output()`
  up to 8 times, **only** when `err.raw_os_error() == Some(libc::ETXTBSY)`, with a
  short backoff between attempts.
- Classification is numeric (`libc::ETXTBSY` == 26), never string-based, so it is
  locale-independent.
- Any other error, or a persistent `ETXTBSY`, still classifies **fail-closed** as
  not-ours — the conservative "never clobber a foreign file" contract is
  unchanged.
- This also hardens **real installs**: a concurrent writer of the entrypoint no
  longer flips the reconciler's verdict.

With the transient converted into the correct verdict, `#[serial(install)]` and
`src/install/serial_isolation_guard.rs` were removed. Verification: all
`install::` tests, `--test-threads=14`, 200 iterations under 64-way CPU load →
**200/200 green** (vs. the pre-fix reproduction that reddened at iteration 47).

The sibling `flock` exclusivity test
(`install::paths::tests::install_lock_is_exclusive_per_simard_home`) did not
reproduce under the same load once the `ETXTBSY` classification race was fixed:
Rust opens the lock file `O_CLOEXEC` by default, so a sibling's forked child
drops the inherited lock fd at its own `exec`.
