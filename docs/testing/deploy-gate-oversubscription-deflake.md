---
title: De-flaking the local canary deploy-gate under CPU oversubscription (flock inheritance + wall-clock timing)
description: >
  How the local canary unit-test deploy-gate was made deterministic when many
  engineers run full-parallel canaries on one host (observed load avg 73/158/125).
  Three flakes are closed: the install-lock `flock` EAGAIN fork-inheritance race
  (#4559), whose real guard is the existing `serial(install)` serialization
  (#4536) plus std's default close-on-exec — verified under oversubscription with
  no new production fd flag; a brittle dispatch wall-clock >=2x speedup assertion
  (#4560a) relaxed to an oversubscription-tolerant bound backed by deterministic
  peak-concurrency checks; and three 1s terminal-sleep deadlines (#4560b) widened
  to 5s. All three roll up to the #4558 umbrella. The fixes are additive and
  test-only: no deploy-gate, PRD, or production timing behaviour changes.
last_updated: 2026-07-24
review_schedule: when a new advisory-lock open site is added, or when the dispatch concurrency / interruptible_sleep tests change
owner: simard
doc_type: reference
status: implemented
related:
  - ./deflaking-known-flaky-tests.md
  - ./install-serial-isolation.md
  - ./ci-resilient-test-patterns.md
  - ./hermetic-tests.md
  - ../reference/canary-gate-convergence.md
  - ../reference/self-deploy-api.md
  - ../reference/overseer-deploy-canary-diagnostics.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../../src/install/paths.rs
  - ../../src/install/serial_isolation_guard.rs
  - ../../src/ooda_actions/tests_dispatch_concurrency.rs
  - ../../src/operator_commands_ooda/daemon/helpers.rs
  - ../../src/operator_commands_ooda/tests/daemon_inline.rs
---

# De-flaking the local canary deploy-gate under CPU oversubscription

This page documents the finished state of the work that made the **local canary
unit-test deploy-gate** deterministic when the host is heavily oversubscribed —
the condition that stalled self-deploy for 6+ hours (~10–21 consecutive red
canaries, unit-test exit status `101`), leaving the running binary 8 commits
behind merged `main` (`DeployDrift`) even though GitHub Actions `verify` on
`main` was green the whole time.

The root cause was never a real regression. It was three tests that assume a
lightly-loaded machine and fail when 7 live engineers run full-parallel canaries
on one box (observed load avg 73/158/125). This is the test-author and reviewer
contract for the three fixes and the verification gate that keeps them closed.

> **Not a code defect in the product.** The deploy-gate did exactly what it is
> supposed to do — refuse to deploy a candidate whose unit tests are red. The
> bug was that the *tests themselves* went red for reasons unrelated to the
> candidate. All three fixes harden the tests only; **none change deploy-gate
> logic, the PRD, product code, or any production timing.**

## The flakes and their fixes

| Issue | Flaky test | Root cause under oversubscription | Fix |
| ----- | ---------- | --------------------------------- | --- |
| [#4559](https://github.com/rysweet/Simard/issues/4559) | `install::paths::install_lock_is_exclusive_per_simard_home` | The install `flock` fd could be inherited by a sibling test's `fork`/`exec` child, which holds the inherited open-file-description across a guard drop, so a re-acquire gets `EAGAIN` | **Already closed by the existing `serial(install)` key (#4536)**, which prevents sibling install `fork`/`exec` tests from overlapping the lock window; std already opens the fd close-on-exec. No new production change — verify determinism under oversubscription via the exclusivity stress loop |
| [#4560](https://github.com/rysweet/Simard/issues/4560) (a) | `ooda_actions::tests_dispatch_concurrency::concurrent_dispatch_parallelizes_and_respects_cap` | Wall-clock assertion `parallel * 2 <= serial` requires a ≥2x speedup; full-parallel CPU oversubscription slows the parallel run's wall-clock enough to break the ratio | Relax the wall-clock bound to `parallel <= serial`; keep the deterministic peak-concurrency assertions as the real guarantee |
| [#4560](https://github.com/rysweet/Simard/issues/4560) (b) | `interruptible_sleep_exits_immediately_when_already_shutdown` and `test_interruptible_sleep_returns_immediately_on_shutdown` (already-shutdown fast-return), plus `interruptible_sleep_very_short_duration` (1ms normal completion) | A `< 1s` terminal deadline trips when the scheduler cannot resume the asserting thread within 1s under oversubscription — whether the return is a shutdown fast-return or the completion of a 1ms sleep | Widen the three `< Duration::from_secs(1)` deadlines to `< Duration::from_secs(5)` |
| [#4558](https://github.com/rysweet/Simard/issues/4558) | — (umbrella) | Tracks the three flakes above as one deploy-gate reliability workstream | Closed when #4559 and #4560 land |

All three fixes preserve full test parallelism and the deploy-gate's real
protective behaviour. No test is blanket-serialized; no production timing is
loosened; no deploy-gate or PRD logic is touched.

> **TL;DR**
>
> - **#4559:** the `flock` exclusivity flake is a fork-inheritance race whose
>   real guard already exists — the `#[serial_test::serial(install)]` key (#4536)
>   that keeps sibling install `fork`/`exec` tests from overlapping the lock
>   window — and Rust std already opens the lock fd close-on-exec. So the
>   deflake deliverable is **verification, not new product code**: prove the
>   serialized exclusivity test stays green under oversubscription. An explicit
>   `O_CLOEXEC` flag was considered and **rejected as redundant** (see the #4559
>   section); the exclusivity stress loop is the authoritative signal.
> - **#4560a:** replace `parallel_elapsed * 2 <= serial_elapsed` with
>   `parallel_elapsed <= serial_elapsed`. The `peak_parallel >= 2`,
>   `peak_serial <= 1`, and `run_count == N` assertions carry the real
>   concurrency proof; the wall-clock check is a weak regression signal only.
> - **#4560b:** widen three `< Duration::from_secs(1)` terminal deadlines to
>   `< Duration::from_secs(5)`. Two guard an already-shutdown fast-return against
>   a 60s sleep (5s is 12x below that hang); the third bounds completion of a 1ms
>   sleep. All three sit 5x above the flaky 1s ceiling.
> - **Gate:** the local canary unit-test suite exits `0` (no exit `101`) under
>   an oversubscribed `--test-threads`, and each named test passes on a
>   repeat/stress loop under simulated oversubscription.

---

## Fix #4559 — the install-lock `flock` fork-inheritance race

### The race and why it is already guarded

`acquire_install_lock` in `src/install/paths.rs` opens `$SIMARD_HOME/.install.lock`
and takes a non-blocking exclusive advisory lock:

```rust
let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
```

`flock` locks are associated with the open file **description**. Two facts govern
inheritance:

- Across `fork()`, a child inherits **all** open descriptors — `O_CLOEXEC` has no
  effect here.
- Across `exec()`, a descriptor survives **unless** it is marked close-on-exec
  (`FD_CLOEXEC`).

The original flake theory: under a full-parallel canary, a sibling install test
`fork`/`exec`s a child (classifying an on-disk entrypoint with `--version`)
during the window the lock fd is open. If the child holds an inherited reference
to the lock's open file description across the parent's guard drop, a re-acquire
in the exclusivity test sees the lock still held and `flock(LOCK_EX | LOCK_NB)`
returns `EAGAIN`, reddening the canary non-deterministically.

**This race is already closed — by two independent, pre-existing mechanisms:**

1. **The `serial(install)` serialization (issue #4536).**
   `install_lock_is_exclusive_per_simard_home` already carries
   `#[serial_test::serial(install)]`, and `src/install/serial_isolation_guard.rs`
   is a build-time meta-test that *enforces* this key on every install test that
   spawns a child or acquires the install `flock`. That key keeps the `flock`
   exclusivity test from ever overlapping a sibling install `fork`/`exec`, which
   is the only realistic source of an inherited lock fd in this binary. This is
   the true, verified guard.
2. **Rust std already opens the fd close-on-exec.**
   `std::fs::OpenOptions::open` sets `O_CLOEXEC` on every fd it opens on Unix
   (empirically confirmed on this toolchain: `F_GETFD` on the opened lock fd
   returns `FD_CLOEXEC` set). So the residual `exec()` inheritance vector from a
   *non-install* sibling is already covered with no code change.

### What we build for #4559: verification, not new product code

Because the race is already guarded, the correct deliverable is to **prove the
serialized exclusivity test stays deterministic under oversubscription** — not to
add production code. `acquire_install_lock` is left exactly as-is:

```rust
// UNCHANGED — std already opens this fd O_CLOEXEC; serialization (#4536)
// prevents sibling fork/exec overlap. No custom_flags needed.
let file = OpenOptions::new()
    .read(true).write(true).create(true).truncate(false)
    .open(&lock_path)
    .map_err(/* … unchanged error text … */)?;
```

> **Rejected option — explicit `O_CLOEXEC` via `custom_flags(libc::O_CLOEXEC)`.**
> An earlier draft proposed opening the lock with an explicit
> `OpenOptionsExt::custom_flags(libc::O_CLOEXEC)`. It is **rejected as redundant
> and misleading**:
>
> 1. It is **redundant with the standard library** — std already sets
>    `O_CLOEXEC`, so the flag changes no runtime behaviour.
> 2. `O_CLOEXEC` governs `exec()`, **not `fork()`**; it cannot close a
>    fork-without-exec inheritance path, so it would not be a general fix even if
>    std did not already set it.
> 3. Adding a no-op flag violates ruthless simplicity and would falsely imply the
>    fd flag is the mechanism, when the real guard is the `serial(install)` key.
>
> If the `EAGAIN` flake ever reproduces *despite* serialization, the cause lies
> elsewhere — a raw `fork`-without-`exec`, a `dup`/`dup2`'d descriptor, an fd
> passed explicitly to a child, or a genuine hold-across-drop timing race — and
> must be investigated directly. The exclusivity stress loop (below) is the
> authoritative signal, never the presence of a flag.

### What the test asserts, and how it stays green

`install_lock_is_exclusive_per_simard_home` is unchanged: a second
`acquire_install_lock` on the same `SIMARD_HOME` must fail while the first lock
is held, and must succeed after it is dropped. Its determinism rests on the
`serial(install)` key (enforced by `serial_isolation_guard.rs`) plus std's
default close-on-exec. The exclusivity stress loop under an oversubscribed
`--test-threads` is the verification that this holds; if it ever flakes, the
serialization/root-cause must be re-examined — there is no fd flag to "fix" it.


---

## Fix #4560a — the dispatch wall-clock ratio

### The assertion that was brittle

`concurrent_dispatch_parallelizes_and_respects_cap` in
`src/ooda_actions/tests_dispatch_concurrency.rs` runs the same workload twice —
once with `cap = N` (parallel) and once with `cap = 1` (serialized) — and
formerly demanded a ≥2x wall-clock speedup:

```rust
// BEFORE — brittle under oversubscription
assert!(
    parallel_elapsed * 2 <= serial_elapsed,
    "concurrent dispatch must be >=2x faster than serialized: …"
);
```

A ≥2x speedup requires that the parallel run actually gets ~N cores of wall-clock
throughput. Under full-parallel canary oversubscription (load avg 73–158), the
parallel run's threads are time-sliced against everyone else's, its wall-clock
inflates, and the ratio breaks even though dispatch is behaving correctly.

### The fix

Keep a weak, oversubscription-tolerant wall-clock bound, and let the existing
deterministic logical assertions carry the real guarantee:

```rust
// AFTER — tolerant wall-clock bound; logical assertions do the real work
assert!(
    parallel_elapsed <= serial_elapsed,
    "concurrent dispatch must not be slower than serialized: parallel={parallel_elapsed:?}, serial={serial_elapsed:?}"
);
```

The surrounding assertions are untouched and remain the proof of correctness:

```rust
assert!(peak_parallel >= 2, "with cap=N the slow run_turn calls must overlap; peak={peak_parallel}");
assert!(peak_serial   <= 1, "cap=1 must serialize dispatch; peak={peak_serial}");
// plus run_count == N
```

- `peak_parallel >= 2` deterministically proves the `cap = N` run **overlaps**
  work (real concurrency), regardless of wall-clock.
- `peak_serial <= 1` deterministically proves the `cap = 1` run **serializes**.
- `run_count == N` proves every action ran exactly once.

The wall-clock check is retained (not deleted) so a gross serialization
regression still leaves a signal, but it no longer gates on a load-sensitive
ratio.

---

## Fix #4560b — the 1-second terminal sleep deadlines

### The deadlines that were too tight

`interruptible_sleep` returns immediately when shutdown is already signalled,
and otherwise polls until the requested duration elapses. Three terminal
deadlines asserted completion in under **1 second** — two on the already-shutdown
fast-return path, and one on the normal completion of a 1ms sleep:

| File | Test | Kind | Site |
| ---- | ---- | ---- | ---- |
| `src/operator_commands_ooda/daemon/helpers.rs` | `interruptible_sleep_exits_immediately_when_already_shutdown` | already-shutdown fast-return (60s sleep, `shutdown=true`) | terminal deadline |
| `src/operator_commands_ooda/tests/daemon_inline.rs` | `test_interruptible_sleep_returns_immediately_on_shutdown` | already-shutdown fast-return (60s sleep, `shutdown=true`) | terminal deadline |
| `src/operator_commands_ooda/tests/daemon_inline.rs` | `interruptible_sleep_very_short_duration` | normal completion (1ms sleep, `shutdown=false`) | terminal deadline |

The two fast-return tests pass a 60s sleep against an already-`true` shutdown
flag and expect a near-instant return; the third passes a 1ms sleep against a
`false` flag and expects completion shortly after 1ms. All three formerly bounded
that return with a 1s ceiling:

```rust
// BEFORE (fast-return variant shown; the 1ms-completion variant is identical
// except for `Duration::from_millis(1)` and `shutdown=false`)
interruptible_sleep(Duration::from_secs(60), &shutdown);
assert!(start.elapsed() < Duration::from_secs(1));
```

Under oversubscription the return *logic* is instant, but the OS scheduler may
not resume the asserting thread within 1s, so `start.elapsed()` crosses the 1s
ceiling and the test fails.

### The fix

Widen the three terminal deadlines from `< Duration::from_secs(1)` to
`< Duration::from_secs(5)`:

```rust
// AFTER
interruptible_sleep(Duration::from_secs(60), &shutdown);
assert!(start.elapsed() < Duration::from_secs(5));
```

- **For the two already-shutdown fast-return tests, 5s is 12x below the 60s
  sleep** they guard, so a genuine "never woke, waited the full 60s" hang still
  fails the assertion.
- **For the 1ms normal-completion test (`interruptible_sleep_very_short_duration`),**
  5s simply tolerates scheduler wake latency after a trivially short sleep; there
  is no 60s sleep here, so the bound is a loose completion-latency ceiling rather
  than a hang-catcher.
- **5s is 5x above the flaky 1s ceiling**, comfortably tolerating the observed
  73–158 load average.
- The companion `< Duration::from_secs(2)` deadlines on the *mid-sleep* wake and
  short-duration completion tests (`helpers.rs`
  `interruptible_sleep_exits_on_mid_sleep_shutdown`; `daemon_inline.rs`
  `test_interruptible_sleep_completes_short_duration` and
  `test_interruptible_sleep_mid_shutdown`) are **intentionally left as-is** —
  they are not on the observed flaky path and their timing budget is already
  adequate.

Only these three `< Duration::from_secs(1)` deadlines change.

---

## Verification gate

The fixes are considered closed when all of the following hold:

1. **Local canary deploy-gate green.** The local canary unit-test suite exits
   `0` (no exit `101`) under an oversubscribed `--test-threads`, reproducing the
   full-parallel-canary CPU pressure that produced the red streak.
2. **Each named test passes on stress/repeat** under simulated oversubscription:

   ```bash
   # #4559 — lock exclusivity, high thread count + repeat
   for i in $(seq 1 20); do \
     cargo test -p simard install_lock_is_exclusive_per_simard_home -- --exact --test-threads=64 || break; \
   done

   # #4560a — dispatch concurrency
   for i in $(seq 1 20); do \
     cargo test -p simard concurrent_dispatch_parallelizes_and_respects_cap -- --exact --test-threads=64 || break; \
   done

   # #4560b — terminal-sleep deadlines
   for i in $(seq 1 20); do \
     cargo test -p simard interruptible_sleep -- --test-threads=64 || break; \
   done
   ```

3. **Green `cargo test --all-features`** and **`pre-commit run --all-files`**.
4. **Self-deploy unblocked / `DeployDrift` returns to 0** once the candidate
   carrying these fixes passes the reproduced local canary gate.

The deterministic assertions (`peak_parallel >= 2`, `peak_serial <= 1`,
`run_count == N`, and the exclusivity acquire/drop/re-acquire sequence) are the
real verification; the widened wall-clock and terminal bounds must always sit
**alongside** them, never replace them, so a real serialization regression or a
genuine no-wake hang still trips.

## Constraints honoured

- **Additive / test-only / non-breaking.** No product code changes at all —
  `acquire_install_lock` is untouched (#4559 is closed by pre-existing
  serialization). No change to deploy-gate logic, the PRD, or the GitHub Actions
  `verify` workflow (already green). No budget/resource-pressure config touched.
- **No Bridge naming.** No renames introduced.
- **No stray `print!`/`println!`.** These sites use structured `tracing`/OTel
  only; the changes add no logging (and log no fd numbers, raw paths, or lock
  internals).

## Out of scope

- [#4562](https://github.com/rysweet/Simard/issues/4562) (`ooda-stuck`,
  `goal_hygiene`) — the OODA no-progress breaker filed this for **operator
  attention** because the goal's success criteria are unclear after a guided
  retry. It is not a bounded code fix and is routed to a human operator, not to
  a workflow.
- Deploy-gate logic, the `verify` workflow, and budget/resource-pressure config.

## Related pages

- [De-flaking the known flaky tests](./deflaking-known-flaky-tests.md) — the
  earlier env-race / state-root-race de-flake work and its serial-key and
  explicit-root patterns.
- [install serial isolation](./install-serial-isolation.md) — the install-suite
  isolation contract (`serial(install)` key + `serial_isolation_guard.rs`) that
  is the actual guard closing the #4559 `flock` exclusivity race.
- [CI-resilient test patterns](./ci-resilient-test-patterns.md) — companion
  patterns (constant-relative assertions, lazy config resolution, serial
  env-var tests).
- [Canary gate isolation and self-deploy convergence](../reference/canary-gate-convergence.md)
  — the deploy-gate / relaunch-canary machinery whose local unit-test gate these
  fixes keep green.
- [Overseer deploy red-canary diagnostics](../reference/overseer-deploy-canary-diagnostics.md)
  — the diagnostics that surface *which* gate reddened, which pointed at the
  local unit-test gate here.
- [Reconcile and self-deploy](../concepts/reconcile-and-self-deploy.md) — the
  merged-to-running convergence loop that `DeployDrift` measures.
