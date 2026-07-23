---
title: Deploy-gate self-deploy test state-root robustness
description: How the self-deploy health tests resolve their state root deterministically under the deploy-gate CI environment so the `cargo test` step stops exiting with status 101. The exit-101 must first be diagnosed from the real panic message — it may be a test-body assertion/`unwrap` unwinding, an env-divergence panic, or an already-resolved fault — before any fix is applied (rysweet/Simard#4506).
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./state-root-resolution.md
  - ./self-deploy-api.md
  - ./operator-read-state-root-contract.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../../src/self_deploy/tests_health.rs
  - ../../src/self_deploy/tests_source_prep.rs
  - ../../src/state_root.rs
---

# Deploy-gate self-deploy test state-root robustness

> **Status: implemented.** The `StateRootGuard` scoped-env helper and its
> `Drop` teardown live in
> [`src/self_deploy/tests_health.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/tests_health.rs)
> and
> [`src/self_deploy/tests_source_prep.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/tests_source_prep.rs).
> State-root resolution is the shared helper in
> [`src/state_root.rs`](https://github.com/rysweet/Simard/blob/main/src/state_root.rs)
> (`STATE_ROOT_ENV = "SIMARD_STATE_ROOT"`). This page documents the
> environment contract that keeps the deploy-gate unit-test step green.

The **deploy-gate** unit-test step (`cargo test` run under the self-deploy
canary environment) red-canaried for 5h+ with **exit status 101** on the
self-deploy health tests, even though main-branch CI (`verify` + `release`)
stayed green.

> **Diagnose before fixing.** Exit 101 only tells us *a Rust test panicked* —
> it does **not** by itself identify *which* panic. Do **not** assume it is the
> `StateRootGuard` `Drop` or an env-divergence panic: the current
> `StateRootGuard::Drop` is already `unwrap`-free (plain `set_var`/`remove_var`)
> and the tests already build their state root from a `TempDir`, so neither is a
> guaranteed culprit. The fix must start from the **actual panic message** in
> the deploy-gate log. The most likely candidates, in order:
>
> 1. **A test-body assertion or `unwrap` unwinding** — e.g. `set_mtime`'s
>    `File::open(...).unwrap()`/`set_times(...).unwrap()`, the
>    `run_self_health_probe(...).unwrap()`, or an `assert!` on probe output —
>    failing only because the deploy-gate's ambient `$HOME`/`SIMARD_STATE_ROOT`
>    made the on-disk layout differ from a laptop.
> 2. **Env divergence** the guard did not fully isolate (a leaked or absolute
>    env value the test did not override).
> 3. **Already resolved** on the default branch — in which case the deploy-gate
>    was running a stale canary and the correct action is to re-run/re-pin it,
>    not to change test code.

This reference specifies the finished contract once the panic is diagnosed. For
the resolution ladder the guard defers to, see
[State-root resolution](./state-root-resolution.md).

## Contents

- [Diagnosing the exit-101 panic](#diagnosing-the-exit-101-panic)
- [The `StateRootGuard` contract](#the-staterootguard-contract)
- [State-root derivation in tests](#state-root-derivation-in-tests)
- [Running the deploy-gate test locally](#running-the-deploy-gate-test-locally)
- [What is unchanged](#what-is-unchanged)

## Diagnosing the exit-101 panic

A Rust test process exits with status **101** when *any* test thread panics —
whether the panic originates in the test body, in an assertion, or in a `Drop`
running during unwind. The **first step is to read the panic line** from the
deploy-gate log (`panicked at '<msg>', src/self_deploy/...`); the fix depends on
which of the candidates above it names:

- **Test-body assertion / `unwrap`** (most likely): make the offending value
  come **only** from a `TempDir` the test owns (see
  [State-root derivation in tests](#state-root-derivation-in-tests)) so the
  assertion holds regardless of the ambient `$HOME`/`SIMARD_STATE_ROOT`. Keep
  `unwrap`/`expect` on *setup* steps — a genuine setup failure should still
  surface loudly.
- **`Drop`-time fault**: the guard's teardown is already assertion-free and
  `unwrap`-free (see next section); if the panic line points at `Drop`, verify
  no double-panic path was reintroduced rather than assuming one exists.
- **Already resolved**: if the panic cannot be reproduced with a divergent env
  (below), treat the red canary as stale and re-pin/re-run the deploy-gate.

The goal of the fix is the same regardless of candidate: the health tests must
be **independent of the host's `$HOME`** and pass identically on a laptop and
under the deploy-gate canary.

## The `StateRootGuard` contract

`StateRootGuard` is a scoped override of `SIMARD_STATE_ROOT` (`STATE_ROOT_ENV`)
used by the self-deploy health and source-prep tests. Its guarantees:

| Guarantee | Behaviour |
| --- | --- |
| **Serialized** | Every guard-using test carries `#[serial_test::serial(simard_state_root_env, cognitive_memory)]`, so no two tests mutate the process env concurrently. |
| **Restores on drop** | On `Drop`, the guard restores the captured prior value, or removes the var if it was previously unset. |
| **Never panics in `Drop`** | The teardown performs no assertions, no `unwrap()`, and no `expect()` — it is already a plain `set_var`/`remove_var`. This is verified in the current source, so `Drop` is **not** a guaranteed exit-101 source; it is kept assertion-free so it can never *become* one. |
| **Absolute, isolated root** | The guard is always constructed with an **absolute** path inside a per-test `TempDir`, never a relative path and never the real `~/.simard`. |

The guard's construction and teardown (paraphrased):

```rust
/// Scoped `SIMARD_STATE_ROOT` override that restores on drop.
/// Env access is serialized via `#[serial(simard_state_root_env, ...)]`.
struct StateRootGuard {
    prev: Option<std::ffi::OsString>,
}

impl StateRootGuard {
    fn set(value: &std::path::Path) -> Self {
        let prev = std::env::var_os(STATE_ROOT_ENV);
        // SAFETY: serialized via #[serial(simard_state_root_env, cognitive_memory)].
        unsafe { std::env::set_var(STATE_ROOT_ENV, value); }
        Self { prev }
    }
}

impl Drop for StateRootGuard {
    fn drop(&mut self) {
        // Assertion-free and unwrap-free so unwinding through this Drop can
        // never double-panic (rysweet/Simard#4506). NOTE: this is already the
        // shipped behaviour — Drop is not the confirmed exit-101 source; the
        // panic must be read from the deploy-gate log before attributing it.
        // SAFETY: serialized via #[serial(simard_state_root_env, cognitive_memory)].
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var(STATE_ROOT_ENV, v),
                None => std::env::remove_var(STATE_ROOT_ENV),
            }
        }
    }
}
```

## State-root derivation in tests

Health-probe tests scan `<state_root>/state/` for fresh quarantines. The state
root a test uses is derived **only** from a `TempDir` the test owns:

```rust
let tmp = tempfile::tempdir().expect("tempdir");
let root = tmp.path().to_path_buf();          // absolute, isolated
let _guard = StateRootGuard::set(&root);       // scoped env override
std::fs::create_dir_all(root.join("state")).expect("mk state dir");
```

Rules the finished tests obey:

- **No hardcoded `/home/azureuser`** or any absolute developer path. Nothing in
  the test tree assumes a specific `$HOME`.
- **Fail loudly on setup, never in teardown.** Setup steps (`tempdir()`,
  `create_dir_all`) may `expect(...)` — a genuine setup failure should surface.
  Teardown (`Drop`) must not.
- **Deterministic under the deploy-gate.** Because the root comes from
  `TempDir`, the probe sees exactly the files the test created regardless of the
  ambient `$HOME`/`SIMARD_STATE_ROOT` the deploy-gate exports.
- **Test-body `unwrap`s operate only on `TempDir`-owned paths.** I/O helpers
  such as `set_mtime` and the `run_self_health_probe(...).unwrap()` assertion
  act on files under the per-test `TempDir`, so they cannot panic from a
  divergent host layout — the leading candidate for the exit-101.

## Running the deploy-gate test locally

Reproduce the deploy-gate unit-test step exactly (the environment that
red-canaried):

```bash
# Mirror the deploy-gate: run the self-deploy health tests under --locked.
cargo test --all-features --locked --no-fail-fast self_deploy::tests_health
cargo test --all-features --locked --no-fail-fast self_deploy::tests_source_prep
```

To prove the env-independence fix, run with a deliberately divergent env — the
tests must still pass and exit 0:

```bash
HOME=/nonexistent SIMARD_STATE_ROOT= \
  cargo test --all-features --locked self_deploy::tests_health
```

A green run (exit 0, no `test ... FAILED`, no `process didn't exit
successfully: ... (exit status: 101)`) confirms the panic is gone. If it still
fails, capture the `panicked at ...` line and match it to a candidate in
[Diagnosing the exit-101 panic](#diagnosing-the-exit-101-panic) before changing
code.

## What is unchanged

- The **probe semantics** are unchanged: a fresh quarantine under
  `<state_root>/state/` still fails `no_quarantine`; a top-level
  `<state_root>/` artifact still does not (see the existing
  `tests_health.rs` cases).
- The [state-root resolution ladder](./state-root-resolution.md)
  (`$SIMARD_STATE_ROOT` if absolute/non-empty, else `~/.simard`) is unchanged.
  Only the **tests'** env handling and root derivation were hardened.
- The failing `#[test]` is **not** deleted and **not** `#[ignore]`-ed — it
  exists and passes. `rysweet/Simard#4506` is closed by this change.
