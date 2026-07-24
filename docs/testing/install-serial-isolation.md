---
title: serial(install) test isolation — the install fork/exec + flock race
description: >
  How the install-module unit tests were made deterministic under parallel
  `cargo test --lib`. The two named flakes behind the self-deploy exit-101
  red-canary crash-loop (issue #4536) —
  `install::paths::tests::install_lock_is_exclusive_per_simard_home` and
  `install::entrypoint::unix::tests::reconcile_replaces_ours_marker_at_entrypoint`
  — pass in isolation but race intermittently under `--test-threads>1` because
  the install tests concurrently `fork`+`exec` short-lived binaries
  (`Command::new(path).arg("--version")`) and take the install `flock`. The fix
  shares one dedicated `install` serial key across every install unit test that
  spawns a child process or takes the flock, so those TOCTOU windows never
  overlap. It is a test-only, additive change: no production API, no spinner,
  and no weakened assertions.
last_updated: 2026-07-24
review_schedule: when a new install unit test spawns a child process or takes the install flock, or when serial_test is upgraded
owner: simard
doc_type: reference
related:
  - ./deflaking-known-flaky-tests.md
  - ./cognitive-memory-serial-isolation.md
  - ./hermetic-tests.md
  - ./ci-resilient-test-patterns.md
  - ./COVERAGE_BASELINE.md
---

# serial(install) test isolation — the install fork/exec + flock race

This page documents the finished state of the work that made the
`src/install/*` unit tests deterministic under parallel `cargo test --lib`. It
is the test-author and reviewer contract for the dedicated `install` serial
group, and the verification gate that keeps it closed.

It was introduced to close issue
[#4536](https://github.com/rysweet/Simard/issues/4536) — the self-deploy
**exit-101 unit-test red-canary crash-loop** that refused ~9 self-deploys over
6+ hours (22:50–04:08 UTC), stranding the running binary at commit `7d0964ff`
(6 commits behind) because the deploy-gate `cargo test --lib` canary flaked red.

> **Root-cause correction.** An earlier remediation mis-attributed the crash to
> `src/meeting_repl/spinner.rs`'s `Drop` test, because the journal's truncated
> failure line (`⠋ Drop test`, printed on the spinner's non-TTY stderr) *looked*
> like the failing test. The spinner tests actually **pass**. The real,
> previously-**unowned** root cause is two install-module tests racing under
> parallelism. Merged PR #4536 and open PRs #4528 / #4529 / #4533 all target the
> spinner and do **not** touch the install tests. **No spinner file is changed by
> this work.**

## TL;DR

- `cargo test --lib` runs the install tests **concurrently in one process**
  (`--test-threads` defaults to the core count). Several install tests
  `fork`+`exec` a freshly-written script via
  `Command::new(path).arg("--version").output()` (the `classify` →
  `version_banner_is_ours` path in `entrypoint.rs`), and one takes the per-`SIMARD_HOME`
  install lock via `flock(LOCK_EX | LOCK_NB)` (`acquire_install_lock` in
  `paths.rs`).
- Running these write-then-exec and flock sequences concurrently opens a
  process-global TOCTOU window (`ETXTBSY` on a just-written-still-open exec
  target, and classification flips) that flips a test red **only sometimes** —
  invisible when a test runs alone. Reproduced deterministically:
  `cargo test --lib install::` twice gave `run1=ok`, `run2=1 failed`.
- The fix is one rule: **every install unit test that spawns a child process or
  takes the install flock shares the dedicated serial key `install`**
  (`#[serial_test::serial(install)]`), so no two of them ever run concurrently.
- A **dedicated** key (not the global `cognitive_memory` key) is used because
  these tests mutate **no** process-global env — they already inject
  `SIMARD_HOME`/layout explicitly through local `layout()` / `layout_for()`
  helpers over `TempDir`. Gating them on `cognitive_memory` would be over-broad
  and needlessly serialize them against unrelated env tests. The `install` key
  serializes install tests **among themselves** while staying parallel with the
  rest of the suite.
- **Gate:** `cargo test --lib install::` ≥**5 consecutive** clean runs **and** a
  green full `cargo test --lib`.

---

## The race this eliminates

The install tests are already hermetic on state: each builds an `InstallLayout`
rooted in its own `TempDir` (`layout(&temp…)` in `paths.rs`,
`layout_for(temp.path())` in `entrypoint.rs`) and never reads or writes a
process-global env var. The race is **not** an env race. It is a concurrency
window on two process-global OS resources:

1. **`fork`+`exec` of a just-written script (`ETXTBSY` / classification flip).**
   `entrypoint.rs::version_banner_is_ours` (reached from `classify` when the
   candidate is a regular file) runs the candidate binary to check its version:

   ```rust
   // src/install/entrypoint.rs::version_banner_is_ours
   let output = match Command::new(path).arg("--version").output() { … };
   ```

   The tests write these fake binaries with `write_exec(...)` immediately before
   `reconcile`/`classify` exec them. When several install tests do this at once,
   one thread's `execve` can observe another thread's still-open writable file
   descriptor and fail with `ETXTBSY` (or an in-flight write can change what
   `classify` reads), flipping a `Foreign`/`Ours` decision. This is the window
   behind `reconcile_replaces_ours_marker_at_entrypoint`
   (`entrypoint.rs:365`), whose `assert!(meta.file_type().is_symlink(), …)`
   fails when reconcile misclassifies the entrypoint under contention.

2. **The install `flock`.** `paths.rs::acquire_install_lock` takes an exclusive,
   non-blocking advisory lock:

   ```rust
   // src/install/paths.rs
   let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
   ```

   `install_lock_is_exclusive_per_simard_home` (`paths.rs:326`) asserts the lock
   is re-acquirable after the first guard drops
   (`.expect("lock should be available after guard drop")`). Concurrent install
   tests that also `fork`+`exec`/lock share the same process-global exposure and
   can perturb the drop→re-acquire timing.

Both windows are **var-agnostic and injection-proof** — threading more state
into the tests would not close them, because the shared resource is the OS
`fork`/`exec`/`flock` surface of the *process*, not any value the tests read.
The only correct fix is to stop the install tests from running concurrently
with each other.

## The rule

**Every unit test in the install module that spawns a child process or acquires
the install flock carries `#[serial_test::serial(install)]`.**

This includes, but is not limited to, the two named flakes. To make ≥5× runs
deterministic the **entire group** of `Command`-spawning / flock-taking install
tests is serialized, not just the two — any sibling reconcile/classify/lock test
hitting the same window can re-flake otherwise.

### What the finished tests look like

`src/install/entrypoint.rs` — the **entire** `mod unix::tests` group carries the
key. **Four** of its eight tests drive `reconcile`/`classify` down the
`version_banner_is_ours` → `Command::new(path).arg("--version")` fork/exec path,
because they place a **regular file** (written with `write_exec`) at the
entrypoint or an orphan path, and `classify` runs it to read its `--version`
banner (`entrypoint.rs:152–157`, `165`):
`reconcile_removes_ours_marker_orphan`, `reconcile_preserves_foreign_orphan`,
`reconcile_replaces_ours_marker_at_entrypoint` (named flake #1), and
`reconcile_surfaces_foreign_shadow_at_entrypoint_untouched`.

The other **four** never `exec` — `classify` short-circuits *before*
`version_banner_is_ours` when the candidate is **absent**
(`reconcile_creates_owned_symlink_on_fresh_home`, `reconcile_is_idempotent`), an
**owned symlink** into the bin dir (`reconcile_removes_ours_symlink_orphan`), or a
**dangling symlink** that fails `canonicalize` at `entrypoint.rs:150`
(`classify_broken_symlink_is_foreign`). They are serialized with the group
**anyway**. Serializing at the whole-`mod unix::tests` boundary (rather than
cherry-picking only the four spawning members) is deliberate defense-in-depth: it
keeps every member from running concurrently with a spawning sibling, and keeps
the serial-key convention uniform across the whole group. This is a maintainer
convention backed by an **allowlist** guard, not an automatic guarantee: the
regression guard enforces the key only on the forking install tests it already
knows about, so a newly-added test that omits the key still runs in parallel and
could re-open the window until it is added (see
[Adding a new install test](#adding-a-new-install-test)):

```rust
// src/install/entrypoint.rs — mod unix::tests
#[test]
#[serial_test::serial(install)]
fn reconcile_creates_owned_symlink_on_fresh_home() { … }   // no exec: entrypoint Absent

#[test]
#[serial_test::serial(install)]
fn reconcile_removes_ours_symlink_orphan() { … }           // no exec: OursSymlink

#[test]
#[serial_test::serial(install)]
fn reconcile_removes_ours_marker_orphan() { … }            // forks: regular-file --version

#[test]
#[serial_test::serial(install)]
fn reconcile_preserves_foreign_orphan() { … }              // forks: regular-file --version

#[test]
#[serial_test::serial(install)]
fn reconcile_replaces_ours_marker_at_entrypoint() { … }    // forks — named flake #1

#[test]
#[serial_test::serial(install)]
fn reconcile_surfaces_foreign_shadow_at_entrypoint_untouched() { … }  // forks: regular-file --version

#[test]
#[serial_test::serial(install)]
fn reconcile_is_idempotent() { … }                         // no exec: Absent then OursSymlink

#[test]
#[serial_test::serial(install)]
fn classify_broken_symlink_is_foreign() { … }              // no exec: dangling symlink → Foreign
```

`src/install/paths.rs` — the flock test carries the same key, below its
`#[cfg(unix)]` / `#[test]` attributes:

```rust
// src/install/paths.rs — mod tests
#[cfg(unix)]
#[test]
#[serial_test::serial(install)]
fn install_lock_is_exclusive_per_simard_home() { … }      // named flake #2
```

The attribute is written **fully-qualified** (`serial_test::serial(install)`),
so no `use serial_test::serial;` import is added and no other test module is
touched. Assertions are unchanged; nothing is `#[ignore]`d.

### Adding a new install test

If you add a unit test to `entrypoint.rs`'s `mod unix::tests`, carry
`#[serial_test::serial(install)]` to match the rest of that group — the whole
module is serialized as a unit (see above), so keep it uniform even if your
specific test doesn't itself `fork`+`exec`. If you add an install test in
**another** module (`paths.rs`, a future file) that **spawns a child process**
(any `Command::new(...)`, directly or via `reconcile`/`classify`) **or acquires
the install flock** (`acquire_install_lock`), add the key below its `#[test]`.
Pure temp-dir / filesystem tests in other modules that never `fork`+`exec` and
never lock (e.g. the hermetic path-construction tests in `binary.rs`) do not
need the key and should stay parallel.

---

## Why a dedicated `install` key (not `cognitive_memory`)

The whole-binary [`cognitive_memory` serial
contract](./cognitive-memory-serial-isolation.md) exists to stop
process-global **env** reads from racing env **writes**, and its `serial_guard`
meta-test *requires* that key on any test that mutates a watched env var. The
install tests mutate **no** env var, so:

- The `serial_guard` meta-test does **not** demand a key here — it only requires
  keys for env-mutators. Adding a key is always permitted, so the guard stays
  green.
- Reusing `cognitive_memory` would over-serialize: the install tests would
  block on the global env lock and slow every unrelated env test for no
  isolation benefit.

The dedicated `install` key gives exactly the isolation needed — install tests
serialized against **each other** — while leaving them parallel to the rest of
`cargo test --lib`.

---

## Verification gate

A change here is merge-ready only when all of the following pass.

### Targeted stress (the de-flake proof)

Run the whole install test group ≥**5 consecutive** times with **zero**
failures. Run the **group** (`install::`), never a single-test filter — the race
only surfaces when the install tests run concurrently with each other, so a
name-filtered run drops the contending siblings and proves nothing:

```bash
for i in $(seq 1 5); do
  cargo test --lib install:: \
    || { echo "FLAKE on run $i"; break; }
done
```

Before the fix, this loop flips red intermittently (`run1=ok`, `run2=1 failed`
was the reproducer). After it, all 5 runs are green.

### Full suite

```bash
cargo test --lib
```

Must pass with no regressions, including the
[`serial_guard` meta-test](./cognitive-memory-serial-isolation.md) — which stays
green because no watched env var is mutated by this change.

### Build hygiene

```bash
cargo build
```

Must compile **warning-free**. The change adds no imports (the attribute is
fully qualified) and introduces no `print!`/`println!` — the install path uses
structured `tracing` + OTel only, and there are no silent fallbacks.

### CI / deploy gate

The self-deploy deploy-gate runs `cargo test --lib` as its unit-test canary.
With the install group serialized, the canary is deterministic and the exit-101
crash-loop cannot recur from these tests. **No CI-workflow edits** are part of
this work.

---

## Configuration & environment

This change adds **no** new runtime configuration and **no** new dependency:
`serial_test = "=3.4.0"` is already exact-pinned in `Cargo.toml` and the pin is
preserved.

### Local stress-run memory budget

The saved workstation preference for memory-heavy local runs is
`NODE_OPTIONS=--max-old-space-size=32768` (used by the tooling that drives the
stress loops, not by `cargo` itself). To change it, edit `~/.amplihack/config`.
It is not required for CI and does not affect the Rust tests.

---

## Scope

**In scope:** adding `#[serial_test::serial(install)]` to the whole
`entrypoint.rs` `mod unix::tests` group (the eight tests at
`entrypoint.rs:295–434` — four of which `fork`+`exec`, including named flake #1
`reconcile_replaces_ours_marker_at_entrypoint` at `entrypoint.rs:365`, plus four
non-spawning siblings serialized for defense-in-depth) and the `paths.rs` flock
test (named flake #2 `install_lock_is_exclusive_per_simard_home` at
`paths.rs:326`) — plus this page. All changes live under `src/install/*.rs` test
modules; `git diff` touches only install files.

**Out of scope (confirmed):**

- `src/meeting_repl/spinner.rs`, the spinner tests, and PRs
  #4528 / #4529 / #4533 / #4536 — the spinner is a red herring and its tests
  pass.
- Any production code or public API change: assertions are preserved (including
  the `LOCK_EX | LOCK_NB` flock semantics — no substitution of a blocking lock
  to mask flakiness), no test is `#[ignore]`d, and the exercised code paths are
  unchanged.
- `binary.rs` (hermetic, temp-dir-only tests with no `fork`+`exec`) and
  `systemd.rs` (uses `Command` in production paths but has no unit tests) — no
  key needed.

## Related pages

- [De-flaking the known flaky tests](./deflaking-known-flaky-tests.md) — the
  prompt-delivery env race and goal-board state-root race, the companion
  parallel-`cargo test` de-flakes.
- [De-flaking the local canary deploy-gate under CPU oversubscription](./deploy-gate-oversubscription-deflake.md)
  — confirms the `install_lock_is_exclusive_per_simard_home` `flock` EAGAIN
  fork-inheritance flake (#4559) stays closed under oversubscription via this
  page's `serial(install)` key (no new product code), alongside the dispatch and
  terminal-sleep timing fixes (#4560).
- [serial(cognitive_memory) test isolation](./cognitive-memory-serial-isolation.md)
  — the whole-binary watched-env contract and the `serial_guard` meta-test that
  this dedicated `install` key deliberately stays orthogonal to.
- [Writing hermetic tests against cognitive memory](./hermetic-tests.md) — how a
  single test allocates isolated state (the install tests apply the same
  temp-dir discipline to `InstallLayout`).
- [CI-resilient test patterns](./ci-resilient-test-patterns.md) — companion
  patterns for deterministic tests under parallelism.
