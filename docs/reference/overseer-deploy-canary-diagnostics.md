---
title: Overseer deploy red-canary diagnostics
description: Reference for the additive red-canary diagnostics on the Overseer autonomous deploy gate — the CanaryResult.failing_gate / failing_detail fields, CanaryResult::refusal_reason, the enriched deploy_refused notification and Capability error detail, the structured overseer::deploy WARN telemetry (failing_gate / failing_detail attributes), and the is_transient deploy_gate/target_canary fail-closed classification guard that keeps a red canary from being retried as a transient blip.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./self-deploy-api.md
  - ./overseer-operator-notifications.md
  - ./overseer-tick-details.md
  - ./overseer-tick-self-healing.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../safe-self-update.md
  - ../../src/overseer/deploy.rs
  - ../../src/overseer/wiring.rs
  - ../../src/self_deploy/source_prep.rs
  - ../../src/self_relaunch/mod.rs
  - ../../src/safe_update/mod.rs
---

# Overseer deploy red-canary diagnostics

> **Status: implemented.** The `CanaryResult.failing_gate` / `failing_detail`
> fields, the `CanaryResult::refusal_reason` method, the enriched
> `deploy_refused` notification and `OverseerError::Capability` detail, the
> structured `overseer::deploy` WARN event, and the `is_transient`
> `deploy_gate` / `target_canary` guard live in
> [`src/overseer/deploy.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy.rs)
> and [`src/overseer/wiring.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/wiring.rs).
> The failing gate is threaded up from the per-gate `GateResult` list through an
> extended `TargetCanaryReport` (`src/overseer/deploy.rs`) fed by
> `prepare_build_and_verify_canary`
> ([`src/self_deploy/source_prep.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/source_prep.rs)).
> The change is **additive and non-breaking**: no public signature was removed,
> `DeployRefusal`'s `Display` is unchanged, and every existing construction site
> and test compiles against the new optional fields.

## Why this exists

When the Overseer's guarded deploy gate refused a deploy on a red canary, the
only thing an operator saw was the opaque line:

```
red canary (one or more gates failed)
```

That message named neither the reddened gate nor its detail, so a recurring
self-deploy crash-loop was undiagnosable from the tick log: every failed
overseer tick emitted the same string while `DeployDrift` climbed monotonically
and Simard ran increasingly stale code without ever self-deploying.

This feature surfaces the **specific reddening gate name and its detail** all the
way to the tick WARN and the OTel attribute surface, so a red canary is
diagnosable in one glance rather than requiring a manual canary re-run. It also
closes a fail-closed hole: a red canary whose detail happens to contain a word
like `timeout` or `503` can no longer be misclassified as a transient,
retryable blip.

The enrichment is **read-only telemetry**. It never converts a refusal into a
proceed, and gates continue to fail closed.

## Data model

### `CanaryResult` (additive fields)

`CanaryResult` (in [`src/overseer/deploy.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy.rs))
carries the result of building + verifying the canary. Two optional fields are
added; the existing `passed` / `detail` fields are unchanged.

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanaryResult {
    /// Did every canary gate pass?
    pub passed: bool,
    /// Human-readable summary (e.g. "4/4 gates" on green).
    pub detail: String,
    /// Name of the specific gate that reddened the canary, when known.
    /// `None` on a green canary and when the runner cannot attribute the
    /// failure to a single named gate.
    pub failing_gate: Option<String>,
    /// The reddening gate's detail (e.g. the failing `cargo test` output or the
    /// build error), when known. `None` on a green canary. Bounded to a
    /// UTF-8-safe length at population time in `ProdCanaryRunner::run_canary`
    /// (see [Bounding](#bounding)).
    pub failing_detail: Option<String>,
}
```

| Field | Green canary | Red canary (named gate) | Red canary (build failed) |
| --- | --- | --- | --- |
| `passed` | `true` | `false` | `false` |
| `detail` | `"4/4 gates"` | `"3/4 gates"` | `"target canary build failed: …"` |
| `failing_gate` | `None` | `Some("unit-test")` | `Some("build")` |
| `failing_detail` | `None` | `Some("test tests::… FAILED …")` | `Some("linker error: …")` |

**Invariant:** on a green canary (`passed == true`) both `failing_gate` and
`failing_detail` are `None`.

#### Population by the production runner

The reddening gate name and detail **live in the per-gate `GateResult` list, not
in `SafeUpdateError`**. `crate::self_relaunch::verify_canary` /
`prepare_build_and_verify_canary`
([`src/self_deploy/source_prep.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/source_prep.rs))
return `Ok(Vec<GateResult>)` even when a gate is red — each `GateResult` carries
its own `gate: RelaunchGate` (whose `Display` is the gate name, e.g. `unit-test`)
and `detail: String`. They only return `Err` on a harness-level fault (build
failure, prepare failure, or an infrastructure error). So a **normal red canary
reaches the runner through the `Ok` path**, and the failing gate must be threaded
up from that list — not read from `SafeUpdateError`.

