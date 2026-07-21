---
title: Self-deploy canary gate outcomes API reference
description: Reference for the additive Skipped gate outcome (GateResult.skipped and the pass/fail/skip constructors with the skipped ⇒ passed invariant), the fail-closed endpoint_absent predicate that lets only positively-detected absent-endpoint probe gates skip, all_gates_passed's treatment of skips, and the per-gate evidence (failing_gate / failing_detail / skipped_gates) threaded from TargetCanaryReport through CanaryResult into the operator deploy-refused notification.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/self-deploy-canary-gate-skip-and-diagnosability.md
  - ../concepts/reconcile-and-self-deploy.md
  - ./self-deploy-api.md
  - ./self-deploy-source-prep.md
  - ./overseer-operator-notifications.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../../src/self_relaunch/types.rs
  - ../../src/self_relaunch/gates.rs
  - ../../src/overseer/deploy.rs
---

# Self-deploy canary gate outcomes API reference

> **Status: implemented.** The `GateResult` `skipped` field and its
> `pass`/`fail`/`skip` constructors live in
> [`src/self_relaunch/types.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs);
> the `endpoint_absent` predicate and the probe-gate skip branches live in
> [`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs);
> the per-gate evidence on `TargetCanaryReport` / `CanaryResult` and its wiring
> into the deploy refusal live in
> [`src/overseer/deploy.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy.rs).
> These are **additive fields only** on existing types, preserving their current
> derives and test assertions: `GateResult` keeps its `Clone, Debug` derives (it
> has no `PartialEq`), while `TargetCanaryReport` and `CanaryResult` keep their
> `Clone, Debug, PartialEq, Eq` derives. For the
> rationale and flow, see
> [self-deploy canary gate skip-on-absent-endpoint and per-gate diagnosability](../concepts/self-deploy-canary-gate-skip-and-diagnosability.md).

This reference specifies the typed surface that lets the self-deploy canary
distinguish a *genuinely failing* gate from a probe gate whose *live endpoint is
absent* in an isolated fresh-build canary, and that names the failing gate in the
operator refusal. It extends the canary described in the
[self-deploy API reference](./self-deploy-api.md).

## Contents

- [`GateResult` (the `Skipped` outcome)](#gateresult-the-skipped-outcome)
- [`GateResult` constructors](#gateresult-constructors)
- [`all_gates_passed` and skips](#all_gates_passed-and-skips)
- [`endpoint_absent` predicate](#endpoint_absent-predicate)
- [Which gates may skip](#which-gates-may-skip)
- [Per-gate evidence: `TargetCanaryReport`](#per-gate-evidence-targetcanaryreport)
- [Per-gate evidence: `CanaryResult`](#per-gate-evidence-canaryresult)
- [Operator refusal detail](#operator-refusal-detail)
- [Configuration](#configuration)
- [Tests that are security controls](#tests-that-are-security-controls)

## `GateResult` (the `Skipped` outcome)

`GateResult` gains one additive field, `skipped`, governed by a hard invariant.

```rust
#[derive(Clone, Debug)]
pub struct GateResult {
    pub gate: RelaunchGate,
    pub passed: bool,
    /// True iff this gate did not run because its required live endpoint is
    /// legitimately absent in the isolated canary. INVARIANT: `skipped` implies
    /// `passed == true` — a skipped gate is never a failing gate.
    pub skipped: bool,
    pub detail: String,
}
```

**Invariant (enforced by the constructors and a `debug_assert!`):**

> `skipped == true` ⇒ `passed == true`.

Because every existing consumer reads `.passed`, and a skipped result is always
`passed: true`, a skip is automatically counted as **non-failing** everywhere
without changing those read-sites. This is why the field is a `bool` rather than
a new `GateOutcome` enum: it keeps the change additive across the existing
`.passed` read-sites. `GateResult` currently derives only `Clone, Debug` (it has
no `PartialEq`), and its unit tests assert field-by-field (`gate` / `passed` /
`detail`); the additive `skipped` field preserves those derives and assertions.

`Display` renders a skipped gate distinctly:

```text
[PASS] smoke: version: 1.4.2
[FAIL] rpc-health: rpc health failed (exit 1): connection refused
[SKIP] rpc-health: endpoint absent in isolated canary (no daemon) — skipped
```

## `GateResult` constructors

Gates return one of three constructors instead of building the struct inline.
Each encodes intent and upholds the invariant.

```rust
impl GateResult {
    /// The gate ran and passed.
    pub fn pass(gate: RelaunchGate, detail: impl Into<String>) -> Self;

    /// The gate ran and FAILED — the canary is red.
    pub fn fail(gate: RelaunchGate, detail: impl Into<String>) -> Self;

    /// The gate's required live endpoint is absent in this isolated canary.
    /// Non-failing: sets `passed: true, skipped: true`. Use ONLY when
    /// `endpoint_absent(..)` proved the endpoint is unreachable — never for a
    /// reachable-but-unhealthy endpoint or an unknown outcome.
    pub fn skip(gate: RelaunchGate, detail: impl Into<String>) -> Self;
}
```

| Constructor | `passed` | `skipped` | Effect on canary verdict |
| --- | --- | --- | --- |
| `pass` | `true` | `false` | non-failing |
| `fail` | `false` | `false` | **red** |
| `skip` | `true` | `true` | non-failing |

## `all_gates_passed` and skips

`all_gates_passed` is unchanged in shape and treats a skip as non-failing by
construction (a skipped gate is `passed: true`):

```rust
/// The canary is green iff no gate failed. A `Skipped` gate counts as
/// non-failing because `skip()` sets `passed: true`.
pub fn all_gates_passed(results: &[GateResult]) -> bool {
    results.iter().all(|r| r.passed)
}
```

## `endpoint_absent` predicate

A single, centralized predicate decides whether a probe gate is allowed to skip.
It is **fail-closed**: only a positively-recognized absence signal returns
`true`; every other result returns `false`.

The probe gates do **not** have a structured result type today — they shell out
with [`std::process::Command`] and observe only a
`Result<std::process::Output, std::io::Error>` (an exit status + stderr, or a
spawn error). `endpoint_absent` therefore operates on that real surface:

```rust
/// True iff the probe result positively proves the target endpoint is ABSENT
/// (no daemon listening / connection refused) — the only condition under which a
/// probe gate may `skip()`. FAIL-CLOSED: any result not positively matched as an
/// absence signal (a non-zero exit that is not recognized as absence, a spawn
/// error, or an indeterminate outcome) returns `false`, so the gate FAILS and
/// the canary stays red. Never widen the `true` path without a security test.
fn endpoint_absent(probe: &std::io::Result<std::process::Output>) -> bool {
    /// `sysexits.h` EX_UNAVAILABLE — the service/endpoint is unavailable.
    const EX_UNAVAILABLE: i32 = 69;
    /// Unambiguous "no listener" signals — each positively indicates that a
    /// connection attempt reached no daemon, so it stands on its own.
    ///
    /// NOTE (SR-4 hardening): "connection reset" (ECONNRESET) is deliberately
    /// EXCLUDED. A reset means the peer *was* reachable — it accepted the
    /// connection and then aborted mid-exchange, the canonical symptom of a
    /// daemon that is present-but-crashing while servicing the probe. That is
    /// the reachable-but-unhealthy case the contract says must RED.
    const CONNECTION_SIGNALS: [&str; 3] = ["connection refused", "no daemon", "could not connect"];
    /// Ambiguous on its own; only counts as absence alongside a socket marker.
    const ENOENT: &str = "no such file or directory";
    /// Markers that scope an ENOENT to a *socket* (endpoint) rather than an
    /// arbitrary file. Simard's RPC socket is `<state_root>/memory.sock`, so
    /// only the precise `.sock` suffix qualifies (SR-4): the bare word "socket"
    /// would misclassify unrelated missing files (e.g. a missing "websocket"
    /// module/config) as endpoint absence.
    const SOCKET_MARKERS: [&str; 1] = [".sock"];

    let output = match probe {
        Ok(output) => output,
        // Spawn error: the canary binary itself could not be executed. That is a
        // build/binary defect, NOT endpoint absence → fail closed.
        Err(_) => return false,
    };
    // A healthy (exit 0) probe is a pass, not an absence signal.
    if output.status.success() {
        return false;
    }
    // Primary, fail-closed signal: the dedicated EX_UNAVAILABLE exit code.
    if output.status.code() == Some(EX_UNAVAILABLE) {
        return true;
    }
    // Fallback: a bounded, allow-listed absence phrase on (lower-cased) stderr.
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    if CONNECTION_SIGNALS.iter().any(|sig| stderr.contains(sig)) {
        return true;
    }
    // ENOENT alone is not enough: require a `.sock` marker so a missing socket
    // (endpoint absent) skips, but a missing file (real failure) fails closed.
    stderr.contains(ENOENT) && SOCKET_MARKERS.iter().any(|m| stderr.contains(m))
}
```

**Contract:**

| Probe result | `endpoint_absent` | Gate result |
| --- | --- | --- |
| Non-zero exit recognized as connection-refused / no-daemon (`EX_UNAVAILABLE` code or an allow-listed absence phrase) | `true` | `skip()` (non-failing) |
| Non-zero exit, reachable but unhealthy | `false` | `fail()` → **red** |
| Spawn error (binary could not be executed) | `false` | `fail()` → **red** |
| Healthy (exit 0) | `false` | `pass()` |
| Any other / unrecognized non-zero exit | `false` | `fail()` → **red** |

The skip is conditioned on a *positively-recognized absence signal*, **never** on
the gate simply returning a nonzero exit code.

### Detection signal (as shipped)

`run_rpc_health_gate` runs `<binary> probe rpc --timeout <n>` and inspects the
process result through `endpoint_absent`.

The `probe rpc` subcommand ships in `src/operator_cli/probe.rs`. It opens the
canonical reader client (`memory_ipc::open_reader_client` — the daemon socket
when one is up, else a direct on-disk open, fail-closed on a present-but-
unconnectable socket) and confirms a `get_statistics()` round-trip. It exits `0`
when the RPC / cognitive-memory endpoint answers — the normal canary path, so a
healthy fresh-build candidate **passes** `rpc-health` instead of reddening the
canary on a subcommand that used to not exist. A non-zero exit is then classified
by `endpoint_absent` below. (Before this subcommand existed, the gate shelled out
to a non-existent `probe` command, got `unsupported command 'probe'` on stderr,
and — since that phrase matches no absence signal — reddened the canary every
tick; that was the self-deploy false-red root cause.)

`endpoint_absent` recognizes absence via **two** fail-closed mechanisms, in
preference order:

1. **Dedicated exit code (primary, fail-closed).** A `probe rpc` that positively
   detects "no daemon listening / connection refused" exits with the documented
   `69` (`EX_UNAVAILABLE`) code. `endpoint_absent` matches that single code and
   nothing else, so a reachable-but-unhealthy probe (which exits with a
   *different* non-zero code) is never mistaken for absence. The shipped
   in-process `probe rpc` reports a healthy exit `0` rather than `69` because its
   reader open legitimately falls back to a direct store when no daemon is up;
   the `EX_UNAVAILABLE` path stays wired as fail-closed insurance for a future
   socket-only transport that signals connection-refused.
2. **Bounded, allow-listed stderr match (fallback).** When the exit code is not
   `EX_UNAVAILABLE`, `endpoint_absent` matches a fixed, enumerated set of absence
   phrases (`connection refused`, `no daemon`, `could not connect`) against a
   lower-cased stderr, plus a bare `ENOENT` (`no such file or directory`) **only**
   when it co-occurs with a `.sock` socket-path marker. `connection reset` is
   excluded (a reset proves the peer was reachable ⇒ reachable-but-unhealthy ⇒
   red). The list is closed: any stderr not on the allow-list ⇒ `false` (fail).
   This keeps the control fail-closed even before option 1 is wired into
   `probe rpc`.

Either way, the `Err(_)` spawn-error arm is **never** absence — a binary that
cannot execute is a build defect and must red the canary — and a healthy exit `0`
is a pass, not a skip.

## Which gates may skip

Only probe gates that require a live daemon consult `endpoint_absent`:

| Gate (`RelaunchGate`) | May skip? | Condition |
| --- | --- | --- |
| `Smoke` | No | never skips (runs `--version`, no live endpoint) |
| `UnitTest` | No | never skips (build/regression gate) |
| `GymBaseline` | **No by default** | current impl runs `<binary> gym list`, a local listing that does not contact a live daemon — so it should **not** skip. Add to the skip set **only if** verified that its subcommand requires a live daemon; otherwise a skip here blinds a real gate. |
| `RpcHealth` | Yes | `endpoint_absent` true (`probe rpc` positively found no daemon) |

The build/smoke/unit/regression gates **cannot** produce a `Skipped` result;
`GateResult::skip` is only reachable from the probe gates. Example wiring for the
RPC health gate — a narrow skip branch inserted into the existing `Command`-based
match, leaving the other arms as fail:

```rust
fn run_rpc_health_gate(binary: &Path, config: &RelaunchConfig) -> GateResult {
    let timeout_secs = config.health_timeout.as_secs().to_string();
    let probe = Command::new(binary)
        .args(["probe", "rpc", "--timeout", &timeout_secs])
        .output();
    match &probe {
        Ok(output) if output.status.success() => {
            GateResult::pass(RelaunchGate::RpcHealth, "rpc health check passed")
        }
        // NEW: narrow, fail-closed skip only when absence is positively proven.
        _ if endpoint_absent(&probe) => GateResult::skip(
            RelaunchGate::RpcHealth,
            "endpoint absent in isolated canary (no daemon) — skipped",
        ),
        Ok(output) => GateResult::fail(
            RelaunchGate::RpcHealth,
            format!(
                "rpc health failed (exit {}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ),
        Err(e) => GateResult::fail(
            RelaunchGate::RpcHealth,
            format!("rpc health probe failed to run: {e}"),
        ),
    }
}
```

## Per-gate evidence: `TargetCanaryReport`

The build-and-verify report stops collapsing gate results to a bare count. It
gains three additive fields so the failing gate's identity survives.

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
struct TargetCanaryReport {
    passed: bool,
    passed_gates: usize,
    total_gates: usize,
    /// Name of the first genuinely failing gate (e.g. "rpc-health"), or `None`
    /// when the canary is green. Skipped gates are NOT failing gates.
    failing_gate: Option<String>,
    /// Bounded, sanitized detail for `failing_gate` (log data only; length-capped).
    failing_detail: Option<String>,
    /// Names of gates that skipped on an absent endpoint.
    skipped_gates: Vec<String>,
}
```

`SharedTargetCanaryVerifier::build_and_verify` populates `failing_gate` /
`failing_detail` from the **first non-passing, non-skipped** `GateResult` and
collects the `skipped_gates` names:

```rust
let passed = crate::self_relaunch::all_gates_passed(&results);
let passed_gates = results.iter().filter(|r| r.passed && !r.skipped).count();
let first_failing = results.iter().find(|r| !r.passed); // skips are passed:true
Ok(TargetCanaryReport {
    passed,
    passed_gates,
    total_gates: results.len(),
    failing_gate: first_failing.map(|r| r.gate.to_string()),
    failing_detail: first_failing.map(|r| sanitize_capped(&r.detail)),
    skipped_gates: results.iter()
        .filter(|r| r.skipped)
        .map(|r| r.gate.to_string())
        .collect(),
})
```

## Per-gate evidence: `CanaryResult`

`CanaryResult` (the value the deploy gate consumes) gains the same evidence so a
red verdict carries the named gate downstream.

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanaryResult {
    pub passed: bool,
    pub detail: String,
    /// Named failing gate when `passed == false`; `None` on a green canary.
    pub failing_gate: Option<String>,
    /// Bounded, sanitized detail for the failing gate.
    pub failing_detail: Option<String>,
}
```

`ProdCanaryRunner::run_canary` threads the report's evidence into the result.
The existing `Ok(report)` arm builds `detail` inline as `format!("{}/{} gates",
report.passed_gates, report.total_gates)`; it is extended to append any skips and
to carry the named gate:

```rust
Ok(report) => Ok(CanaryResult {
    passed: report.passed,
    // Existing "{passed}/{total} gates" detail, extended with any skips.
    detail: if report.skipped_gates.is_empty() {
        format!("{}/{} gates", report.passed_gates, report.total_gates)
    } else {
        format!(
            "{}/{} gates [skipped: {}]",
            report.passed_gates,
            report.total_gates,
            report.skipped_gates.join(", ")
        )
    },
    failing_gate: report.failing_gate.clone(),
    failing_detail: report.failing_detail.clone(),
}),
```

The two hard-error arms already in `run_canary` — `SafeUpdateError::BuildFailed`
and `SafeUpdateError::GateFailed { gate, detail }` — must likewise populate
`failing_gate` / `failing_detail` (the `GateFailed` arm already has the gate name
in scope) so a red verdict from *any* path carries a named gate downstream, not
just the `Ok(report)` false-red path.

## Operator refusal detail

The `deploy_refused` notification signature is unchanged; the caller composes a
richer reason string from the named gate. The `GuardedDeployer` refusal path
already has the `refusal: DeployRefusal` in scope; only the `RedCanary` variant
gets the named-gate detail, so a `NoOp` / `Rollback` / `CrashLoop` refusal keeps
its own `Display`:

```rust
// Only the RedCanary refusal gets named-gate detail; other refusals
// (NoOp, Rollback, CrashLoop) keep their own DeployRefusal::Display.
let reason = match (&refusal, &canary.failing_gate, &canary.failing_detail) {
    (DeployRefusal::RedCanary, Some(gate), Some(detail)) =>
        format!("red canary: gate {gate} failed — {detail}"),
    _ => refusal.to_string(),
};
let notification = OperatorNotification::deploy_refused(commit, &running, &self.repo, &reason);
```

Resulting operator log:

```text
OVERSEER did: deploy a1b2c3d4 refused - red canary:
  gate unit-test failed — tests failed (exit 101): 2 failed
  [skipped: rpc-health]
```

`failing_detail` is treated as **log data only** — never interpolated into a
shell or probe command — and is sanitized (allow-listed, no tokens or
credentialed URIs) and length-capped before it reaches the notification.

## Configuration

No new configuration keys. The skip behavior is automatic and requires no
operator opt-in:

- The canary already builds and probes in an isolated target dir
  (`RelaunchConfig::canary_target_dir`); the skip triggers only there, when the
  probe positively detects an absent endpoint.
- `RelaunchConfig::health_timeout` (default `30s`) is unchanged and still bounds
  the RPC probe. A timeout is **not** treated as endpoint-absence — it is an
  indeterminate outcome and fails closed.
- Local validation excludes the long-running `UnitTest` gate (as the existing
  gate tests do); the new outcome and evidence tests use fakes and need no live
  daemon or full test suite.

## Tests that are security controls

These tests are load-bearing; weakening them is a security regression:

| Test | Asserts |
| --- | --- |
| `gate_result_skip_upholds_skipped_implies_passed_for_all_gates` (`types.rs`) | No `GateResult` with `skipped == true` has `passed == false` (the `skipped ⇒ passed` invariant). |
| `endpoint_absent_true_for_positively_absent_endpoint` (`gates.rs`) | A recognized connection-refused / no-daemon result (`EX_UNAVAILABLE` or an allow-listed phrase) → `true`. |
| `endpoint_absent_false_for_reachable_but_unhealthy` (`gates.rs`) | A reachable-but-unhealthy probe → `false`, never `skip()`. |
| `endpoint_absent_false_for_connection_reset` (`gates.rs`) | `ECONNRESET` (reachable-but-crashing) → `false`, never `skip()` (SR-4). |
| `endpoint_absent_false_for_enoent_on_non_socket_file_mentioning_socket` (`gates.rs`) | ENOENT on a non-`.sock` file whose name merely contains "socket" → `false`, never `skip()` (SR-4). |
| `endpoint_absent_false_for_spawn_error` (`gates.rs`) | A probe spawn `Err` (binary could not execute) → `false`, never `skip()`. |
| `endpoint_absent_false_for_healthy_success` (`gates.rs`) | A healthy exit `0` probe → `false` (it is a pass, not an absence signal). |
| `rpc_health_gate_spawn_error_fails_closed_not_skipped` (`gates.rs`) | The `RpcHealth` gate on a spawn error → `fail()` (RED), `skipped == false`. |
| `all_gates_passed_treats_skip_as_non_failing` (`gates.rs`) | A skip is counted non-failing; a genuine failure alongside a skip is still RED. |
| `run_canary_appends_skipped_gates_to_detail_and_stays_green` (`deploy.rs`) | An absent-endpoint probe skips, the canary aggregate is green, and the skip is surfaced in the detail. |
| `deploy_refused_names_failing_gate_for_red_canary` (`deploy.rs`) | A genuine gate failure names the gate + detail in `deploy_refused` (not a bare count). |
| `run_canary_threads_named_failing_gate_from_red_report` (`deploy.rs`) | The named failing gate + detail survive from the report into `CanaryResult`. |

## Related

- [Concept: self-deploy canary gate skip-on-absent-endpoint and per-gate diagnosability](../concepts/self-deploy-canary-gate-skip-and-diagnosability.md)
- [Self-deploy API reference](./self-deploy-api.md) — `evaluate_deploy_gate`,
  `CanaryRunner`, `DeployRefusal`, and the operator notifications.
- [Concept: reconcile-and-self-deploy](../concepts/reconcile-and-self-deploy.md)
- [How to verify and roll back a self-deploy](../howto/verify-and-roll-back-a-self-deploy.md)
