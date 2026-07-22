---
title: De-flaking ooda_config_default_values (OodaConfig env leak)
description: >
  How the shared flaky unit test
  `ooda_loop::tests_types::ooda_config_default_values` was made deterministic
  under parallel `cargo test`. `OodaConfig::default()` reads the process-global
  concurrency env vars, so a concurrent env-mutating test in the sibling
  `ooda_loop::types::tests_ooda_config` module could leak a value into the
  unguarded default test and make `max_concurrent_actions != 24`. The fix tags
  the test with the shared `cognitive_memory` serial key and clears the three
  concurrency vars before reading the default — a test-only, additive change
  with no production behaviour change.
last_updated: 2026-07-22
review_schedule: when a new env var is read by OodaConfig::default(), or when serial_test is upgraded
owner: simard
doc_type: reference
related:
  - ./hermetic-tests.md
  - ./cognitive-memory-serial-isolation.md
  - ./deflaking-known-flaky-tests.md
  - ./ci-resilient-test-patterns.md
  - ./COVERAGE_BASELINE.md
---

# De-flaking `ooda_config_default_values`

This page documents the finished state of the work that made the shared flaky
unit test `ooda_loop::tests_types::ooda_config_default_values` deterministic
under parallel `cargo test`. It is the test-author and reviewer contract for the
fix and the verification gate that keeps it closed.

The flake and its fix:

