---
title: "Reference: Deploy-gate canary unit-test stage"
description: >
  The contract for the self-deploy canary unit-test gate
  (run_unit_test_gate in src/self_relaunch/gates.rs): how the gate invokes the
  canary test suite, how a red canary (exit 101) blocks self-deploy, and the
  root-cause fix that cleared the recurring exit-101 red canary so the running
  daemon can self-deploy to merged main.
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./self-deploy-api.md
  - ./overseer-deploy-canary-diagnostics.md
  - ./typed-ooda-ledger-concurrency.md
  - ../howto/enable-autonomous-self-merge-canary.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../safe-self-update.md
  - ../../src/self_relaunch/gates.rs
  - ../../src/self_relaunch/types.rs
---

# Reference: Deploy-gate canary unit-test stage

> **Status: implemented (issues #4470, #4471, #4481, #4475).** Present-tense
> description of shipped behaviour. Primary source:
> [`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
> (`run_unit_test_gate`, `verify_canary`).
>
> This change root-causes and clears the recurring **red canary** — every
> self-deploy was failing the `deploy_gate` unit-test stage with
> `exit status: 101`, which blocked the running daemon from advancing to merged
> `main` (`simard status`: *"running binary is 1 commit(s) behind merged main —
> self-deploy required"*). The failing test lived in the typed-OODA concurrency
> surface, so the root fix is delivered by the
> [ledger concurrency hardening](./typed-ooda-ledger-concurrency.md); this page
> documents the gate contract and the canary-green resolution.

---

## The canary gate sequence

Before a freshly built candidate binary replaces the running daemon, the
self-deploy path runs it through a sequence of gates via
[`verify_canary`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs).
The sequence **does not short-circuit** — every gate runs so diagnostics report
all failures, not just the first:

| Gate (`RelaunchGate`) | What it proves |
|---|---|
| `Smoke` | The candidate binary starts and answers `--version`. |
| `UnitTest` | The canary test suite passes (`cargo test`). |
| `GymBaseline` | `gym list` succeeds against the candidate. |
| `RpcHealth` | The candidate answers an RPC health probe within `health_timeout`. |

A single failed gate produces a **red canary** and the self-deploy is aborted;
the running binary stays in place. This is the intended fail-closed posture:
**a red canary must never be papered over by disabling the gate.**

---

## The unit-test gate contract

`run_unit_test_gate(config: &RelaunchConfig)` shells out to `cargo test` with
**fixed arguments** (no `sh -c`, no dynamic interpolation of caller input):

```text
cargo test \
  --manifest-path <RelaunchConfig.manifest_dir>/Cargo.toml \
  --target-dir    <RelaunchConfig.canary_target_dir>
```

with `CARGO_BUILD_JOBS` set from
[`cargo_jobs`](https://github.com/rysweet/Simard/blob/main/src/cargo_jobs.rs).
The relevant [`RelaunchConfig`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs)
fields are:

| Field | Meaning |
|---|---|
| `manifest_dir` | Directory holding the candidate's `Cargo.toml` (default `.`). |
| `canary_target_dir` | Isolated, PID-scoped `--target-dir` under the temp dir, so the canary build never clobbers the live target. |
| `health_timeout` | Deadline for the RPC-health gate. |

Result mapping:

| `cargo test` outcome | `GateResult` |
|---|---|
| exit `0` | `passed: true`, detail `"all tests passed"`. |
| non-zero (e.g. `exit 101` = test failures) | `passed: false`, detail `"tests failed (exit <status>): <truncated stderr>"`. |
| failed to spawn | `passed: false`, detail `"cargo test failed to run: <err>"`. |

Captured `stderr` is truncated (200 chars) before it is logged, and only the
gate verdict and truncated detail are emitted through structured `tracing` /
OTel — never full test output, tokens, or approval payloads.

---

## Root cause of the recurring exit-101 canary

Exit status `101` is `cargo test`'s exit code for **test failures** (not a gate
or harness bug). Reproducing the canary suite locally with the same fixed
arguments surfaced the failing test in the typed-OODA persistence/lifecycle
surface: the same `database is locked` contention and reaper races described in
[typed-OODA ledger concurrency hardening](./typed-ooda-ledger-concurrency.md)
made the affected tests fail non-deterministically under the canary's parallel
test execution.

The resolution root-causes the defect rather than quarantining the symptom:

- The underlying concurrency defect is fixed in the ledger/reaper layer, so the
  previously-failing tests now pass deterministically.
- The gate itself is unchanged in posture — it is **not** disabled, weakened, or
  made non-blocking.
- Per issue #4471, deliberate quarantine remains available **only** for a test
  proven obsolete/wrong, applied narrowly with a justification comment citing
  #4471. It was not needed here.

The fix lands on a fresh, non-conflicting branch, superseding the stale
conflicting PRs #4480 / #4454 / #4436 / #4429 and coordinating with the
root-cause/quarantine/hardening issues #4470 / #4471 / #4481 / #4475.

---

## Verifying a green canary

After deploy, `simard status` no longer reports the running binary as behind
merged `main`, and the deploy log records `deploy_gate: green canary` instead of
the previous `red canary (gate unit-test: tests failed exit status: 101)`. To
reproduce the gate manually, see
[Enable the autonomous self-merge canary](../howto/enable-autonomous-self-merge-canary.md)
and
[Verify and roll back a self-deploy](../howto/verify-and-roll-back-a-self-deploy.md).
For deep canary telemetry see
[Overseer deploy-canary diagnostics](./overseer-deploy-canary-diagnostics.md).
