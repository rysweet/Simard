---
title: Canary gate attribution API reference
description: >
  Reference for the structured, per-gate attribution the autonomous
  deploy rail emits on every red canary: the additive failing_gate /
  failing_detail / passed_gates / total_gates fields on
  TargetCanaryReport and CanaryResult, the canary_gate_failed / canary_build_failed
  / canary_infra_error closed-vocabulary root-cause tags emitted via tracing +
  OpenTelemetry from overseer::deploy and self_relaunch::gates, the typed
  first-failing RelaunchGate carried end-to-end via TargetCanaryReport (with
  SafeUpdateError::GateFailed's reserved target-canary label used only for a
  pre-gate infra fault), the failing_detail sanitizer, and the hermetic scoping
  of the UnitTest gate. Explains how a future Overseer tick attributes DeployDrift
  to a concrete gate instead of a bare "red canary".
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
status: current
related:
  - ./self-deploy-api.md
  - ./self-deploy-source-prep.md
  - ./overseer-tick-details.md
  - ./overseer-root-cause-why-api.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../../src/overseer/deploy.rs
  - ../../src/self_relaunch/gates.rs
  - ../../src/self_relaunch/types.rs
  - ../../src/self_deploy/source_prep.rs
---

# Canary gate attribution API reference

> **Status: implemented.** This feature has landed in
> [`src/overseer/deploy.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy.rs),
> [`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs),
> [`src/self_relaunch/types.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs),
> and [`src/self_deploy/source_prep.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/source_prep.rs).
> This document is the authoritative reference for the additive struct fields, the
> closed-vocabulary root-cause tags, the per-gate tracing events, the
> `failing_detail` sanitizer, and the hermetic `UnitTest` gate scoping.
> The feature **extends** — never replaces — the deploy gate documented in the
> [Self-deploy API reference](./self-deploy-api.md). Every change is purely
> additive: no type is renamed or removed, and `DeployRefusal::RedCanary`'s
> `Display` string stays byte-for-byte identical to before.

## Why this exists

The autonomous deploy rail refuses a candidate build whenever any canary gate is
red. Before this feature the refusal surfaced only as the human string
`red canary (one or more gates failed)`. When `DeployDrift` climbed
monotonically (Simard merged PRs but could not ship them into her running
binary), every tick logged the same opaque line — the operator could see *that*
the canary was red but not *which* gate reddened it or *why*. Drift could not be
attributed to a concrete cause, so it could not be fixed without a manual local
reproduction of the whole build-and-verify path.

This feature makes every red canary **self-attributing**. The first failing gate
(a typed `RelaunchGate`) and a sanitized failure `detail` propagate
end-to-end from gate execution to the refusal site, and each refusal emits a
single structured `tracing` event carrying a stable machine-readable
`root_cause` tag. A future tick — and any OpenTelemetry backend — can attribute
drift to a named gate (`smoke`, `unit-test`, `gym-baseline`, `rpc-health`)
without a repro.

## Contents