| Issue | Flaky test | Root cause | Fix |
| ----- | ---------- | ---------- | --- |
| [#4433](https://github.com/rysweet/Simard/issues/4433) | `ooda_loop::tests_types::ooda_config_default_values` (`src/ooda_loop/tests_types.rs`) | `OodaConfig::default()` reads the process-global concurrency env vars; a **concurrent** env-mutating test in the sibling `types::tests_ooda_config` module leaks a value into this unguarded default test, so `max_concurrent_actions != 24` | Tag the test with the shared `#[serial_test::serial(cognitive_memory)]` key and clear the three concurrency vars before calling `OodaConfig::default()` |

The fix preserves full test parallelism (it serializes only against the other
env-mutating members of the `cognitive_memory` group, not the whole suite) and
changes **no production behaviour**: `OodaConfig::default()` still resolves the
concurrency ceiling from the environment exactly as before. The suite is simply
prevented from racing on that read.

> **TL;DR**
>
> - Add `#[serial_test::serial(cognitive_memory)]` to `ooda_config_default_values`,
>   reusing the exact literal key the sibling `tests_ooda_config` module already
>   uses. The `serial_test` key is a **binary-global** string registry, so it
>   serializes this test against the env-mutating tests in the other module even
>   though they live in different modules.
> - At the top of the test body, clear `SIMARD_OODA_MAX_CONCURRENT`,
>   `SIMARD_MAX_CONCURRENT_ACTIONS`, and `SIMARD_SCALING` so the test starts from a
>   known-unset baseline and is order-independent (it never relies on a sibling's
>   cleanup).
> - **Gate:** ≥20 consecutive parallel runs with zero failures, plus a green
>   `cargo test --all-features --locked --no-fail-fast` and green
>   `coverage/coverage`.

---

## The race this eliminates

The `cargo test --lib` test binary runs many tests **concurrently in one
process**. The OS environment (`environ`) is **process-global** and glibc
`setenv`/`getenv` are **not thread-safe**. See
[serial(cognitive_memory) test isolation](./cognitive-memory-serial-isolation.md)
for the whole-binary treatment of this hazard.

`OodaConfig::default()` reads three env vars to compute the per-OODA-cycle
concurrency ceiling and scaler wiring:

- `SIMARD_OODA_MAX_CONCURRENT`
- `SIMARD_MAX_CONCURRENT_ACTIONS`
- `SIMARD_SCALING`

Two modules test this surface:

- `src/ooda_loop/types.rs` → `mod tests_ooda_config` — several tests that
  **mutate** those vars (`set_var`/`remove_var`) to exercise the scaler wiring
  (issues #2182 / #2935). These already carry
  `#[serial_test::serial(cognitive_memory)]` and use the module-private
  `clear_concurrency_env()` helper under a private `ENV_LOCK`.
- `src/ooda_loop/tests_types.rs` → `ooda_config_default_values` — asserts the
  **unset** defaults (`max_concurrent_actions == 24`, `improvement_threshold ~
  0.02`, `gym_suite_id == "progressive"`).

Before the fix, `ooda_config_default_values` carried **no** serial key and did
**not** clear the env. When it ran concurrently with a `tests_ooda_config` test
that had just done `set_var("SIMARD_OODA_MAX_CONCURRENT", …)` (or was
mid-`set`/`remove`), the default test observed the leaked value and
`config.max_concurrent_actions` was no longer `24`. Because the failing code is
shared from `main`, the flake appeared **identically across every open PR** —
including a fresh, otherwise-clean PR (#4433) — which is the signature of a
shared test defect rather than a per-PR regression.

## The fix

`ooda_config_default_values` is made hermetic with two test-only edits, both in
`src/ooda_loop/tests_types.rs`:

1. **Serial key.** The test is tagged
   `#[serial_test::serial(cognitive_memory)]`, reusing the **exact literal key**
   the sibling `tests_ooda_config` module uses. `serial_test` keys resolve
   through a single binary-global registry keyed by the string, so this
   serializes `ooda_config_default_values` against the env-mutating tests in the
   other module without needing access to that module's private `ENV_LOCK`.

2. **Clean baseline.** The test body clears the three concurrency vars before
   constructing the config, establishing an unset baseline and making the test
   order-independent — it no longer depends on any sibling having cleaned up.
   The clears are factored into a small module-local `clear_concurrency_env()`
   helper (mirroring the sibling `tests_ooda_config` module's helper of the same
   name) and called at the top of the test body:

   ```rust
   /// Remove every env var that influences `OodaConfig::default()` so the test
   /// starts from a known-unset baseline. The `cognitive_memory` serial key on
   /// the caller guarantees no other test is mutating the process environment
   /// concurrently, which makes these `remove_var` writes sound.
   fn clear_concurrency_env() {
       unsafe { std::env::remove_var("SIMARD_OODA_MAX_CONCURRENT") };
       unsafe { std::env::remove_var("SIMARD_MAX_CONCURRENT_ACTIONS") };
       unsafe { std::env::remove_var("SIMARD_SCALING") };
   }

   // ... at the top of the test body:
   clear_concurrency_env();
   ```

The three assertions are unchanged:

```rust
let config = OodaConfig::default();
assert_eq!(config.max_concurrent_actions, 24);
assert!((config.improvement_threshold - 0.02).abs() < f64::EPSILON);
assert_eq!(config.gym_suite_id, "progressive");
```

### Why the serial key, not the private lock

The sibling module guards its env mutation with a module-private
`static ENV_LOCK: Mutex<()>` plus the `cognitive_memory` serial key. `ENV_LOCK`
is private to `tests_ooda_config` and cannot be imported from
`tests_types.rs`. Reaching for it would require widening the visibility of a
test-internal symbol for no functional benefit — the `serial_test` key **already**
provides binary-wide mutual exclusion by string, which is exactly what is needed
across modules. The inline `remove_var` clears then make the test self-contained.
This is the same reasoning documented in
[serial(cognitive_memory) test isolation](./cognitive-memory-serial-isolation.md):
the shared key is the whole-binary contract; the per-test env reset is the
hermetic baseline.

## Verification gate

The fix is considered closed when all of the following pass.

### Targeted

```bash
cargo test ooda_config_default_values
```

### Deterministic / order-independence

Run the exact CI invocation, and repeat under varied thread counts to prove the
result does not depend on scheduling luck:

```bash
cargo test --all-features --locked --no-fail-fast
cargo test --all-features -- --test-threads=1
cargo test --all-features -- --test-threads=8
```

≥20 consecutive parallel runs of the OODA-config tests must complete with zero
failures. The build must also pass the
[`serial_guard` meta-test](./cognitive-memory-serial-isolation.md) that enforces
the `cognitive_memory` watched-env contract.

### Coverage

```bash
# CI pins the nightly toolchain (see .github/workflows/coverage.yml);
# use the same pin locally to match the instrumented build:
cargo +nightly-2026-07-01 llvm-cov --workspace --lib --bins
```

`coverage/coverage` must go green. The coverage failures previously seen on
#4331 / #4366 share this root cause (the same non-hermetic test panicking inside
the instrumented run) and are resolved by the same fix — see
[the coverage baseline](./COVERAGE_BASELINE.md).

### CI

The `.github/workflows/verify.yml` → `Run cargo test` step (which runs
`cargo test --all-features --locked --no-fail-fast` directly as a commit-stage
gate, not through Python pre-commit) and the `coverage/coverage` check must go
green on all required checks. **No CI-workflow edits** are part of this work.

---

## Configuration & environment

This fix adds no new runtime configuration. The relevant variables are all
existing `OodaConfig::default()` inputs:

| Variable | Role | Notes |
| -------- | ---- | ----- |
| `SIMARD_OODA_MAX_CONCURRENT` | Overrides the per-OODA-cycle concurrency ceiling (default `24`, issue #2935) | Read by `OodaConfig::default()`; cleared by the test to observe the unset default |
| `SIMARD_MAX_CONCURRENT_ACTIONS` | Legacy/alias override for the concurrency ceiling | Cleared by the test for the same reason |
| `SIMARD_SCALING` | Selects the auto-scaler wiring (`auto` populates `config.scaler`) | Cleared by the test so `scaler` follows the unset default |

None of these are secrets; they are non-sensitive concurrency knobs. The fix
adds no new dependency — `serial_test` is already pinned exactly
(`serial_test = "=3.4.0"`) in `Cargo.toml`.

### Local stress-run memory budget

The saved workstation preference for memory-heavy local stress loops is
`NODE_OPTIONS=--max-old-space-size=32768` (used by the tooling that drives the
loops, not by `cargo` itself). To change it, edit `~/.amplihack/config`. It is
not required for CI and does not affect the Rust tests.

---

## Scope

**In scope:** the test-only changes to `src/ooda_loop/tests_types.rs` and
`src/operator_commands_ooda/tests/report_tests.rs` (serial key + env clears on
every test that asserts the built-in `OodaConfig::default()` concurrency
defaults), a deterministic regression guard
(`ooda_config_default_values_is_hermetic_under_leaked_env`), the verification
gate, and this page.

**Out of scope (confirmed):**

- Any change to `OodaConfig::default()` or other production code — the env
  contract is deliberately untouched.
- Widening the visibility of the sibling module's private `ENV_LOCK` /
  `clear_concurrency_env()` — the `cognitive_memory` serial key already provides
  the needed cross-module exclusion.
- Weakening coverage gates, deleting tests, or `#[ignore]`-ing the test to make
  CI pass.
- amplihack-rs PR #968 (a separate, already-green operator merge decision, not a
  code fix).
- Unrelated in-flight items (self-deploy red-canary loop, reaper duplicate-PR
  defect, the `ooda-stuck` label, delivery pair #4440/#4398) and per-PR feature
  defects.

## Related pages

- [Writing hermetic tests against cognitive memory](./hermetic-tests.md) — how a
  single test allocates an isolated baseline.
- [serial(cognitive_memory) test isolation](./cognitive-memory-serial-isolation.md)
  — the whole-binary watched-env contract and the `serial_guard` meta-test that
  keeps env-mutating tests keyed.
- [De-flaking the known flaky tests](./deflaking-known-flaky-tests.md) — the
  prompt-delivery env race and the goal-board state-root race, the sibling
  entries in this ledger.
- [CI-resilient test patterns](./ci-resilient-test-patterns.md) — companion
  patterns (lazy config resolution, serial env-var tests).
- [Test-coverage baseline](./COVERAGE_BASELINE.md) — the `coverage/coverage`
  gate this fix also un-blocks.
