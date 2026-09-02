---
title: Self-deploy canary — unit-test gate state-root isolation
description: Reference for how the RelaunchGate::UnitTest canary isolates its STATE ROOT into a fresh per-run TempDir (overriding SIMARD_STATE_ROOT / SIMARD_HOME and removing SIMARD_MEMORY_SOCKET after scrub_gate_env) so the canary's `cargo test` never collides with the live daemon's lbug cognitive store and typed-OODA sqlite outcome store — the collision that reddened every self-deploy with `exit status: 101` at test Drop. Fail-closed, absolute isolated root, scoped to the unit-test gate only.
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./deterministic-canary-unit-test-gate.md
  - ./canary-gate-convergence.md
  - ./self-deploy-api.md
  - ./overseer-deploy-canary-diagnostics.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../howto/converge-a-stuck-red-canary-self-deploy.md
  - ../../src/self_relaunch/gates.rs
  - ../../src/error/mod.rs
---

# Self-deploy canary — unit-test gate state-root isolation

> **Status: implemented.** This document describes the isolation seam in
> [`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
> as `build_unit_test_command` + `unit_test_gate_failed_closed`, consumed by
> `run_unit_test_gate`. The gate is **fail-closed**: if the isolated state root
> cannot be created the canary reddens; it never falls back to the live root.
> This supersedes the stale PR #4632 (see [Provenance](#provenance)).

## Why this exists

Simard's OODA daemon self-deploys by building a candidate binary and running a
canary of [`default_gates()`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
before it swaps in the new image. One of those gates, `RelaunchGate::UnitTest`,
shells the **full** `cargo test` suite against the candidate.

The gate ran with a scrubbed environment (see
[`scrub_gate_env`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)),
but that scrub's operator/canary allow-list **passes `SIMARD_STATE_ROOT` and
`SIMARD_HOME` through** from the live daemon environment — they are legitimate
deploy-shape signals the `rpc-health` gate needs to dial the running daemon.
The side effect: the spawned `cargo test` inherited the **same state root the
running daemon already held open**, so the test process opened the very same
on-disk stores:

- the **lbug cognitive store** (the daemon's cognitive-memory backing store), and
- the **typed-OODA sqlite outcome store**.

Two processes opening the same single-writer stores collide on locks / state.
The collision surfaced late — at test **`Drop`**, roughly **1.7 s** into
`unittests` — as `exit status: 101`. That reddened **every** self-deploy canary
run, so the daemon refused to promote a perfectly healthy candidate and
autonomous self-deploy was blocked, even though CI ran the identical suite green
on clean runners (which have no live daemon and thus no shared state root).

The fix gives the unit-test gate its **own throwaway state root** so its
`cargo test` can never touch the live daemon's stores.

> **Deterministic fix only.** This feature does **not** loosen any timing bound,
> add sleeps/retries, or suppress a flaky test. The `101` was never a race in
> test logic — it was a shared-state collision. Removing the sharing removes the
> red at the root.

## What it does

`run_unit_test_gate` builds its `cargo test` command through a new seam,
`build_unit_test_command`, which:

1. Creates a **fresh, absolute, per-run** temporary directory
   (`tempfile::TempDir`) to serve as an isolated state root.
2. Builds the scrubbed `cargo test` command via the existing `scrubbed_command`
   helper — so `scrub_gate_env` runs **first**.
3. **After** the scrub (last-write-wins), overrides the state-pointing env so the
   canary opens its own throwaway stores instead of the daemon's:
   - `cmd.env("SIMARD_STATE_ROOT", <tempdir>)`
   - `cmd.env("SIMARD_HOME", <tempdir>)`
   - `cmd.env_remove("SIMARD_MEMORY_SOCKET")`
4. Returns `(Command, TempDir)`. The `TempDir` guard is **bound by
   `run_unit_test_gate` (as `_state_root`) so it outlives `cmd.output()`** — the
   isolated root exists for the whole test run and is cleaned up only after the
   subprocess exits.

The three overrides are applied *after* `scrubbed_command` on purpose: the scrub
re-injects the allow-listed `SIMARD_STATE_ROOT` / `SIMARD_HOME` from the live
env, and the later `cmd.env(...)` calls **win** (last-write-wins), exactly the
way the existing `CARGO_BUILD_JOBS` override already layers on top of the scrub.

### The env contract, before vs after

| Variable | Live daemon value (post-scrub) | Value seen by the canary `cargo test` |
| --- | --- | --- |
| `SIMARD_STATE_ROOT` | live daemon state root (allow-listed through) | **overridden** → fresh absolute `TempDir` |
| `SIMARD_HOME` | live deploy-shape home (allow-listed through) | **overridden** → same fresh absolute `TempDir` |
| `SIMARD_MEMORY_SOCKET` | live memory-ipc socket (if set) | **removed** → tests cannot dial the live daemon |
| `CARGO_BUILD_JOBS` | — | set to `cargo_jobs()` (unchanged behavior) |
| everything else | per `scrub_gate_env` deny-by-default floor + allow-list | unchanged |

## Fail-closed contract

If the isolated state root **cannot be created**, the gate **reddens** — it must
never run `cargo test` against the live state root.

- `TempDir::new()` failure is mapped to
  [`SimardError::PersistentStoreIo { store, action, path, reason }`](https://github.com/rysweet/Simard/blob/main/src/error/mod.rs)
  and propagated out of `build_unit_test_command`
  (`build_unit_test_command(config) -> SimardResult<(Command, TempDir)>`).
- `run_unit_test_gate` matches on that result. On `Err`, it logs and returns a
  **RED** `GateResult` via `unit_test_gate_failed_closed(reason)`. `GateResult`
  has exactly three fields — `gate`, `passed`, `detail` (see
  [`src/self_relaunch/types.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs)) —
  so the verdict is carried by `passed: false`, with a `detail` that names the
  isolated-state-root failure. There is no `unwrap_or`, no `.ok()`, and no
  fallback to the live root anywhere on this path.

