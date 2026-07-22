---
title: Deploy canary gate curation API reference
description: Reference for the curated, recursion-free deploy-canary gate list — the `canary_gates()` constructor in src/self_relaunch/types.rs (the `[Smoke, GymBaseline, RpcHealth]` set that deliberately excludes the `UnitTest` gate), the overseer deploy wiring that runs it instead of `default_gates()`, and the fail-closed recursion sentinel that keeps the canary from shelling `cargo test` inside itself. Root-causes and fixes the deterministic red-canary exit-101 self-deploy crash-loop (#4469 / #4470).
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./overseer-deploy-canary-diagnostics.md
  - ./self-deploy-api.md
  - ./self-deploy-source-prep.md
  - ./self-health-quarantine-classification-api.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../safe-self-update.md
  - ../../src/self_relaunch/types.rs
  - ../../src/self_relaunch/gates.rs
  - ../../src/overseer/deploy.rs
  - ../../src/self_deploy/source_prep.rs
---

# Deploy canary gate curation API reference

> **Status: implemented.** The curated gate constructor `canary_gates()` lives in
> [`src/self_relaunch/types.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs)
> next to the unchanged `default_gates()`. The Overseer's autonomous deploy
> canary is wired to `canary_gates()` at its actual gate-selection site,
> `prepare_build_and_verify_canary` in
> [`src/self_deploy/source_prep.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/source_prep.rs)
> (the sole deploy-path caller of `default_gates()`, reached via
> [`src/overseer/deploy.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy.rs)'s
> `build_and_verify`); the fail-closed recursion sentinel guarding it lives
> alongside the gate runner in
> [`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs).
> The change is **additive and non-breaking**: `default_gates()`,
> `RelaunchGate`, `GateResult`, and the `#4420` red-canary diagnostics fields are
> all untouched. References **#4469** (self-deploy frozen) and **#4470**
> (root-cause the red-canary `101`).

## Why this exists

The Overseer's guarded self-deploy path builds the merged-`main` binary and runs
it through a **canary** — a list of [`RelaunchGate`](#relaunchgate)s — before it
is allowed to replace the running binary. Until this change the deploy canary ran
the full [`default_gates()`](#default_gates-unchanged) list, which includes the
`UnitTest` gate. `UnitTest`'s runner shells out to `cargo test`.

Running `cargo test` **from inside a canary that is itself launched by the
overseer** is recursive: the canary process re-enters the same test suite (which
transitively exercises deploy/canary code paths), the nested build/test blows
past its budget, and the gate returns a **deterministic exit status `101`** (the
Rust test-harness failure code). Every overseer cycle therefore produced the same
red canary, the deploy gate refused (see
[red-canary diagnostics](./overseer-deploy-canary-diagnostics.md)), and
`DeployDrift` climbed monotonically while Simard ran a stale binary — the
crash-loop tracked in **#4469 / #4470**.

The fix does **not** disable or weaken any gate. It introduces a **curated,
recursion-free gate list** for the deploy canary that keeps every read-only,
local health check (`Smoke`, `GymBaseline`, `RpcHealth`) and omits the one gate
(`UnitTest`) whose runner recursively invokes the test binary. The full
`default_gates()` list — including `UnitTest` — is unchanged and still used by
CI and by any caller that wants the exhaustive suite.

## Data model (unchanged)

### `RelaunchGate`

The gate enum in
[`src/self_relaunch/types.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs)
is **unchanged**. Its `Display` values are the stable gate names surfaced in the
[red-canary diagnostics](./overseer-deploy-canary-diagnostics.md) `failing_gate`
attribute.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelaunchGate {
    Smoke,       // Display: "smoke"       — process starts, `--version` responds
    UnitTest,    // Display: "unit-test"   — shells `cargo test` (RECURSIVE in a canary)
    GymBaseline, // Display: "gym-baseline"— local coin-gym baseline, read-only
    RpcHealth,   // Display: "rpc-health"  — daemon RPC liveness probe
}
```

### `default_gates()` (unchanged)

```rust
/// The exhaustive relaunch gate list. UNCHANGED — still includes `UnitTest`.
/// Used by CI and any caller that wants the full suite. NOT used by the
/// overseer deploy canary (which would recurse on `UnitTest`).
pub fn default_gates() -> Vec<RelaunchGate> {
    vec![
        RelaunchGate::Smoke,
        RelaunchGate::UnitTest,
        RelaunchGate::GymBaseline,
        RelaunchGate::RpcHealth,
    ]
}
```

## `canary_gates()`

The new constructor. It is the **only** gate list the overseer deploy canary
runs.

```rust
/// The curated, recursion-free gate list for the OVERSEER DEPLOY CANARY.
///
/// Deliberately excludes `RelaunchGate::UnitTest`: its runner shells `cargo
/// test`, which — when executed inside a canary already spawned by the overseer
/// — re-enters the test suite recursively and returns a deterministic exit
/// `101` (#4469 / #4470). Every retained gate is read-only and local:
///
///   * `Smoke`        — the built binary starts and answers `--version`
///   * `GymBaseline`  — a local coin-gym baseline run (no deploy authority)
///   * `RpcHealth`    — daemon RPC liveness
///
/// Order is stable and asserted by test. Callers wanting the exhaustive suite
/// (CI, manual verification) keep using `default_gates()`.
pub fn canary_gates() -> Vec<RelaunchGate> {
    vec![
        RelaunchGate::Smoke,
        RelaunchGate::GymBaseline,
        RelaunchGate::RpcHealth,
    ]
}
```

### Contract

| Property | Guarantee |
| --- | --- |
| Excludes `UnitTest` | **Invariant** — asserted by a `types.rs` unit test. Adding `UnitTest` back is a test failure. |
| Every member is read-only / local | No gate in the list has deploy or write authority (SR-A4). |
| Stable order | `[Smoke, GymBaseline, RpcHealth]`, asserted by test — keeps `failing_gate` attribution deterministic. |
| No new inputs | No CLI flag, config key, or RPC toggles the list. It is a compile-time constant list. |

## Overseer deploy wiring

The switch is **one line**, and it lives at the point where the deploy path
actually selects its gate list —
[`prepare_build_and_verify_canary`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/source_prep.rs)
in `src/self_deploy/source_prep.rs` (~L675) — **not** in `deploy.rs`. The call
chain is:

```text
overseer::deploy::SharedTargetCanaryVerifier::build_and_verify(source, target_commit)
  └─ self_deploy::source_prep::prepare_build_and_verify_canary(source, target_commit, target_dir)
       └─ let gates = self_relaunch::default_gates();   // ← the actual selection site
       └─ self_relaunch::verify_canary(&candidate, &gates, &config)
```

`build_and_verify` in
[`src/overseer/deploy.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy.rs)
takes only `(source, target_commit)` — it does **not** receive a gate list — so
the curation must happen inside `prepare_build_and_verify_canary`:

```rust
// src/self_deploy/source_prep.rs, in prepare_build_and_verify_canary:
// Was: let gates = crate::self_relaunch::default_gates(); (recursed on UnitTest → exit 101)
let gates = crate::self_relaunch::canary_gates();
crate::self_relaunch::verify_canary(&candidate, &gates, &config)
```

> **Design note (correction to the spec's `files_to_change`).** The design
> specification lists `src/overseer/deploy.rs` as the wiring change site and
> omits `src/self_deploy/source_prep.rs`. That is inaccurate against the current
> code: `deploy.rs::build_and_verify` delegates gate selection to
> `prepare_build_and_verify_canary`, which is the sole caller of
> `default_gates()` on the deploy path. **`src/self_deploy/source_prep.rs` must
> be added to `files_to_change`, and the one-line switch made there.** An
> alternative — threading a `gates: &[RelaunchGate]` parameter up through
> `build_and_verify` so `deploy.rs` chooses — is possible but strictly larger
> and changes a trait signature; the minimal, non-breaking fix is the
> `source_prep.rs` swap.

Everything downstream is unchanged: `prepare_build_and_verify_canary` still
returns a `GateResult` list, the `#4420`
[red-canary diagnostics](./overseer-deploy-canary-diagnostics.md) still thread
the first failing gate into `CanaryResult.failing_gate` / `failing_detail`, and
a genuinely red gate still fails closed at the deploy gate.

## Fail-closed recursion sentinel

Excluding `UnitTest` from the list removes the *known* recursion source. To keep
a *future* regression from silently re-introducing it — e.g. a new gate whose
runner also shells `cargo test`, or a code path that calls the canary with
`default_gates()` — the gate runner in
[`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
carries a **recursion sentinel**:

- Before the deploy path invokes `verify_canary`, the deploy code
  (`prepare_build_and_verify_canary`) sets the environment marker
  `SIMARD_IN_DEPLOY_CANARY=1` for the current process. (The `UnitTest` gate's
  `cargo test` runs as a subprocess of this same process via
  `Command::new("cargo")`, so it inherits the marker.)
- The `UnitTest` gate runner checks that marker on entry. If it is set, the
  runner **refuses to shell `cargo test`** and returns a **failed**
  `GateResult` with a clear detail (`"unit-test gate refused inside deploy
  canary (recursion guard) — use canary_gates()"`) instead of recursing.

```rust
// In run_unit_test_gate, before spawning `cargo test`:
if std::env::var_os("SIMARD_IN_DEPLOY_CANARY").is_some() {
    return GateResult {
        gate: RelaunchGate::UnitTest,
        passed: false, // FAIL CLOSED — never silently green
        detail: "unit-test gate refused inside deploy canary \
                 (recursion guard) — use canary_gates()".into(),
    };
}
```

**Contract:** the sentinel **fails closed**. A canary that somehow runs
`UnitTest` produces a red gate (surfaced by the existing diagnostics) rather than
a recursive `cargo test` hang or exit `101`. It never converts a real failure to
green. The sentinel is defense-in-depth; the primary fix is that
`canary_gates()` simply never lists `UnitTest`.

## What an operator observes

**Before** — every overseer cycle, deterministically:

```
WARN overseer::deploy: self-deploy refused by deploy gate
    failing_gate=unit-test
    failing_detail="error: test failed, to rerun pass ... (exit code: 101)"
    refusal="red canary (gate unit-test: ... exit code: 101)"
```

`DeployDrift` climbs; the running binary stays 1–2 commits behind merged `main`
for cycle after cycle.

**After** — the canary runs `[smoke, gym-baseline, rpc-health]`, all green on a
healthy build, the deploy gate proceeds, the binary is swapped, and
`DeployDrift` returns to `0`:

```
INFO overseer::deploy: canary green (3/3 gates) — deploying target 9f2c1ab
INFO overseer::deploy: self-deploy complete — running_commit now 9f2c1ab
```

If a *real* regression reddens one of the curated gates, the deploy still
refuses and the [red-canary diagnostics](./overseer-deploy-canary-diagnostics.md)
name the failing gate exactly as before — detection is fully intact.

## Compatibility

- **Additive.** `canary_gates()` is a new function; `default_gates()`,
  `RelaunchGate`, and `GateResult` are byte-for-byte unchanged.
- **No schema drift.** The canary report shape, the `CanaryResult` fields, and
  the `simard self-health --json` output are unchanged.
- **No new inputs.** No CLI flag, config key, RPC, or operator "skip gate"
  control. The trust boundary is unchanged; the deploy gate still fails closed.
- **No `print`-family macros.** All emission is `tracing` structured key=value
  at ≥ INFO/WARN — no `print!` / `println!` / `eprintln!`, no `bridge` naming.

## Testing

| Test | Location | Asserts |
| --- | --- | --- |
| `canary_gates_excludes_unit_test` | `src/self_relaunch/types.rs` | The list never contains `RelaunchGate::UnitTest`. |
| `canary_gates_order_is_stable` | `src/self_relaunch/types.rs` | Exactly `[Smoke, GymBaseline, RpcHealth]` in order. |
| `deploy_canary_runs_curated_gates` | `src/self_relaunch/gates.rs` | The canary verifier is invoked with `canary_gates()`, and a healthy build passes with `3/3 gates` and no exit `101`. |
| `unit_test_gate_refuses_inside_canary` | `src/self_relaunch/gates.rs` | With `SIMARD_IN_DEPLOY_CANARY=1`, `run_unit_test_gate` returns a failed `GateResult` and never spawns `cargo test`. |

All tests are hermetic — no network, no live `gh`, no real deploy.

## See also

- [Overseer deploy red-canary diagnostics](./overseer-deploy-canary-diagnostics.md) — how the reddening gate name/detail is surfaced (unchanged).
- [Self-deploy API reference](./self-deploy-api.md) — the `GuardedDeployer`, `DeployRefusal`, and `evaluate_deploy_gate`.
- [Self-health quarantine classification API](./self-health-quarantine-classification-api.md) — the sibling fix that lets a healthy build clear quarantine so `DeployDrift` can drain (#4471).
- [Concept: reconcile and self-deploy](../concepts/reconcile-and-self-deploy.md) — where the deploy canary sits in the reconcile loop.
- [How-to: verify and roll back a self-deploy](../howto/verify-and-roll-back-a-self-deploy.md).