To carry it, `TargetCanaryReport`
([`src/overseer/deploy.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy.rs))
is extended with the first failing gate's name and detail, populated in
`build_and_verify` from the `GateResult` list instead of collapsing it to bare
counts:

```rust
struct TargetCanaryReport {
    passed: bool,
    passed_gates: usize,
    total_gates: usize,
    /// First failing `GateResult.gate` (Display), when a gate reddened.
    failing_gate: Option<String>,
    /// That gate's `GateResult.detail`, when a gate reddened.
    failing_detail: Option<String>,
}

// in TargetCanaryVerifier::build_and_verify, after collecting `results`:
let first_failing = results.iter().find(|r| !r.passed);
Ok(TargetCanaryReport {
    passed: all_gates_passed(&results),
    passed_gates: results.iter().filter(|r| r.passed).count(),
    total_gates: results.len(),
    failing_gate: first_failing.map(|r| r.gate.to_string()),
    failing_detail: first_failing.map(|r| r.detail.clone()),
})
```

`ProdCanaryRunner::run_canary` then maps the verifier result onto `CanaryResult`,
bounding `failing_detail` at population time (see [Bounding](#bounding)):

| Source | `failing_gate` | `failing_detail` |
| --- | --- | --- |
| `Ok(report)`, `report.passed == true` (green) | `None` | `None` |
| `Ok(report)`, `report.passed == false` (a gate reddened) | `report.failing_gate` — e.g. `Some("unit-test")` | `Some(bound(report.failing_detail))` |
| `Err(SafeUpdateError::BuildFailed { detail })` (build/prepare failed) | `Some("build")` | `Some(bound(detail))` |
| `Err(SafeUpdateError::GateFailed { gate, detail })` (harness-level gate fault) | `Some(gate)` | `Some(bound(detail))` |
| `Err(_)` other harness fault | — returns `Err(OverseerError::Capability { what: "target_canary", .. })`, no `CanaryResult` | — |

> **Why not `SafeUpdateError::GateFailed`?** Its `gate` field is the hardcoded
> literal `"target-canary"` (`src/self_deploy/source_prep.rs`), never the real
> gate name, and a routine red gate does not travel that path at all. Populating
> `failing_gate` solely from `SafeUpdateError` would leave `failing_gate == None`
> on the exact path #4420 must diagnose — hence the `TargetCanaryReport`
> threading above.

<a id="bounding"></a>
#### Bounding

`failing_detail` embeds subprocess output (compiler / test-runner bytes) whose
size is not bounded by shape. It is truncated **once, at population time in
`ProdCanaryRunner::run_canary`**, to `CanaryResult::DETAIL_CAP` (512 bytes) at a
UTF-8 char boundary — before it is stored on `CanaryResult` — with a visible
`… (truncated)` marker counted against the cap (so the bound is idempotent).
Because that stored field is later emitted verbatim as the raw `failing_detail`
OTel attribute *and* folded by `refusal_reason` into *both* the operator
`deploy_refused` reason *and* the `OverseerError::Capability` detail (which rides
up to the tick WARN), bounding at the single population site covers every
downstream sink at once. `refusal_reason` re-applies the same idempotent bound
defensively for `CanaryResult`s built by other paths. Truncation never splits a
multi-byte character.

### `unit-test` gate `first_failure=` detail (#4470)

The `failing_detail` surfaced above is only as useful as the underlying
`GateResult.detail`. For the `unit-test` gate — the gate that reddened the
self-deploy canary in the #4470 incident — the raw `cargo test` stderr tail
often does **not** contain the failing test's name near the end, so the bounded
512-byte tail could name no test at all. `run_unit_test_gate`
([`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs))
therefore **extracts the first failing test path** from the full `cargo test`
output and prepends it to the gate detail as a stable `first_failure=` prefix:

```text
tests failed (exit 101): first_failure=<crate>::<module>::<test_name>; <bounded stderr tail>
```

| Field | Meaning |
| --- | --- |
| `first_failure=<test::path>` | The first test path parsed from a `test <path> ... FAILED` line (or the `failures:` block) in the `cargo test` output. Omitted only when no test name can be parsed (e.g. a link/compile abort with no test lines) — the bounded stderr tail is still included. |
| `<bounded stderr tail>` | The existing truncated stderr, unchanged. |

Extraction rules:

- **Parsed from the runner output**, not guessed — it reads the `... FAILED`
  lines / `failures:` section that `cargo test` emits. The first failing test
  wins (deterministic).
- **Bounded** to ≤ 512 bytes total, at a UTF-8 char boundary, consistent with the
  `failing_detail` cap above.
- **Sanitized**: CR, LF, and other control characters are stripped from the
  parsed test name before it is embedded, so the detail is a single clean line
  and cannot forge additional log fields or JSON. The parsed name is treated as
  **data, not a format string**.
- **Schema-stable**: `GateResult` keeps its `{ gate, passed, detail }` shape;
  only the *content* of `detail` is enriched. `exit 101` (a Rust test-binary
  panic/abort) still surfaces as before, now accompanied by the specific test.

Because the failing gate's `detail` is what `TargetCanaryReport.failing_detail`
copies from, the `first_failure=` prefix rides all the way up to the operator
`deploy_refused` reason, the `overseer::deploy` WARN, and the `failing_detail`
OTel attribute — so a red `unit-test` canary now names the exact test to fix in
one glance. Acting on it is covered in
[STEP 2: acting on the surfaced detail](#step-2-acting-on-the-surfaced-detail).

### `CanaryResult::refusal_reason`

A new inherent method composes the enriched, human-readable refusal string
without touching `DeployRefusal`'s `Display`:

```rust
impl CanaryResult {
    /// Compose the operator-facing refusal reason for a `DeployRefusal`.
    ///
    /// For `DeployRefusal::RedCanary`, returns
    /// `"red canary (gate {failing_gate}: {failing_detail})"` when a gate is
    /// attributed, degrading to `"red canary ({detail})"` (the aggregate gate
    /// summary, e.g. `"0/4 gates"`) when no single gate is named — never the
    /// bare opaque phrase. For every other refusal variant, returns
    /// `refusal.to_string()` verbatim.
    pub fn refusal_reason(&self, refusal: &DeployRefusal) -> String;
}
```

Only `RedCanary` is enriched. `NoOp`, `Rollback`, and `CrashLoop` reasons are
unchanged.

Example outputs:

| Refusal | `refusal_reason` output |
| --- | --- |
| `RedCanary` + `failing_gate = Some("unit-test")` | `red canary (gate unit-test: test tests::deploy_gate_refuses_rollback FAILED)` |
| `RedCanary` + `failing_gate = None` (detail `"0/4 gates"`) | `red canary (0/4 gates)` |
| `NoOp` | `no-op deploy (target == running commit)` |
| `Rollback` | `rollback refused (target is older than running)` |
| `CrashLoop { churn: 4 }` | `crash-loop suspected (restart churn 4) — not deploying` |

## Behavior at the refusal site

When `evaluate_deploy_gate` returns `Err(refusal)`, `GuardedDeployer::deploy`
now:

1. Builds the enriched reason with `canary.refusal_reason(&refusal)`.
2. Sends the operator `deploy_refused` notification with that enriched reason
   (instead of the opaque `refusal.to_string()`).
3. Emits a structured WARN on `target: "overseer::deploy"` carrying the gate
   name and detail as discrete attributes.
4. Returns `OverseerError::Capability { what: "deploy_gate", detail: <enriched
   reason> }` so the reason rides up to the tick WARN through the existing
   `error = %e` field.

```rust
if let Err(refusal) = evaluate_deploy_gate(&ctx) {
    let reason = canary.refusal_reason(&refusal);
    let failing_gate = canary.failing_gate.as_deref().unwrap_or("");
    let failing_detail = canary.failing_detail.as_deref().unwrap_or("");

    tracing::warn!(
        target: "overseer::deploy",
        target_commit = %commit,
        running_commit = %running,
        failing_gate,
        failing_detail,
        refusal = %reason,
        "self-deploy refused by deploy gate"
    );

    let notification =
        OperatorNotification::deploy_refused(commit, &running, &self.repo, &reason);
    let _ = self.notifier.notify(&notification);

    return Err(OverseerError::Capability {
        what: "deploy_gate",
        detail: reason,
    });
}
```

`failing_gate` / `failing_detail` are empty strings on a non-canary refusal
(`NoOp` / `Rollback` / `CrashLoop`) and on a green-gate path; they carry a value
**only** when a named gate reddened the canary. The `refusal` attribute always
carries the enriched reason, which is byte-for-byte the same string returned in
the `OverseerError::Capability` detail — so the tick WARN (`error = %e`) and this
`overseer::deploy` WARN agree.

> **Two red surfaces, one enrichment.** A red canary reaches the refusal site
> only via the `Ok`-path `DeployRefusal::RedCanary` decision shown above — that is
> where `refusal_reason` enriches. The *other* red surface, a harness-level
> `target_canary` fault, is returned earlier by `ProdCanaryRunner::run_canary` as
> `Err(OverseerError::Capability { what: "target_canary", .. })`; it never
> produces a `CanaryResult` or a `DeployRefusal`, so there is no gate to
> attribute and `refusal_reason` is not called on it. Both surfaces are held
> non-transient by the [`is_transient` guard](#fail-closed-classification-guard).

### Telemetry / OTel attributes

Simard's `tracing` layer is the OTel bridge; there is no separate OTel SDK call
site. The structured key=value fields on the `warn!` event **are** the OTel
attributes — no subscriber, exporter, or config change is required. The two new
attributes are:

| Attribute | Type | Present when | Example |
| --- | --- | --- | --- |
| `failing_gate` | string | a named gate reddened the canary | `unit-test` |
| `failing_detail` | string | a red canary carried a detail (bounded) | `test tests::… FAILED` |

Because emission stays at WARN on the existing `overseer::deploy` /
`overseer::tick` targets, any exporter-side redaction and retention already
configured for those targets continues to apply.

## What an operator now sees

**Before** — the tick log (`target: "overseer::tick"`) on each of the 8 failed
ticks:

```
WARN overseer::tick: capability deploy_gate failed: red canary (one or more gates failed)
```

**After** — the same tick, plus the dedicated `overseer::deploy` event:

```
WARN overseer::deploy: self-deploy refused by deploy gate
    target_commit=9f2c1ab running_commit=3b7e4d0
    failing_gate=unit-test
    failing_detail="test tests::deploy_gate_refuses_rollback ... FAILED"
    refusal="red canary (gate unit-test: test tests::deploy_gate_refuses_rollback ... FAILED)"
WARN overseer::tick: capability deploy_gate failed: red canary (gate unit-test: test tests::deploy_gate_refuses_rollback ... FAILED)
```

The reddening gate (`unit-test`) and its detail are now in the log and in the
operator notification, so STEP 2 root-causing — deciding whether the red canary
is a genuine regression on `main` or a flaky/misconfigured gate — is
evidence-driven rather than a blind re-run.

## Fail-closed classification guard

`is_transient` (in [`src/overseer/wiring.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/wiring.rs))
decides whether a `run_cycle` failure is a transient, self-clearing upstream
fault the next tick may simply retry. It matches an allowlist of substrings
(`503`, `timeout`, `connection reset`, …) against the capability `detail`.

Enriching the deploy refusal detail with subprocess output creates a hazard: a
red canary whose failing test output literally contains `timeout` or `503`
could be misread as a retryable blip, silently masking a real regression and
letting drift climb unchecked.

To close this, `is_transient` **fails closed for deploy decisions**: it returns
`false` early for `OverseerError::Capability` whose `what` is `"deploy_gate"` or
`"target_canary"`, *before* the substring allowlist is consulted.

```rust
pub fn is_transient(err: &OverseerError) -> bool {
    let OverseerError::Capability { what, detail } = err else {
        return false;
    };

    // A deploy-gate / target-canary refusal is a DECISION, never an
    // infrastructure blip — it must latch, regardless of what the enriched
    // detail happens to contain. Guard runs before the substring allowlist.
    if matches!(*what, "deploy_gate" | "target_canary") {
        return false;
    }

    let d = detail.to_ascii_lowercase();
    // … existing TRANSIENT_SIGNALS allowlist …
}
```

**Contract:** a `deploy_gate` or `target_canary` capability failure is **never**
transient, even when its detail contains an allowlisted signal word. This is the
SR-1 invariant (a real defect must latch `"erroring"`) applied to the deploy
path, and it ships with a regression test asserting that a
`Capability { what: "deploy_gate", detail: "… timeout …" }` is not transient.

## Compatibility

- **Additive fields.** `CanaryResult` gains two `Option<String>` fields. Every
  struct-literal construction site (the production runner and the test fakes)
  is updated in the same change; `None` on green.
- **`DeployRefusal::Display` unchanged.** The variant `Display` strings are
  byte-for-byte identical, so any test asserting the raw variant text still
  passes. Enrichment happens only via `refusal_reason` at the deploy site.
- **`OverseerError::Capability` unchanged.** No new variant or field; the
  enriched reason flows through the existing `detail: String`.
- **No new inputs.** No new endpoints, RPC, CLI flags, config keys, or operator
  "skip gate" controls. The trust boundary is unchanged.
- **No `print`-family macros.** All emission is `tracing` structured
  key=value at ≥ WARN. There are no `print!` / `println!` / `eprintln!` sinks
  and no silent fallbacks.

## STEP 2: acting on the surfaced detail

Once STEP 1 surfaces the reddening gate, classify and fix the true root cause —
never disable a gate to force green:

- **Genuine regression** (e.g. a reproducible `unit-test` `cargo test` failure on
  merged `main`): fix the failing source/test at its origin so the canary goes
  green legitimately.
- **Flaky / misconfigured gate** (non-deterministic, environment/endpoint
  absent, or a wrong threshold): correct the gate's configuration or logic so it
  stops false-reddening **while still failing closed** on real regressions.

Either way, the fix must let a legitimately green build self-deploy so
`DeployDrift` returns to 0. Detection must remain intact — a gate is never
weakened or disabled to mask a real regression.

## See also

- [Self-deploy API reference](./self-deploy-api.md) — the `GuardedDeployer`,
  `DeployRefusal`, `evaluate_deploy_gate`, and the `OrchestratedBinaryDeployer`
  swap path.
- [Overseer operator notifications](./overseer-operator-notifications.md) — the
  `deploy_refused` / `deploy_starting` notification surface.
- [Overseer tick details](./overseer-tick-details.md) — the `overseer::tick`
  WARN event and the per-problem detail rows.
- [Overseer tick self-healing](./overseer-tick-self-healing.md) — the
  `is_transient` fail-closed classifier and the SR-1 latch invariant.
- [Self-deploy quarantine-acknowledge](./self-deploy-quarantine-acknowledge.md)
  — the paired `no_quarantine` deadlock fix (#4469): the *other* self-deploy
  blocker that had to clear alongside the red canary for self-deploy to converge.