```text
build_unit_test_command(config)
  ├─ Ok((cmd, state_root)) ─▶ bind `_state_root`; run cmd.output();
  │                            preserve the #4558 summarize_test_failure path
  └─ Err(e) ───────────────▶ unit_test_gate_failed_closed(e)  → RED GateResult
```

On the happy path the existing diagnostics are preserved unchanged: a real test
failure still routes through `summarize_test_failure(&stdout, &stderr)` (the
#4558 diagnosable-gate behavior, landed via PR #4629) and still fails closed. See the companion
[deterministic canary unit-test gate reference](./deterministic-canary-unit-test-gate.md).

## Scope discipline — only the unit-test gate

The isolation override is applied to **`run_unit_test_gate` only**. The other
three canary gates are **unchanged**:

| Gate | Isolated state root? | Why |
| --- | --- | --- |
| `RelaunchGate::UnitTest` | **yes** | Shells `cargo test`, which opens the stores; must not touch the live daemon's. |
| `RelaunchGate::Smoke` | no | Runs the candidate binary's smoke probe; no store-collision seam. |
| `RelaunchGate::GymBaseline` | no | Runs `gym list`; no store-collision seam. |
| `RelaunchGate::RpcHealth` | no | **Must** keep dialing the **live** daemon over its real state root / memory socket — that is the availability check. Isolating it would break the probe. |

Copying the override into `rpc-health` (or the other two) is an anti-goal: it
would defeat the health check the gate exists to perform.

## Security considerations

- **Isolation (primary).** Overriding `SIMARD_STATE_ROOT` + `SIMARD_HOME` to an
  ephemeral `TempDir` and removing `SIMARD_MEMORY_SOCKET` guarantees the canary
  test process cannot read, write, lock, or corrupt the live daemon's cognitive
  and OODA-outcome stores, and cannot dial the live memory-ipc socket.
- **Fail-closed (non-negotiable).** A tempdir-creation failure reddens the gate
  via `PersistentStoreIo`; there is no path that silently reuses the live root.
- **Ordering integrity.** The overrides are applied **after** `scrubbed_command`
  so they win last-write-wins over any allow-listed live `SIMARD_STATE_ROOT` /
  `SIMARD_HOME`. The scrub's deny-by-default floor and `is_hijack_class_env`
  refusal are untouched.
- **Absolute path.** The isolated root is always an **absolute** path (from
  `mkdtemp` under the system temp dir), asserted with
  `debug_assert!(path.is_absolute())`; no CWD-relative root can leak in.
- **No cross-run bleed.** `TempDir::new()` (mkdtemp, `0700` owner-only) yields a
  unique root per invocation, so two canary runs never share a state root.
- **Lifetime safety.** The `TempDir` guard is bound in `run_unit_test_gate` and
  outlives `cmd.output()`; the root is never deleted mid-run.

## Verification (tests)

Two ported security tests in the `#[cfg(test)] mod tests` block in
`gates.rs` pin the contract. They mutate global env and therefore run under
`#[serial_test::serial(cognitive_memory)]` with verbatim save/restore of the
env they touch:

- **`unit_test_gate_overrides_state_root_to_isolated_temp`** — with a live
  allow-listed `SIMARD_STATE_ROOT` present in the environment, asserts the
  built command's `SIMARD_STATE_ROOT` / `SIMARD_HOME` point at an **absolute**
  path **under the system temp dir** (not the live root), that the override
  **wins** over the scrubbed allow-list value, that `SIMARD_MEMORY_SOCKET` is
  **removed**, and that two invocations produce **distinct** roots.
- **`unit_test_gate_fails_closed_when_state_isolation_unavailable`** — when the
  isolated root cannot be created, asserts the gate **reddens** (the `Err`
  propagates / the `GateResult` is `passed: false`) rather than falling back to
  the live state root.

## Provenance

This work **supersedes PR #4632** (branch
`feat/issue-4628-diagnosed-systemic-crash-loop-the-self-deploy-cana`, commit
`a0308a7`), which authored the same isolation *approach* but branched **before**
PR #4629 rewrote `gates.rs` (landing the #4558 diagnosability fix that introduced
`scrub_gate_env` / `summarize_test_failure`) and had gone stale / conflicting. This implementation
ports #4632's approach and its security tests **cleanly onto current `main`**,
preserving #4558's `summarize_test_failure` diagnostics rather than restoring
#4632's older `stderr`-truncation failure path. Tracking: issue **#4628**.

## See also

- [Deterministic self-deploy canary — unit-test gate diagnostics](./deterministic-canary-unit-test-gate.md)
- [Canary gate convergence](./canary-gate-convergence.md)
- [Self-deploy API reference](./self-deploy-api.md)
- [Concept: reconcile-and-self-deploy](../concepts/reconcile-and-self-deploy.md)
- [How to converge a stuck red-canary self-deploy](../howto/converge-a-stuck-red-canary-self-deploy.md)
