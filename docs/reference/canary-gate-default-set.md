---
title: Canary default gate set — the local full-suite unit-test gate is off the deploy hot path
description: Reference for the self-deploy crash-loop repair (#4619) that removes the redundant, load-flaky local full-suite `unit-test` canary from `default_gates()`, leaving the deploy hot path as `Smoke -> GymBaseline -> RpcHealth`. The `RelaunchGate::UnitTest` variant and `run_unit_test_gate` are retained for explicit/manual verification; the full test suite stays covered by the green GitHub `verify` CI (full suite on every push and PR). The fail-closed refusal invariant (#4420) is preserved — a red canary is still fatal, never retried.
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./canary-gate-convergence.md
  - ./overseer-deploy-canary-diagnostics.md
  - ./self-deploy-api.md
  - ./overseer-tick-self-healing.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../../src/self_relaunch/types.rs
  - ../../src/self_relaunch/gates.rs
  - ../../src/self_relaunch/mod.rs
  - ../../.github/workflows/verify.yml
---

# Canary default gate set

> **Status: implemented.** `default_gates()`
> ([`src/self_relaunch/types.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs))
> returns the three-gate deploy hot path `Smoke -> GymBaseline -> RpcHealth`.
> The local full-suite `unit-test` canary is **no longer in the default set**.
> The change is **additive and non-breaking**: the `RelaunchGate::UnitTest`
> variant, its `"unit-test"` `Display` string, and the `run_unit_test_gate`
> full-suite runner are all retained for explicit/manual verification;
> `verify_canary`, `all_gates_passed`, and `default_gates` keep their
> signatures. The full test suite remains gated — by GitHub
> [`verify.yml`](https://github.com/rysweet/Simard/blob/main/.github/workflows/verify.yml),
> which runs `cargo test --all-features --locked --no-fail-fast` on
> clean runners on every push and PR. (`release.yml` triggers on push to `main`
> and only builds and signs the artifact via `cargo build --release --locked`; it
> does not re-run the test suite.)

## Why this exists

The self-deploy loop was **crash-looping**. On every Overseer deploy tick the
guarded gate refused with:

```
deploy_gate: red canary (gate unit-test: tests failed exit status: 101)
```

The running binary froze at `0.36.0` / commit `7d0964ffe4`, twelve commits
behind merged `main`, after ~20 red-canary refusals in ~6h (issues #4619,
#4618, #4617, #4609).

The reddening was **not a real regression.** GitHub default-branch CI (`verify`
+ `release`) was **green** for the same commits. Only the *local* full-suite
`unit-test` canary — the one gate that shells out to run the entire `cargo test`
suite inside the deploy tick — reddened, and it reddened **flakily under CPU
oversubscription**: the deploy host was carrying a load average of
`56.95 / 75.89 / 92.29`. Under that contention the full suite intermittently
exits `101` (a test process killed / timed out / resource-exhausted), not
because the candidate is unhealthy but because the host cannot schedule the
suite deterministically.

That produced a deterministic refusal loop: a healthy candidate reddened the
same gate every tick, the fail-closed classifier (correctly) never retried it as
transient, and the self-deploy loop re-queued the identical refusal without ever
advancing past the stuck target SHA.

### The fix: remove the redundant gate from the default set

The local full-suite `unit-test` canary was **redundant**. GitHub `verify.yml`
already runs the entire test suite (`cargo test --all-features --locked
--no-fail-fast`) on clean, isolated runners on every push and PR — and it was
green. Re-running the same full suite a second time, inside the deploy tick, on
a load-saturated host, added **no signal** and introduced a flaky failure mode
on the deploy hot path.

This feature therefore **removes `RelaunchGate::UnitTest` from
`default_gates()`**. The deploy hot path becomes the three fast, deploy-shape,
deterministic gates that GitHub CI *cannot* cover because they exercise the
actual built binary against the live host:

- `smoke` — the candidate binary starts and answers.
- `gym-baseline` — the candidate meets the gym baseline.
- `rpc-health` — the candidate's RPC endpoint is live (`binary probe rpc
  --timeout N`).

The full test suite stays gated — by GitHub CI, on hardware built for it —
rather than by a duplicate local run on a contended deploy host.

> **This is the "remove the redundant check" repair, not "weaken the check."**
> The full test suite is still a hard merge gate on GitHub. Nothing that
> could ship an untested regression is relaxed: an untested change cannot reach
> `main`, and the three retained canary gates still fail closed on an unhealthy
> candidate. Only the *second, redundant, flaky* execution of an
> already-green suite is taken off the deploy hot path.

## What changed

1. **`default_gates()` returns three gates.** The default deploy set is
   `[Smoke, GymBaseline, RpcHealth]`. `UnitTest` is no longer present.
2. **`UnitTest` is retained for explicit verification.** The
   `RelaunchGate::UnitTest` enum variant, its `"unit-test"` `Display` string,
   and `run_unit_test_gate` are unchanged — an operator or test can still run
   the full-suite gate explicitly by passing `RelaunchGate::UnitTest` to
   `verify_canary`.
3. **No retry, no reclassification.** The #4420 fail-closed invariant is
   untouched: a red canary is still fatal and is **never** retried as transient.
   The crash-loop is fixed by removing the flaky *input*, not by making a red
   canary retryable.
4. **Docs and gate-sequence comments updated** from the four-gate sequence to
   the three-gate hot path with the CI-redundancy rationale.

## API

### `default_gates()` — the deploy hot path

```rust
/// The gates run on every self-deploy tick, in order, without short-circuit.
///
/// The local full-suite `unit-test` canary is intentionally NOT here: the full
/// suite is already gated by GitHub `verify` CI on clean runners, and
/// re-running it inside the deploy tick on a load-saturated host added no signal
/// while introducing a flaky exit-101 crash-loop (#4619). Use
/// `RelaunchGate::UnitTest` explicitly if you want the full-suite gate on demand.
pub fn default_gates() -> Vec<RelaunchGate> {
    vec![
        RelaunchGate::Smoke,
        RelaunchGate::GymBaseline,
        RelaunchGate::RpcHealth,
    ]
}
```

| Index | Gate | Covers | Why on the hot path |
| --- | --- | --- | --- |
| `0` | `RelaunchGate::Smoke` | candidate binary starts and answers | binary-level, host-specific — CI cannot run the deployed artifact |
| `1` | `RelaunchGate::GymBaseline` | candidate meets the gym baseline | behavioral baseline of the built binary |
| `2` | `RelaunchGate::RpcHealth` | candidate RPC endpoint is live | live-host readiness of the actual candidate |

`RelaunchGate::UnitTest` (`Display` → `"unit-test"`) is **still a valid variant**
and `run_unit_test_gate` is **still callable** — it is simply not in the default
set.

### `RelaunchGate` — unchanged variant set (non-breaking)

The enum is unchanged; all four variants still exist so existing code, tests,
and explicit callers keep compiling:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelaunchGate {
    Smoke,
    UnitTest,   // retained — off the default set, available for explicit runs
    GymBaseline,
    RpcHealth,
}
```

`RelaunchGate::UnitTest.to_string() == "unit-test"` still holds.

### `verify_canary` — signature and semantics unchanged

`verify_canary(binary, gates, config)` still runs the supplied `gates` slice in
order to completion **without short-circuit**, and `all_gates_passed(&results)`
still requires every gate to pass. Only the *default* slice handed in by the
self-deploy path (`default_gates()`) changed length; the function contract did
not.

## Usage

### Default self-deploy path (three gates, automatic)

No caller action is required. The self-deploy / `DeployDrift` path calls
`default_gates()` and therefore runs the three-gate hot path automatically:

```rust
let gates = crate::self_relaunch::default_gates(); // [Smoke, GymBaseline, RpcHealth]
let results = verify_canary(&candidate_binary, &gates, &config)?;
if all_gates_passed(&results) {
    // promote / handover
}
```

### Explicit full-suite verification (opt-in `unit-test`)

When you *want* the full local suite as part of a canary — for example a manual
pre-deploy confidence check on a quiescent host — pass `UnitTest` explicitly:

```rust
use crate::self_relaunch::{RelaunchGate, verify_canary, all_gates_passed, default_gates};

// Three-gate hot path PLUS the full-suite gate, on demand.
let mut gates = default_gates();
gates.insert(1, RelaunchGate::UnitTest); // Smoke -> UnitTest -> GymBaseline -> RpcHealth

let results = verify_canary(&candidate_binary, &gates, &config)?;
assert!(all_gates_passed(&results));
```

Because `verify_canary` runs to completion without short-circuit, every gate's
verdict — including the explicit `unit-test` gate — is observable on a single
run via the per-gate `self_relaunch::gate` tracing spans (see
[canary-gate-convergence](./canary-gate-convergence.md)).

## Configuration

There are **no new operator inputs** — no CLI flags, RPC methods, config keys,
or environment variables are added, and none are required to get the fix. The
default set is a static `Vec` literal in `default_gates()`; changing which gates
run on the hot path is a source change, deliberately, so the deploy trust
boundary stays code-reviewed rather than operator-tunable at runtime.

| Surface | Before (#4619) | After (#4619) |
| --- | --- | --- |
| `default_gates()` | `[Smoke, UnitTest, GymBaseline, RpcHealth]` | `[Smoke, GymBaseline, RpcHealth]` |
| `RelaunchGate::UnitTest` | present, in default set | present, **not** in default set |
| `run_unit_test_gate` | on hot path | retained, explicit-call only |
| Full test suite coverage | GitHub CI **and** local canary (duplicate) | GitHub `verify` CI |
| Fail-closed refusal (#4420) | red canary fatal, no retry | **unchanged** — red canary fatal, no retry |
| New operator knobs | — | **none** |

### Where the full suite is gated now

The full suite is not skipped — it moved off the deploy hot path onto CI, where
it belongs. GitHub
[`verify.yml`](https://github.com/rysweet/Simard/blob/main/.github/workflows/verify.yml)
runs it on clean runners on every push and PR:

```yaml
# .github/workflows/verify.yml (illustrative — see the real step for exact flags)
- name: Run cargo test (streamed, captured for artifact)
  run: |
    # One long-running install test is skipped here and covered by the
    # dedicated `install-real` job in the same workflow.
    cargo test --all-features --locked --no-fail-fast \
      -- --skip install_packages_runs_and_self_installs
```

(`release.yml` runs on push to `main` and only builds + signs the artifact via
`cargo build --release --locked`; it does not run the test suite.)

An untested change cannot reach `main`, so the deploy candidate built from
`main` is already suite-verified before the canary ever runs.

## Behavior

### Convergence

With the flaky full-suite gate off the hot path, a healthy candidate now
produces a green canary from the three deterministic gates. The guarded deploy
gate stops returning `DeployRefusal::RedCanary`, the swap proceeds, and the next
drift observation sees `DeployDrift == 0`. The self-deploy loop advances past
the previously stuck target SHA instead of re-queuing an identical
`exit status: 101` refusal. No loop, requeue, or drift logic changed — the loop
was already correct; it was simply being handed a flaky red every tick.

### The Overseer deploy-gate verdict

The Overseer deploy gate's canary condition
([`prompt_assets/simard/overseer/deploy_gate.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/overseer/deploy_gate.md))
now reads as **Smoke + GymBaseline + rpc-health** per `self_relaunch::default_gates`.
The gate still requires `all_gates_passed(canary_gate_results)` and still refuses
on a red canary — only the *membership* of the default set changed.

## Fail-closed invariants (preserved)

None of the deploy safety rails is relaxed by this change:

- **Canary is still the authorization boundary.** The three retained gates gate
  promotion and still fail closed. An unhealthy candidate (dead binary, failed
  baseline, unreachable RPC) reddens and is refused exactly as before.
- **No short-circuit.** All gates in the supplied set run to completion, so every
  verdict is observable; `all_gates_passed` still requires every gate to pass.
- **A red canary is still fatal, never retried (#4420).** The `is_transient`
  self-healing classifier
  ([overseer-tick-self-healing](./overseer-tick-self-healing.md)) still treats a
  `deploy_gate` / `target_canary` failure as **non-transient**. The crash-loop
  is fixed by removing the flaky *input*, not by making a genuine red retryable —
  so a real regression still stops the deploy.
- **Full suite still gated.** The full test suite remains a hard gate on GitHub
  `verify` (push + PR); it is not skipped, only relocated off the deploy hot path.
- **No new operator inputs.** No CLI flags, RPC, config keys, or "skip gate"
  controls. The trust boundary is unchanged; the default set is code-reviewed.

## Regression tests

The change ships assertions locking the new three-gate default and proving the
removed gate is genuinely gone from the default set (not merely reordered):

| Test surface | Asserts |
| --- | --- |
| [`src/self_relaunch/types.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs) (unit) | `default_gates().len() == 3`; `gates[0] == Smoke`, `gates[2] == RpcHealth`; **`!default_gates().contains(&RelaunchGate::UnitTest)`**. |
| [`src/self_relaunch/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/mod.rs) (unit) | Ordered three-gate sequence `Smoke, GymBaseline, RpcHealth` + negative `!contains(UnitTest)`. `RelaunchGate::UnitTest.to_string() == "unit-test"` still holds (variant retained). |
| [`tests/self_improve.rs`](https://github.com/rysweet/Simard/blob/main/tests/self_improve.rs) (integration) | `default_gates_is_ordered`: `gates[1] == GymBaseline`, `gates[2] == RpcHealth`, `len == 3`, `!contains(UnitTest)`. `GateResult`-based tests that reference `UnitTest` explicitly remain intact (variant still valid). |

The negative `!contains(UnitTest)` assertion is deliberate: it makes a future
accidental re-add of the flaky gate to the default set red on CI, so the
crash-loop cannot silently regress.

## Compatibility

- **Non-breaking.** `RelaunchGate` keeps all four variants; `default_gates`,
  `verify_canary`, `all_gates_passed`, and `run_unit_test_gate` keep their
  signatures. Only the *contents* of the `default_gates()` `Vec` changed.
- **Explicit callers unaffected.** Any code passing `RelaunchGate::UnitTest` to
  `verify_canary` still compiles and runs the full-suite gate on demand.
- **No `print`-family macros.** No new emission is added; the retained per-gate
  observability stays `tracing`/OTel structured (see
  [canary-gate-convergence](./canary-gate-convergence.md)). No `print!` /
  `println!` / `eprintln!`.
- **No `Bridge` naming.** No new identifiers introduced; the
  [no-Bridge-naming guard](./no-bridge-naming-guard.md) is respected.
- **PRD preserved.** Additive/non-breaking; no PRD behavior removed.

## See also

- [Canary gate isolation and self-deploy convergence](./canary-gate-convergence.md) —
  the #4440 per-gate tracing spans, `scrub_gate_env` env discipline, and the
  no-short-circuit gate sequence this default-set change runs within.
- [Overseer deploy red-canary diagnostics](./overseer-deploy-canary-diagnostics.md) —
  the #4420 observability (`failing_gate` / `failing_detail`, `refusal_reason`,
  the `overseer::deploy` WARN, the `is_transient` fatal-refusal guard preserved
  here).
- [How to converge a stuck red-canary self-deploy](../howto/converge-a-stuck-red-canary-self-deploy.md) —
  the operator runbook for diagnosing and confirming convergence.
- [Self-deploy API reference](./self-deploy-api.md) — the `GuardedDeployer`,
  `DeployRefusal`, and the `OrchestratedBinaryDeployer` swap path the green
  three-gate canary unblocks.
