---
title: Self-deploy default canary gates
description: Reference for the default self-deploy canary gate set returned by self_relaunch::default_gates() — the three fast candidate-binary gates (Smoke, GymBaseline, RpcHealth) that verify a locally-built candidate boots and its core subsystems respond, why the full-suite UnitTest gate is NOT a default (CI owns full-suite verification), and how verify_canary / all_gates_passed remain count-agnostic over any gate slice.
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./canary-gate-convergence.md
  - ./overseer-deploy-canary-diagnostics.md
  - ./self-deploy-api.md
  - ./self-deploy-source-prep.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../../src/self_relaunch/types.rs
  - ../../src/self_relaunch/gates.rs
  - ../../.github/workflows/verify.yml
---

# Self-deploy default canary gates

> **Status: implemented.** `self_relaunch::default_gates()`
> ([`src/self_relaunch/types.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs))
> returns exactly three gates —
> `[Smoke, GymBaseline, RpcHealth]` — in that order. The full-suite
> `RelaunchGate::UnitTest` gate is **retained as an enum variant and
> implementation** (`run_unit_test_gate`) for explicit/manual verification, but
> is **not** part of the default self-deploy canary set. `verify_canary`,
> `all_gates_passed`, and the fail-closed no-short-circuit semantics are
> unchanged; they simply iterate over three gates instead of four.

## Summary

The self-deploy canary verifies that a **locally-built candidate binary** is
safe to promote onto the running host. The default gate set is deliberately
scoped to that host-specific job:

| Gate | Variant | What it verifies | Command shape |
| --- | --- | --- | --- |
| Smoke | `RelaunchGate::Smoke` | The candidate binary launches and reports its version. | `<candidate> --version` |
| GymBaseline | `RelaunchGate::GymBaseline` | A core subsystem enumerates without error. | `<candidate> gym list` |
| RpcHealth | `RelaunchGate::RpcHealth` | The RPC subsystem answers a health probe. | `<candidate> probe rpc --timeout <N>` |

Each gate exercises the **actual artifact that will ship** and is fast (seconds,
not minutes). Together they answer the deploy gate's one legitimate question:
*does this specific candidate binary boot and do its core subsystems respond on
this host?*

`RelaunchGate::UnitTest` — which shells out to the **full** `cargo test` suite
from source — is **not** in the default set. See
[Why UnitTest is not a default gate](#why-unittest-is-not-a-default-gate).

## API

### `default_gates()`

```rust
/// The default canary gate set for self-deploy.
///
/// Returns the three fast gates that verify the locally-built *candidate
/// binary* is safe to promote on this host: `Smoke` (the binary launches and
/// reports its version), `GymBaseline` (a core subsystem enumerates), and
/// `RpcHealth` (the RPC subsystem answers a health probe). Each exercises the
/// exact artifact that will ship and completes in seconds.
///
/// `RelaunchGate::UnitTest` is deliberately **excluded** from this default set.
/// Full-suite verification is owned by CI: `.github/workflows/verify.yml`
/// (a required status check on both `push` and `pull_request`) runs
/// `cargo test --all-features --locked --no-fail-fast` on clean, dedicated
/// runners, so any commit that reaches a deploy target (`main`) already has a
/// green full test suite. Re-running the full `cargo test` suite from source in
/// the self-deploy canary is redundant with that CI gate, and doing so under
/// production host load (the deploy host runs many concurrent recipes and is
/// heavily oversubscribed) produced 30+ minute runtimes and load-induced false
/// reds that froze self-deploy — with zero genuine assertion failures. The
/// `UnitTest` variant and `run_unit_test_gate` remain available for explicit or
/// manual verification; they are simply not run automatically on every deploy.
pub fn default_gates() -> Vec<RelaunchGate> {
    vec![
        RelaunchGate::Smoke,
        RelaunchGate::GymBaseline,
        RelaunchGate::RpcHealth,
    ]
}
```

**Contract**

- Returns exactly three gates.
- Order is stable: `Smoke → GymBaseline → RpcHealth`.
- `RelaunchGate::UnitTest` is **never** an element of the returned vec.

### `RelaunchGate::UnitTest` (retained, non-default)

The variant and its implementation are unchanged and still available:

```rust
// Still valid — Display, equality, and the full-suite implementation are kept.
assert_eq!(RelaunchGate::UnitTest.to_string(), "unit-test");

// Explicit/manual verification can still include it in a custom gate slice:
let gates = [RelaunchGate::Smoke, RelaunchGate::UnitTest];
let results = verify_canary(&candidate, &gates, &config);
```

`run_unit_test_gate` continues to shell out to the full suite:

```text
cargo test --manifest-path <manifest_dir>/Cargo.toml --target-dir <canary_target_dir>
```

This remains useful for a deliberate, operator-initiated full verification on a
lightly loaded host. It is only removed from the **default** self-deploy path.

### `verify_canary` / `all_gates_passed` (count-agnostic)

Neither function assumes a gate count. `verify_canary` iterates the slice it is
handed and runs every gate to completion without short-circuit;
`all_gates_passed` requires every result in the slice to pass. With the
three-gate default they operate identically to before — the promotion decision
still fails closed on any red gate:

```rust
let gates = default_gates();                 // [Smoke, GymBaseline, RpcHealth]
let results = verify_canary(&candidate, &gates, &config);
if all_gates_passed(&results) {
    // promote
}
```

## Why UnitTest is not a default gate

The full-suite `UnitTest` gate was removed from the default set because it was
**redundant** with CI and **load-flaky** on the deploy host.

### Full-suite verification is owned by CI

`.github/workflows/verify.yml:152` runs, on every `push` **and**
`pull_request`, on clean dedicated runners:

```yaml
cargo test --all-features --locked --no-fail-fast \
  -- --skip install_packages_runs_and_self_installs
```

(The single `--skip` excludes only the multi-minute self-install test, which the
dedicated `install-real` job covers; the rest of the suite runs in full.)

Because that job is a required status check with branch protection on `main`,
**any commit that becomes a deploy target already has a verified-green full test
suite**. Re-running the same suite in the self-deploy canary verifies nothing
new about the source.

### Re-running it under host load froze self-deploy

`run_unit_test_gate` shells out to the entire `cargo test` suite (lib + **all**
integration binaries) from source. The code already documented this as
pathological — see the comment in
[`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
(`verify_canary_runs_all_gates_without_short_circuit`), which curates a gate
list that "excludes `RelaunchGate::UnitTest`, which would recursively invoke
`cargo test` and run for 30+ minutes."

On the production deploy host — heavily oversubscribed by many concurrent
engineer recipes — a faithful reproduction of the exact gate command produced:

- **zero** genuine assertion failures, but
- individual integration test binaries taking **538s and 618s**, with dozens of
  tests exceeding 60s.

Under that load the gate either timed out or a load-sensitive deadline test
panicked (exit 101) — a **false red on code that is actually correct**. Because
the self-deploy loop re-queues an identical refusal on every tick, this froze
autonomous self-deploy at a stale version for **30+ hours** with
`failing_gate="unit-test"` and truncated spinner-noise detail.

### The three retained gates still guard the binary

Removing `UnitTest` does **not** weaken the deploy boundary. The retained gates
each verify the **locally-built candidate binary** — which CI does *not* build —
so a corrupt, mis-built, or tampered artifact still reddens the canary:

- **Smoke** — the candidate launches and reports its version.
- **GymBaseline** — a core subsystem (`gym list`) enumerates without error.
- **RpcHealth** — the RPC subsystem answers a health probe.

This is the deploy gate's legitimate, host-specific job. Source correctness is
CI's job; artifact-boots-on-this-host is the canary's job. The two no longer
overlap.

## Fail-closed invariants (preserved)

- **Canary is the authorization boundary.** The default gates still gate
  promotion and still fail closed. An unhealthy candidate reddens exactly as
  before.
- **No short-circuit.** All (three) gates run to completion so every verdict is
  observable; `all_gates_passed` still requires every gate to pass.
- **No "skip gate" control.** There is no new CLI flag, RPC, or config key to
  disable a gate. The change is a narrower **default set**, not a runtime bypass.
- **Capability preserved.** `RelaunchGate::UnitTest` and `run_unit_test_gate`
  remain for explicit/manual full verification (defense-in-depth on a lightly
  loaded host).

## Compatibility

- **Signatures unchanged.** `default_gates`, `verify_canary`,
  `all_gates_passed`, `RelaunchGate`, and `GateResult` keep their signatures.
- **Enum unchanged.** No variant is removed; `RelaunchGate::UnitTest` and its
  `"unit-test"` `Display` are retained.
- **Callers unaffected.** `verify_canary` and `all_gates_passed` are
  count-agnostic, so the overseer deploy path
  ([`src/overseer/deploy.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy.rs))
  and the source-prep canary wiring
  ([`src/self_deploy/source_prep.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/source_prep.rs))
  require no logic change — they simply verify three gates instead of four.

## Tests

The default-gate contract is locked by tests in three surfaces, each asserting
the exact three-gate ordered set **and** the explicit absence of `UnitTest`:

| Test surface | Asserts |
| --- | --- |
| [`src/self_relaunch/types.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs) (`default_gates_has_three`) | `default_gates().len() == 3`; `[Smoke, GymBaseline, RpcHealth]` in order; `!default_gates().contains(&RelaunchGate::UnitTest)`. |
| [`src/self_relaunch/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/mod.rs) (`default_gates_returns_three_in_order`) | Same three-gate order; negative `UnitTest` assertion. |
| [`tests/self_improve.rs`](https://github.com/rysweet/Simard/blob/main/tests/self_improve.rs) (`default_gates_is_ordered`) | Same three-gate order; negative `UnitTest` assertion. |

No test is weakened, re-timed, serialized, or deleted. Full-suite coverage
remains enforced by CI (`verify.yml`).

## See also

- [Canary gate isolation and self-deploy convergence](./canary-gate-convergence.md) —
  per-gate spans, the `canary_env` allow-list, and gate env discipline.
- [Overseer deploy red-canary diagnostics](./overseer-deploy-canary-diagnostics.md) —
  the `failing_gate` / `failing_detail` signal that named the frozen gate.
- [Reconcile and self-deploy](../concepts/reconcile-and-self-deploy.md) — where
  the canary gates sit in the end-to-end self-deploy flow.
- [Self-deploy API reference](./self-deploy-api.md) — the guarded deployer and
  the promotion path the gates authorize.
