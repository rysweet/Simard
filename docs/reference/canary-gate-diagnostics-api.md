---
title: Canary-gate diagnostics API reference
description: Reference for the RedCanaryDetail payload, the first_failure accessor over ordered gate results, the structured tracing fields emitted by every relaunch gate, the deploy-refusal and dual-channel notification surfacing, and the corrected candidate RPC-health probe.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/canary-gate-diagnostics.md
  - ./self-deploy-api.md
  - ./overseer-operator-notifications.md
  - ../howto/diagnose-a-red-canary-deploy-refusal.md
  - ../../src/self_relaunch/gates.rs
  - ../../src/self_relaunch/types.rs
  - ../../src/self_relaunch/canary.rs
  - ../../src/overseer/deploy.rs
  - ../../src/self_relaunch_semaphore/handoff.rs
---

# Canary-gate diagnostics API reference

> **Status: implemented.** The types, accessor, `tracing` fields, and the
> corrected `run_rpc_health_gate` probe below live in
> [`src/self_relaunch/`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
> and are surfaced on the deploy-refusal path in
> [`src/overseer/deploy.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy.rs).
> The additions are **additive and non-breaking**: existing verdict logic
> (`evaluate_deploy_gate`, `all_gates_passed`, `verify_canary`) is unchanged, and
> the canary is not weakened. For the rationale and the end-to-end flow, see the
> [canary-gate diagnostics concept](../concepts/canary-gate-diagnostics.md).

## Contents

- [Gate order](#gate-order)
- [`GateResult` and `first_failure`](#gateresult-and-first_failure)
- [`RedCanaryDetail`](#redcanarydetail)
- [`DeployRefusal::RedCanary` surfacing](#deployrefusalredcanary-surfacing)
- [Operator notification (invariant #2590)](#operator-notification-invariant-2590)
- [Structured tracing fields](#structured-tracing-fields)
- [Candidate RPC-health probe](#candidate-rpc-health-probe)
- [Telemetry hygiene contract](#telemetry-hygiene-contract)
- [Invariants preserved](#invariants-preserved)

## Gate order

The canary runs an **ordered** list of gates (`self_relaunch::types`). Order is
load-bearing: `first_failure` and `RedCanaryDetail` report the **first** gate
that fails in this order, so the reported culprit is deterministic.

```rust
pub fn default_gates() -> Vec<RelaunchGate> {
    vec![
        RelaunchGate::Smoke,       // simard --version
        RelaunchGate::UnitTest,    // cargo test against the candidate manifest
        RelaunchGate::GymBaseline, // simard gym list
        RelaunchGate::RpcHealth,   // candidate RPC health (see below)
    ]
}
```

`RelaunchGate` renders to a stable slug used everywhere the gate is named
(`Display`): `smoke`, `unit-test`, `gym-baseline`, `rpc-health`.

## `GateResult` and `first_failure`

`verify_canary` runs **every** gate (it does not short-circuit) and returns the
`GateResult`s in gate order. `first_failure` selects the first failing one.

```rust
#[derive(Clone, Debug)]
pub struct GateResult {
    pub gate: RelaunchGate,
    pub passed: bool,
    /// Human-readable outcome. Sanitized + length-bounded before it is surfaced.
    pub detail: String,
}

/// The first failing gate in gate order, or `None` when every gate passed.
/// Deterministic: identical result sets always name the same gate.
pub fn first_failure(results: &[GateResult]) -> Option<&GateResult>;
```

`first_failure` is a pure, read-only accessor over the existing results — it does
not re-run gates and does not alter `all_gates_passed`'s verdict.

## `RedCanaryDetail`

An additive payload that names the first failing gate and its sanitized detail.
It is `Default`-able so existing constructors and tests compile unchanged.

```rust
/// Diagnostic payload attached to a red-canary deploy refusal. Additive: a
/// `Default` value (no failing gate named) is equivalent to the prior,
/// detail-free refusal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RedCanaryDetail {
    /// Slug of the first failing gate in gate order (e.g. `"rpc-health"`).
    /// Empty when no specific gate is known.
    pub failed_gate: String,
    /// Sanitized, length-bounded detail from that gate's `GateResult`.
    pub detail: String,
}

impl RedCanaryDetail {
    /// Build from the first failing `GateResult` in an ordered slice.
    /// Returns `Default` (empty) when every gate passed.
    pub fn from_results(results: &[GateResult]) -> Self;

    /// One-line summary for the deploy notification / refusal `Display`, e.g.
    /// `` gate `rpc-health` failed: … ``. Returns the legacy
    /// `"one or more gates failed"` wording for a `Default` (empty) value.
    pub fn summary(&self) -> String;
}
```

## `DeployRefusal::RedCanary` surfacing

`DeployRefusal::RedCanary` carries the diagnostic. The variant remains a
red-canary refusal — only its payload is enriched.

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeployRefusal {
    NoOp,
    Rollback,
    /// The canary gates did not all pass. Carries the first-failing-gate detail.
    RedCanary(RedCanaryDetail),
    CrashLoop { churn: u64 },
}
```

> **Threading the detail through `DeployContext`.** Today
> [`DeployContext`](./self-deploy-api.md) exposes only `canary_passed: bool`, so
> `evaluate_deploy_gate` has no gate-level information to put in the refusal. To
> emit `RedCanary(RedCanaryDetail)` the context must additionally carry the
> diagnostic — e.g. a `red_canary_detail: RedCanaryDetail` field (populated from
> `RedCanaryDetail::from_results` at the call site that already owns the
> `GateResult`s). The field is additive and `Default`-able: an all-passing canary
> leaves it empty, and the boolean verdict (`canary_passed`) is unchanged.

Its `Display` names the gate when known, and falls back to the legacy wording
otherwise:

```text
red canary (gate `rpc-health` failed: rpc health failed (exit 1): connection refused)
red canary (one or more gates failed)   // Default payload — legacy wording
```

`evaluate_deploy_gate` is otherwise unchanged: it refuses when `canary_passed`
is false and proceeds when the full gate set passes.

## Operator notification (invariant #2590)

Every deploy attempt — success **and** refusal — fires the operator notification
on **both** channels (Signal + email). Invariant #2590 is preserved; the red-canary
notification now embeds the sanitized failing-gate detail, at **parity** across
both channels.

The existing [`OperatorNotification::deploy`](./overseer-operator-notifications.md)
constructor already takes a `gate_summary: &str`; the diagnostic is delivered by
folding the sanitized failing-gate detail into that summary — **no new
constructor and no new notification variant** are introduced.

```rust
// Signature (unchanged):
//   OperatorNotification::deploy(commit, previous, repo, gate_summary)
// On a red-canary refusal, gate_summary carries the sanitized RedCanaryDetail,
// so the same body (naming the failing gate + reason) reaches both channels.
let gate_summary = red_canary_detail.summary(); // "gate `rpc-health` failed: …"
let notification =
    OperatorNotification::deploy(target_commit, running_commit, repo, &gate_summary);
let report = notifier.notify(&notification);
debug_assert!(report.dispatched(), "deploy refusal must notify the operator");
```

The `dispatched()` invariant (at least one channel accepted the message) is
unchanged; the diagnostic does not add an early return or alter control flow.

## Structured tracing fields

Every gate emitter in `gates.rs` records the outcome as **structured `tracing`
fields**, never a formatted print. There are no `print!`/`println!`
statements — OTel-exported `tracing` only.

| Field | Type | Meaning |
| --- | --- | --- |
| `canary.gate` | string | Gate slug (`smoke`, `unit-test`, `gym-baseline`, `rpc-health`). |
| `canary.passed` | bool | Whether this gate passed. |
| `canary.detail` | string | Sanitized, length-bounded outcome detail. |

```rust
tracing::info!(
    canary.gate = %gate,
    canary.passed = result.passed,
    canary.detail = %sanitize(&result.detail),
    "canary gate evaluated"
);
```

Because the fields are attached to the span/event as **field values** (not
interpolated into the message), candidate stderr cannot inject log lines.

## Candidate RPC-health probe

`run_rpc_health_gate` validates the **candidate** binary's own RPC health, not
the shared daemon socket.

```rust
/// Verify RPC health of the *candidate* binary by having it answer a health
/// check on its own in-process RPC transport. Fail-closed: a probe error,
/// non-zero exit, or timeout scores the gate red.
fn run_rpc_health_gate(binary: &Path, config: &RelaunchConfig) -> GateResult;
```

> **Starting point.** The current gate runs
> `Command::new(binary).args(["probe", "rpc", …])`, but the operator CLI had **no
> `probe` command** — `dispatch_operator_cli` fell through to its `unsupported
> command` arm, so the candidate exited non-zero on every invocation and the
> `rpc-health` gate was red on every tick. The gate was validating a command that
> did not exist, not candidate health.

> **Additive precondition.** Brick 2 adds a `probe rpc --self-check
> [--timeout=SECS]` command. It drives the pre-existing `bridge.health` method
> against a fresh **in-process** `NativeRpcTransport` (the same dispatch the
> daemon registers) via `rpc_transport::self_check_rpc_health`, exits `0` only
> when the candidate reports healthy, and is otherwise fail-closed.
> `run_rpc_health_gate` invokes this command. This is additive: no existing
> command changes and the `bridge.health` handler is reused (no rename, no new
> "Bridge" identifier).

Contract:

- **Target = candidate.** The probe exercises the freshly built `binary`'s own
  RPC health path, never the shared daemon socket. A drifted live daemon can no
  longer redden the gate.
- **In-process, socket-free.** The self-check runs entirely in the candidate
  process; **no loopback port is bound** and no external `host:port` is dialed.
- **Bounded.** The probe is bounded by `RelaunchConfig::health_timeout`
  (default 30s), passed through as `--timeout`. An in-process dispatch cannot
  hang; a non-zero exit is a **red** result, not a stall.
- **Fail-closed.** Probe spawn error, non-zero exit, or timeout ⇒ `passed:
  false`. There is no default-pass or force-pass.
- **Self-cleaning by construction.** Nothing is bound or spawned, so there is no
  leftover socket or child process to clean up — even on the error path.

Unhealthy candidates are still refused; the change only stops a *healthy*
candidate from being falsely refused.

## Telemetry hygiene contract

Gate details are treated as untrusted candidate output before they are surfaced
anywhere (tracing field, `RedCanaryDetail`, or notification):

- Truncated and trimmed via the existing char-boundary-safe `truncate_output`
  (bounded to `≤ 512` chars) — safe on multi-byte UTF-8.
- Emitted as structured field values, never format-string-interpolated.
- No secrets, environment values, or home paths surfaced.
- Sanitized identically for both operator channels (dual-channel parity).

> **Implementation note.** `truncate_output` already exists in `gates.rs` but is
> currently **private** and applied to the `unit-test` detail only, at a 200-char
> bound; the `smoke`, `gym-baseline`, and `rpc-health` gates today emit the full
> trimmed child stderr. Delivering this contract requires applying the helper
> uniformly to every gate's `detail` and reconciling the bound to the intended
> `512`. This is the additive sanitization change Brick 1 introduces — it does
> not alter any gate's pass/fail verdict.

## Invariants preserved

| Invariant | Preserved by |
| --- | --- |
| Notify #2590 — both channels fire on every attempt incl. refusal | Diagnostic rides inside the existing `deploy` notification; no new early return. |
| `with_self_deploy_build_lock` serialization | Diagnostics run inside the existing build-lock scope; no bypass. |
| `SIMARD_GIT_HASH` / `version_advanced` integrity gate | Untouched; candidate build + integrity wiring unchanged. |
| No "Bridge" naming | Pre-existing `bridge.health` RPC untouched; no new Bridge identifiers. |
| `tracing` + OTel only | No `print!`/`println!`; all output is structured fields. |
| Verdict identical with/without diagnostics | `evaluate_deploy_gate` / `all_gates_passed` unchanged; diagnostics are side-channel. |

## See also

- [Canary-gate diagnostics concept](../concepts/canary-gate-diagnostics.md)
- [Self-deploy API reference](./self-deploy-api.md)
- [Overseer operator notifications](./overseer-operator-notifications.md)
- [How to diagnose a red-canary deploy refusal](../howto/diagnose-a-red-canary-deploy-refusal.md)