- [Root-cause tags (closed vocabulary)](#root-cause-tags-closed-vocabulary)
- [Structured event schema](#structured-event-schema)
- [Data model additions](#data-model-additions)
  - [`RelaunchGate` label contract](#relaunchgate-label-contract)
  - [`TargetCanaryReport` (additive fields)](#targetcanaryreport-additive-fields)
  - [`CanaryResult` (additive fields)](#canaryresult-additive-fields)
  - [`SafeUpdateError::GateFailed.gate`](#safeupdateerrorgatefailedgate)
- [`failing_detail` sanitizer](#failing_detail-sanitizer)
- [Gate scoping: `UnitTest` is hermetic](#gate-scoping-unittest-is-hermetic)
- [Compatibility guarantees](#compatibility-guarantees)
- [Examples](#examples)
- [Tutorial: attribute a red-canary tick to a gate](#tutorial-attribute-a-red-canary-tick-to-a-gate)
- [See also](#see-also)

## Root-cause tags (closed vocabulary)

Every canary failure emits a `root_cause` field drawn from a **fixed, closed
vocabulary**. The vocabulary is a stable machine contract: dashboards, alert
rules, and the Overseer root-cause analyzer key off these exact strings, so they
never change spelling and never carry free-form text.

| `root_cause` | Meaning | Carries a concrete `gate`? |
| --- | --- | --- |
| `canary_gate_failed` | A relaunch gate ran on the candidate and returned `passed: false` (a genuine or environment red). | **Yes** — the first failing `RelaunchGate`. |
| `canary_build_failed` | The candidate `cargo build --release` failed before any gate ran. | No — no gate executed. |
| `canary_infra_error` | The verify harness itself could not run the gate sequence (e.g. `verify_canary` returned `Err`, build-lock acquisition failed). Pre-gate infrastructure fault, not a candidate regression. | No — `gate` is the reserved label `target-canary`. |

The rule that ties this together:

> **No bare `red canary` may ever be the terminal diagnostic without an attached
> concrete `gate` field.** A `canary_gate_failed` event without a `gate` is a
> contract violation caught by the deploy-drift tests.

> **Implementer note — build-lock classification.** For build-lock contention to
> surface as `canary_infra_error` (as this table and the infra example below
> state), `source_prep::with_self_deploy_build_lock` must wrap the acquisition
> failure as a **pre-gate infra** fault, distinct from a candidate
> `SafeUpdateError::BuildFailed`. It currently wraps it as `BuildFailed`
> ([`source_prep.rs`](../../src/self_deploy/source_prep.rs) `with_self_deploy_build_lock`),
> which the [CanaryResult mapping](#canaryresult-additive-fields) below routes to
> `canary_build_failed`. Reserve `BuildFailed` for the candidate `cargo build`
> step (`build_self_deploy_candidate`); route lock/harness failures to the infra
> tag (e.g. a distinct `SafeUpdateError` variant or an `Err(..)` that
> `run_canary` maps to `canary_infra_error`).

## Structured event schema

Two events are emitted on the red-canary path. They use **distinct `target`s** so
they never duplicate each other in log output, and both flow to OpenTelemetry via
the existing `tracing-opentelemetry` layer — no new telemetry sink, file, socket,
or `stdout`/`stderr` path is introduced.

### Summary event — `target: "overseer::deploy"`

Emitted **exactly once**, and **only** for `DeployRefusal::RedCanary`, from the
single shared refusal branch in `GuardedDeployer::deploy()` (guard the emission
with `matches!(refusal, DeployRefusal::RedCanary)` — that branch also fires for
`NoOp` / `Rollback` / `CrashLoop`, which must **not** emit a `canary_gate_failed`
event). The `canary: CanaryResult` bound earlier in `deploy()` is in scope at the
refusal branch, so `failing_gate` / `failing_detail` / the gate counts are all
available there. The event is emitted immediately before the `deploy-refused`
operator notification and the returned `Err`.

> **Coverage boundary.** This event covers the *refusal* path, where the canary
> completed and returned `Ok(CanaryResult { passed: false, .. })`. A **hard**
> `run_canary` error (an `Err(OverseerError)`) takes the separate early-return
> path in `deploy()` *before* `evaluate_deploy_gate` runs, so it does not reach
> this branch. That early return already fires its own `deploy-refused`
> notification; if machine-readable attribution is wanted there too, it needs its
> own `warn!` at that site — it is otherwise out of scope for the three
> canary root-cause tags.

```rust
tracing::warn!(
    target: "overseer::deploy",
    root_cause = "canary_gate_failed",     // closed vocabulary (see table)
    gate = %failing_gate,                   // e.g. "unit-test"; absent only for build/infra
    detail = %failing_detail,               // sanitized; see sanitizer section
    passed_gates,                           // usize
    total_gates,                            // usize
    target_commit = %short_sha,             // 12-char short SHA only
    "canary refused deploy",
);
```

| Field | Type | Notes |
| --- | --- | --- |
| `root_cause` | `&'static str` | One of the closed vocabulary above. |
| `gate` | `RelaunchGate` (`Display`) | The **first** failing gate. Omitted for `canary_build_failed`; the reserved `target-canary` label for `canary_infra_error`. |
| `detail` | `String` | Sanitized failure detail (truncated, control-stripped, secret-redacted). Emitted as an escaped structured field (`detail = %…`), **never** format-string interpolated. |
| `passed_gates` | `usize` | How many gates passed before/around the failure. |
| `total_gates` | `usize` | Total gates in the sequence. |
| `target_commit` | `String` | **Short SHA only** (first 12 hex chars) — no branch, remote, or URL, so telemetry never leaks infra topology. The `commit` in scope at the refusal site may be 4–64 hex chars (`is_hex_commitish`), so the emitter **must truncate** (e.g. `&commit[..commit.len().min(12)]`) before emitting; the short form is not guaranteed upstream. |

### Per-gate event — `target: "self_relaunch::gates"`

Emitted by `verify_canary` for **each** gate that returns `passed: false`, at the
moment the gate result is produced. This gives fine-grained per-gate visibility
even when multiple gates are red (the sequence does not short-circuit).

```rust
tracing::warn!(
    target: "self_relaunch::gates",
    gate = %result.gate,                    // the specific gate
    detail = %sanitize(&result.detail),     // sanitized
    "canary gate failed",
);
```

Because the two events use different `target`s, a subscriber that wants only the
one-line-per-tick summary filters to `overseer::deploy`, while a subscriber
debugging a flaky gate enables `self_relaunch::gates=warn`.

## Data model additions

All new fields are `Option`al and default to `None`, so every existing
constructor and every external struct literal keeps compiling unchanged.

### `RelaunchGate` label contract

`RelaunchGate` (in `src/self_relaunch/types.rs`) already derives
`Clone + Copy + Debug + Eq + PartialEq`, which is what lets it be embedded in an
`Option<RelaunchGate>` and copied into an event field. Its `Display` labels are
the **stable attribution vocabulary**:

| Variant | Label (`Display`) |
| --- | --- |
| `RelaunchGate::Smoke` | `smoke` |
| `RelaunchGate::UnitTest` | `unit-test` |
| `RelaunchGate::GymBaseline` | `gym-baseline` |
| `RelaunchGate::RpcHealth` | `rpc-health` |

These labels are the only values that may appear in a `gate` field for a
`canary_gate_failed` event. The reserved label `target-canary` is used **only**
for `canary_infra_error` (a pre-gate harness fault, not one of the four gates).
`target-canary` (harness/verify wrapper) and, if adopted, `self-test` (self-test
harness) are the reserved pre-gate infra labels; they are never emitted for a
`canary_gate_failed`. The current source uses only `target-canary`
([`source_prep.rs`](../../src/self_deploy/source_prep.rs)); if a distinct
`self-test` infra path is not introduced, drop `self-test` from the reserved
vocabulary rather than leaving it undocumented.

### `TargetCanaryReport` (additive fields)

`TargetCanaryReport` (in `src/overseer/deploy.rs`) stops collapsing the
`Vec<GateResult>` into bare counts and additionally captures the first failing
gate:

```rust
struct TargetCanaryReport {
    passed: bool,
    passed_gates: usize,
    total_gates: usize,
    // Additive: populated from the first `!passed` GateResult.
    failing_gate: Option<RelaunchGate>,
    failing_detail: Option<String>,   // already sanitized
}
```

`SharedTargetCanaryVerifier::build_and_verify` walks the gate results, records
`passed_gates` / `total_gates` as before, and sets `failing_gate` /
`failing_detail` from the first result whose `passed == false`. When every gate
passes, both remain `None`.

### `CanaryResult` (additive fields)

`CanaryResult` is `pub`. The two new fields are additive `Option`s so the
concrete gate survives from the verifier all the way to the `RedCanary` refusal
site without being flattened into the free-text `detail`:

```rust
pub struct CanaryResult {
    pub passed: bool,
    pub detail: String,
    // Additive: the concrete first-failing gate + its sanitized detail.
    pub failing_gate: Option<RelaunchGate>,
    pub failing_detail: Option<String>,
    // Additive: gate counts, threaded so the summary event can report them at
    // the refusal site (they otherwise live only on the internal
    // TargetCanaryReport and are lost when run_canary builds CanaryResult).
    pub passed_gates: usize,
    pub total_gates: usize,
}
```

> **Design note (extends the additive-fields set).** The summary event's
> `passed_gates` / `total_gates` fields are **not** derivable at the refusal site
> unless the counts are carried on `CanaryResult`. `TargetCanaryReport` holds them
> but is consumed inside `run_canary`, which currently flattens them into the
> free-text `detail` (`"N/M gates"`). The feature therefore threads the counts
> onto `CanaryResult` in addition to `failing_gate` / `failing_detail`. On the
> `canary_infra_error` path no gate ran but the sequence was *attempted*, so
> `passed_gates == 0` and `total_gates` is the length of the configured gate list
> (`default_gates().len()`). On the `canary_build_failed` path the candidate never
> produced a binary, so no gate was even attempted: `passed_gates == 0` **and**
> `total_gates == 0`. That `total_gates == 0` is the load-bearing discriminator
> the refusal site reads to distinguish a build failure (no gate attempted) from a
> harness/infra fault (full sequence attempted) when neither carries a concrete
> `failing_gate`.

`run_canary` maps each verifier outcome onto these fields:

| Verifier outcome | `passed` | `failing_gate` | `total_gates` | `root_cause` (at refusal) |
| --- | --- | --- | --- | --- |
| All gates passed | `true` | `None` | `N` | — (no refusal) |
| First red gate (`Ok` report, `passed: false`) | `false` | `Some(gate)` | `N` | `canary_gate_failed` |
| `BuildFailed` | `false` | `None` | `0` | `canary_build_failed` |
| `GateFailed { gate: "target-canary", .. }` (infra) | `false` | `None` | `N` | `canary_infra_error` |

A concrete gate red is **not** a `SafeUpdateError` — `verify_canary` does not
short-circuit, so a reddened gate comes back inside `Ok(results)` (with
`passed: false`). `SharedTargetCanaryVerifier::build_and_verify` extracts the
**first** failing gate from that `Vec<GateResult>` into
`TargetCanaryReport.failing_gate`, and `run_canary` threads it onto
`CanaryResult.failing_gate`. The refusal site keys its `root_cause` off the typed
`failing_gate` (present ⇒ `canary_gate_failed`) and, when it is absent, off
`total_gates` (`0` ⇒ `canary_build_failed`, otherwise ⇒ `canary_infra_error`).

> **Do not string-scrape `detail` for the gate name.** Read the typed
> `failing_gate` field. The typed value is a closed enum an attacker-influenced
> candidate cannot spoof; `detail` is display-only telemetry.

### `SafeUpdateError::GateFailed.gate`

`SafeUpdateError::GateFailed { gate, detail }` (see the
[Error variants table](./self-deploy-api.md#error-variants)) is raised **only**
for the pre-gate infra case — when the verify harness itself failed to run the
sequence — and carries the reserved generic label `target-canary`
(`canary_infra_error`). A *concrete* relaunch gate that reddens the candidate is
**not** funnelled through `GateFailed`: it is a normal `Ok(results)` entry with
`passed: false`, so its typed identity survives via
`TargetCanaryReport.failing_gate` / `CanaryResult.failing_gate` and surfaces as a
`canary_gate_failed` event (see the mapping table above). Keeping gate reds on the
`Ok` path preserves the full gate-count context (`passed_gates` / `total_gates`)
and the no-short-circuit "run every gate" behaviour that a concrete-labelled
`GateFailed` would lose. The variant's *shape* is unchanged, so every existing
`match` arm keeps compiling.

`prepare_build_and_verify_canary` (in `src/self_deploy/source_prep.rs`) keeps its
existing mapping: it returns `Ok(results)` for gate reds (concrete attribution
happens downstream in the verifier), and raises
`GateFailed { gate: "target-canary", .. }` only when `verify_canary` returns
`Err(..)` — the harness could not run the sequence.

## `failing_detail` sanitizer

`failing_detail` originates from **untrusted, candidate-controlled** text —
`cargo test` stdout/stderr and `e.to_string()` from the candidate build. Before
it enters any struct field or tracing event it passes through a sanitizer that
applies, in order:

1. **Control-character stripping** — CR, LF, and ANSI/VT escape sequences are
   removed so a candidate can never forge a second log line or inject a fake
   structured field (log-injection defense).
2. **Hard truncation** — capped at a fixed budget (**1024 bytes**) on a UTF-8
   char boundary, with a trailing `…[truncated]` marker. This bounds OpenTelemetry
   payload size (DoS defense) and caps the blast radius of anything the redactor
   misses.
3. **Best-effort secret redaction** — common secret shapes are replaced with
   `[redacted secret]` as defense-in-depth: AWS-style keys (`AKIA…`), `token=…` /
   `key=…` credential assignments, and PEM private-key blocks
   (`-----BEGIN … KEY-----`).

The sanitized string is what lands in `TargetCanaryReport.failing_detail`,
`CanaryResult.failing_detail`, and both tracing events' `detail` fields. It is
**telemetry only** — never executed, dispatched, retried from, or otherwise acted
upon.

## Gate scoping: `UnitTest` is hermetic

The `UnitTest` gate previously ran the **full** `cargo test` suite on the
candidate inside the canary target dir *while the host-wide `BuildLock` was held*
by `prepare_build_and_verify_canary`. That made it environment-sensitive and
effectively self-referential: running the suite could re-enter the very
canary/self-deploy path under test, and `verify_canary`'s own unit test
deliberately **excludes** `UnitTest` for exactly this reason (it would recurse
into `cargo test` and run for 30+ minutes).

The gate is now scoped to a deterministic, **non-recursive** invocation:

```text
cargo test --lib --manifest-path <candidate>/Cargo.toml --target-dir <canary_target_dir>
```

- `--lib` restricts execution to the crate's own library unit tests, which do not
  spawn the canary/self-deploy integration path, so the gate cannot re-enter
  itself or deadlock on the already-held `BuildLock`.
- It still compiles and exercises the candidate's unit tests, so a genuine unit
  regression in the candidate is still caught and surfaces as a
  `canary_gate_failed` with `gate = "unit-test"`.

The gate is **not** disabled, skipped, or weakened — it is correctly scoped. The
integration-level coverage that the full suite once provided remains the job of
the `Smoke`, `GymBaseline`, and `RpcHealth` gates, which each exercise the fully
built candidate binary end-to-end.

> **Scope boundary.** `unit-test` now asserts *unit* correctness of the candidate
> library. Cross-crate / integration regressions are the responsibility of the
> binary-exercising gates (`smoke`, `gym-baseline`, `rpc-health`). This boundary
> is intentional and documented so a future reader does not "restore" the full
> suite and reintroduce the recursion.

## Compatibility guarantees

- **`DeployRefusal::RedCanary` `Display` is byte-for-byte unchanged**:
  `red canary (one or more gates failed)`. Existing assertions such as
  `format!("{err}").contains("red canary")` keep passing. The structured `gate`
  field *augments* the human string; it never replaces it.
- **`skipped ⇒ passed` semantics are preserved untouched.** A skipped gate (a
  legitimately-scoped absence signal, e.g. the hardened `RpcHealth` /
  `endpoint_absent()` logic) still counts as passed. This feature adds **no** new
  skip or broadening path that could mask a red gate.
- **No new sinks.** Fields flow only to the existing `tracing-opentelemetry`
  exporter. No `print!` / `println!` / `eprintln!` is used in any production
  path.
- **All new fields are additive `Option`s** with a `None` default, so external
  struct literals and existing constructors compile unchanged.

## Examples

### A red `unit-test` gate (summary event)

```text
WARN overseer::deploy: canary refused deploy
  root_cause="canary_gate_failed" gate="unit-test"
  detail="tests failed (exit status: 101): test result: FAILED. 1 passed; 2 failed…[truncated]"
  passed_gates=1 total_gates=4 target_commit="9f3c1ab77e02"
```

### A candidate that fails to build (build event)

```text
WARN overseer::deploy: canary refused deploy
  root_cause="canary_build_failed"
  detail="error[E0599]: no method named `tick` found for struct `Overseer`…[truncated]"
  passed_gates=0 total_gates=0 target_commit="9f3c1ab77e02"
```

Note the absence of a `gate` field — no gate ran, so attribution is to the build,
not to a gate. `total_gates=0` (no gate attempted) is what distinguishes this from
a harness/infra fault.

### A harness/infra fault (infra event)

```text
WARN overseer::deploy: canary refused deploy
  root_cause="canary_infra_error" gate="target-canary"
  detail="could not acquire the self-deploy build lock (another self-deploy build may be running)…"
  passed_gates=0 total_gates=4 target_commit="9f3c1ab77e02"
```

### OpenTelemetry span attributes

The same fields arrive on the exported span via the `tracing-opentelemetry`
layer, so a backend query can group deploy refusals by gate:

```text
otel.name         = "canary refused deploy"
overseer.root_cause = "canary_gate_failed"
overseer.gate       = "unit-test"
overseer.passed_gates = 1
overseer.total_gates  = 4
overseer.target_commit = "9f3c1ab77e02"
```

Example backend query (pseudo-PromQL / trace-search): *count deploy refusals in
the last 6h grouped by `overseer.gate`* — this is what turns a monotonically
climbing `DeployDrift` into a single named culprit gate.

### Reading the typed gate in code

```rust
let canary = runner.run_canary(target_commit)?;
if !canary.passed {
    match canary.failing_gate {
        Some(gate) => tracing::warn!(
            target: "overseer::deploy",
            root_cause = "canary_gate_failed",
            gate = %gate,
            "red canary attributed to a concrete gate",
        ),
        None => { /* build or infra fault — see canary.detail / root_cause */ }
    }
}
```

## Tutorial: attribute a red-canary tick to a gate

Before this feature the diagnosis loop was: *see `red canary` → reproduce the
whole build-and-verify locally (30+ min) → guess which gate.* Now:

1. **Grep the tick log for the summary target.** Every refusal is one line:

   ```bash
   journalctl -u simard | grep 'target=overseer::deploy' | grep canary
   # or, if using a JSON subscriber:
   simard logs --json | jq 'select(.target=="overseer::deploy" and .fields.root_cause!=null)'
   ```

2. **Read the `root_cause` + `gate` fields.**
   - `canary_gate_failed` + `gate="unit-test"` → a candidate unit test is red;
     read `detail` for the first failing test, then reproduce just that test with
     `cargo test --lib <name>`.
   - `canary_build_failed` (no `gate`) → the candidate does not compile; `detail`
     has the first compiler error.
   - `canary_infra_error` + `gate="target-canary"` → not a candidate regression;
     the harness could not run (e.g. build-lock contention). Check for a stuck
     concurrent self-deploy or a stale `cargo_build.lock`.

3. **Confirm the fix advances the running binary.** Once the attributed gate is
   green, the next tick's canary passes, `evaluate_deploy_gate` returns `Ok(())`,
   the swap runs, and `DeployDrift` **decreases** (merged work reaches the running
   binary). Future ticks that still red will now name a *different* concrete gate,
   so drift is always attributable.

For the surrounding deploy gate, notification contract, and drift signal, see the
[Self-deploy API reference](./self-deploy-api.md).

## See also

- [Self-deploy API reference](./self-deploy-api.md) — the deploy gate, `DeployRefusal`, `CanaryResult`, and `GateFailed` this feature extends
- [Self-deploy source-prep reference](./self-deploy-source-prep.md) — `prepare_build_and_verify_canary` and the `BuildLock`-serialized critical section
- [Overseer tick details](./overseer-tick-details.md) — the OODA tick the drift observe/decide/act rail rides on
- [Overseer root-cause ("WHY") API](./overseer-root-cause-why-api.md) — the broader root-cause attribution model these tags feed
- [reconcile-and-self-deploy concept](../concepts/reconcile-and-self-deploy.md) — the merged-but-not-running gap this rail closes
- [How to verify and roll back a self-deploy](../howto/verify-and-roll-back-a-self-deploy.md)
