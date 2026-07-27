---
title: Cognitive-thread observability and Overseer oversight
description: >
  Reference for the per-thread telemetry and Overseer oversight of Simard's
  cognitive threads. Documents the simard.thread.<id>.* metric catalog (runs /
  successes / failures counters, duration_seconds histogram, and
  last_run_epoch / next_run_epoch / active gauges), how those series flow through
  the in-process registry into metrics_snapshot.json, the single-source-of-truth
  thread registry (purpose + cadence via the CognitiveThread trait and
  Mind::health), the deterministic thread-oversight rail that turns telemetry +
  ooda.log into anomalies, and the durable error path (failure sink →
  FailureCause::CognitiveThread → Signal::StepFailureDiagnosed) that makes every
  thread error visible to the Overseer.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./telemetry-metrics.md
  - ./cognitive-threads-catalog.md
  - ./cognitive-thread-scheduling.md
  - ../concepts/unified-telemetry-and-status.md
  - ./status-snapshot-api.md
  - ../howto/add-a-new-cognitive-thread.md
---

# Cognitive-thread observability and Overseer oversight

Simard's [cognitive threads](./cognitive-threads-catalog.md) — the OODA loop plus
the reflective threads (metacognition, consolidation, reflection, prospection,
salience, operator_model, analogy, values_deliberation, interoception, narrative,
creative_ideas, engineer_log_analysis, maintenance) — are **fully observable and
supervised**. Each thread emits real OpenTelemetry instruments through the shared
[telemetry facade](./telemetry-metrics.md#facade-api), those series surface in
`metrics_snapshot.json`, and the acting Overseer enumerates every thread from a
single-source-of-truth registry, reads its telemetry and `~/.simard/ooda.log`
signal, and records anomalies. Errors raised inside a thread flow to the Overseer
through **both** a `failures` counter **and** a durable diagnosis signal.

> **Modules:** metric names in `src/telemetry/names.rs`; the thread telemetry
> seam in `src/cognitive_threads/telemetry.rs`; the registry seam in
> `src/cognitive_threads/thread.rs` + `src/cognitive_threads/mind.rs`; the
> deterministic oversight rail in `src/overseer/thread_oversight.rs`, wired in
> `src/overseer/mod.rs`; the durable error channel in `src/overseer/failure_sink.rs`
> + `src/overseer/diagnosis.rs`.

## Design in one paragraph

The thread telemetry seam **dual-writes**: it keeps the structured `tracing`
events it always emitted **and** mirrors every run through the unified facade
(`counter_add` / `histogram_record` / `gauge_set`) onto the shared `simard`
meter. Thread identity is embedded in the **metric name**
(`simard.thread.<id>.<suffix>`) — never as an attribute — so ~14 threads × 7
suffixes ≈ 100 fixed series sit far under the registry's cardinality cap while
avoiding the per-key attribute-value cliff. Those generic series ride the
existing registry → snapshot → OTLP path with no schema bump. The Overseer's
`purpose`/`cadence` knowledge comes from the `CognitiveThread` trait itself (a
`purpose()` default method plus the existing `policy()`), enumerated via
`Mind::health()`, so there is no hand-maintained duplicate list. A thin,
deterministic oversight rail reads the snapshot + a bounded `ooda.log` tail and
appends anomaly strings to the Overseer's `ObservedState`, reusing the existing
`Signal::Anomaly` fan-out.

## Metric catalog — `simard.thread.*`

Every metric name is `simard.thread.<id>.<suffix>`, where `<id>` is the thread's
stable `snake_case` [`CognitiveThread::id`](./cognitive-threads-catalog.md) (a
compile-time constant) and `<suffix>` is one of the seven signals below. There
are **no attributes** on these series — identity lives in the name.

| Suffix | Type | Meaning |
|---|---|---|
| `runs` | counter | Every attempt the thread actually ran (`ThreadOutcome::ran == true`). Denominator for success/failure rate. |
| `successes` | counter | Runs that completed successfully (`success == true`). |
| `failures` | counter | Runs that failed or panicked (`success == false`). Incremented on the same branch that records the durable diagnosis (see [Error propagation](#error-propagation)). |
| `duration_seconds` | histogram | Per-run wall-clock duration, in seconds. Uses the shared `DAEMON_CYCLE_DURATION_BUCKETS` boundaries; `count` + `sum` give an honest average even for sub-second runs. |
| `last_run_epoch` | gauge | Unix epoch (seconds) of the last completed run. `now − last_run_epoch` is the **last-run age** the Overseer and dashboard derive; it is never stored as a decaying value. |
| `next_run_epoch` | gauge | Unix epoch (seconds) of the next scheduled run, from the scheduler's `next_run` bookkeeping. The primary **stall** signal. |
| `active` | gauge | `1` while the thread is mid-tick, `0` otherwise (set by the RAII `enter_active` guard). |

`runs = successes + failures` by construction, so success rate is
`successes / runs` and failure rate is `failures / runs` — both derivable at zero
extra cardinality.

### Example series

For the OODA thread (`id = "ooda"`):

```text
simard.thread.ooda.runs              (counter)
simard.thread.ooda.successes         (counter)
simard.thread.ooda.failures          (counter)
simard.thread.ooda.duration_seconds  (histogram)
simard.thread.ooda.last_run_epoch    (gauge)
simard.thread.ooda.next_run_epoch    (gauge)
simard.thread.ooda.active            (gauge)
```

The full `<id>` set is the 14 thread ids: `ooda`, `maintenance`,
`engineer_log_analysis`, `creative_ideas`, `metacognition`, `consolidation`,
`reflection`, `prospection`, `salience`, `operator_model`, `analogy`,
`values_deliberation`, `interoception`, `narrative`.

### Name constants

The names are single-sourced in `src/telemetry/names.rs`, following the same
constant-not-literal rule as the rest of the catalog:

```rust
/// Prefix for all per-cognitive-thread series: `simard.thread.<id>.<suffix>`.
/// `<id>` is a compile-time `CognitiveThread::id` constant; identity is embedded
/// in the NAME (never an attribute) to bypass the per-key value cardinality cap.
pub const THREAD_METRIC_PREFIX: &str = "simard.thread";

pub const THREAD_SUFFIX_RUNS: &str = "runs";
pub const THREAD_SUFFIX_SUCCESSES: &str = "successes";
pub const THREAD_SUFFIX_FAILURES: &str = "failures";
pub const THREAD_SUFFIX_DURATION_SECONDS: &str = "duration_seconds";
pub const THREAD_SUFFIX_LAST_RUN_EPOCH: &str = "last_run_epoch";
pub const THREAD_SUFFIX_NEXT_RUN_EPOCH: &str = "next_run_epoch";
pub const THREAD_SUFFIX_ACTIVE: &str = "active";
```

## The thread telemetry seam

All emission still funnels through `src/cognitive_threads/telemetry.rs` — the
single seam every thread uses — so callers (the `Mind` scheduler) are unchanged
except for one added argument. The seam now **dual-writes** the `tracing` event
and the facade metric:

```rust
// src/cognitive_threads/telemetry.rs (illustrative)

/// Record a completed run: emits the tracing span/event AND mirrors the
/// counters + duration histogram + last_run_epoch gauge through the facade.
pub fn record_run(id: &str, outcome: &ThreadOutcome, run_epoch: u64) {
    // ... existing tracing::info_span! / tracing::info! ...

    telemetry::counter_add(&metric_name(id, names::THREAD_SUFFIX_RUNS), 1, &[]);
    if outcome.success {
        telemetry::counter_add(&metric_name(id, names::THREAD_SUFFIX_SUCCESSES), 1, &[]);
    } else {
        telemetry::counter_add(&metric_name(id, names::THREAD_SUFFIX_FAILURES), 1, &[]);
    }
    telemetry::histogram_record(
        &metric_name(id, names::THREAD_SUFFIX_DURATION_SECONDS),
        outcome.duration.as_secs_f64(),
        &[],
    );
    telemetry::gauge_set(
        &metric_name(id, names::THREAD_SUFFIX_LAST_RUN_EPOCH),
        run_epoch as i64,
        &[],
    );
}
```

`record_next_run` mirrors `next_run_epoch`, `record_error` bumps `failures` and
records the durable diagnosis, and the `enter_active`/`ActiveGuard` pair drives
the `active` gauge to `1`/`0`.

> **Suffix reconciliation.** The pre-instrumentation seam emitted only a
> `tracing`-side `simard.thread.<id>.errors` metric field and had no
> `successes` / `last_run_epoch` signals. Instrumentation standardises the error
> counter on the **`failures`** suffix (per the [`names.rs` constants](#name-constants))
> and adds the `successes` counter and `last_run_epoch` gauge. `record_error`'s
> `errors` name is replaced by `failures`, not kept alongside it — there is no
> legacy `simard.thread.<id>.errors` series after this change.

Because the `Mind::execute` scheduler already calls `enter_active` /
`record_run` / `record_error` / `record_next_run` for every thread, **no
scheduler or thread-logic change is required** for instrumentation — only the
added `run_epoch` argument at the sole `record_run` call site (where `now` is
already in scope as `entry.last_run`).

> **Honesty (zero-BS).** Every value reflects real activity. There are no
> hardcoded or synthesized "healthy" numbers — a fabricated metric would hide a
> dead thread, which is exactly the failure mode this feature exists to catch.
> Before the facade is initialised (e.g. in unit tests), the global meter is the
> SDK no-op and the in-process registry is the source of truth, so recording is a
> cheap no-op and nothing is fabricated.

## Snapshot surfacing

The `simard.thread.*` series are **generic counters/histograms/gauges**, so they
flow through `src/telemetry/registry.rs` → `capture()` →
`metrics_snapshot.json` automatically via the daemon's per-cycle snapshot flush.
**No schema bump** (`SCHEMA_VERSION` stays `1`) and no new snapshot field.

Read them back with the standard [`MetricsSnapshot`](./telemetry-metrics.md#in-process-registry-and-the-on-disk-snapshot)
accessors — attributes are empty:

```rust
// `snapshot::read` returns `Option<MetricsSnapshot>` (None if the file is
// missing/unreadable); fall back to an empty snapshot.
let snap = telemetry::snapshot::read(&snapshot_path(state_root))
    .unwrap_or_else(MetricsSnapshot::empty);

let runs      = snap.counter("simard.thread.ooda.runs", &[]).unwrap_or(0);
let failures  = snap.counter("simard.thread.ooda.failures", &[]).unwrap_or(0);
let last_run  = snap.gauge("simard.thread.ooda.last_run_epoch", &[]);
let next_run  = snap.gauge("simard.thread.ooda.next_run_epoch", &[]);
let latency   = snap.histogram("simard.thread.ooda.duration_seconds", &[]);

let success_rate = if runs > 0 {
    (runs - failures) as f64 / runs as f64
} else {
    0.0
};
```

Because oversight is **snapshot-file-driven**, it works even though the Overseer
runs out-of-process from the `Mind` — liveness detection needs only the flushed
file, not a live handle to the scheduler.

## Thread registry — single source of truth

The Overseer's per-thread `purpose` and expected `cadence` come from the thread
definitions themselves, not a duplicated list.

### `purpose()` on the trait

`CognitiveThread` gains a default-implemented `purpose()`:

```rust
pub trait CognitiveThread: Send {
    fn id(&self) -> &str;
    // ...

    /// One-line, human-facing statement of the thread's original intent.
    /// Surfaced to the Overseer registry and the dashboard. Defaults to the id.
    fn purpose(&self) -> &'static str {
        "(no purpose declared)"
    }
}
```

Each thread overrides it with a one-liner reusing its existing doc-comment
intent, e.g.:

```rust
// src/cognitive_threads/threads/metacognition.rs
fn purpose(&self) -> &'static str {
    "Self-audit of reasoning quality: reviews recent decisions for drift."
}
```

The Overseer's own sensor thread (`src/overseer/sensor.rs`) declares its purpose
the same way, so it appears in the registry alongside the reflective threads.

### `cadence` from `policy()`

Expected cadence is **derived** from the existing
[`SchedulePolicy`](./cognitive-thread-scheduling.md), not stored twice. An
`Interval(d)` (or `Adaptive { current, .. }`) policy yields `Some(d.as_secs())`;
`OnDemand` / `EventDriven` yield `None` (no cadence to be "late" against).

### Enumeration via `Mind::health()`

`ThreadHealth` is extended additively so the health feed carries the registry
facts:

```rust
pub struct ThreadHealth {
    pub id: String,
    pub enabled: bool,
    pub last_run_epoch: Option<u64>,
    pub next_run_epoch: Option<u64>,
    pub last_success: Option<bool>,
    pub consecutive_errors: u32,
    pub backoff_until_epoch: Option<u64>,

    /// One-line original purpose (from `CognitiveThread::purpose`).
    pub purpose: String,
    /// Expected cadence in seconds (derived from `policy()`); `None` for
    /// on-demand / event-driven threads.
    pub cadence_secs: Option<u64>,
}
```

`Mind::health()` populates `purpose` and `cadence_secs` for every registered
thread. This is the single registry seam the Overseer enumerates — name +
purpose + cadence in one place, sourced from the definitions.

> New `ThreadHealth` fields are additive and the struct derives `Serialize`, so
> existing (non-strict, `serde_json::Value`-based) dashboard consumers keep
> working without change.

## Overseer oversight

On each Observe pass, the Overseer evaluates every thread against a thin,
**pure, deterministic** rail in `src/overseer/thread_oversight.rs`:

```rust
/// Detect cognitive-thread anomalies from telemetry + registry + an ooda.log
/// tail. Pure, bounded, panic-free: malformed input degrades to "no anomaly",
/// never a panic. Returns bounded, control-char-stripped anomaly strings.
pub fn detect_thread_anomalies(
    snapshot: &MetricsSnapshot,
    registry: &[ThreadHealth],
    ooda_tail: &str,
    now_epoch: u64,
) -> Vec<String>;
```

The rules:

1. **Stalled** — `next_run_epoch` is in the past by more than a tolerance of its
   expected `cadence_secs` (the thread should have run and did not).
2. **Stale** — `now − last_run_epoch` far exceeds the cadence (the thread has
   gone silent).
3. **Failure-rate** — lifetime `failures / runs` exceeds a threshold (a
   slow-burn backstop; the recent, sharp signal comes from the per-failure
   diagnosis below).
4. **Log errors** — a bounded scan of the `ooda.log` tail for the thread's id
   alongside an `ERROR` line (fixed literal `contains`, never a backtracking
   regex).

The wiring in `src/overseer/mod.rs::run_cycle` extends `observed.anomalies`
beside the existing `drain_recent()` call, reusing the existing
`Signal::Anomaly { detail }` fan-out — no new scraping mechanism, no new
notification path. Anomalies also appear in `ObservedState.anomalies`, which the
status snapshot already surfaces.

### ooda.log tail reader

Oversight reads `~/.simard/ooda.log` (the daemon's log; **not** journald) with a
bounded tail reader (`read_tail(path, n)`), reusing the existing bounded reader
from the operator dashboard. It has a byte ceiling — **never a full-file read** —
decodes lossily, and tolerates truncated/partial final lines without panicking.

## Error propagation

Errors raised inside a thread are made visible to the Overseer through **two
channels** so they are both measurable and actionable:

1. **The `failures` counter** — `record_error` / the `!success` branch of
   `Mind::execute` bumps `simard.thread.<id>.failures`.
2. **A durable diagnosis** — the same branch records a
   [`FailureDiagnosis`](./telemetry-metrics.md) into the process-global failure
   sink (`overseer::failure_sink::record_step_failure`). A new
   `FailureCause::CognitiveThread` variant (kebab label `cognitive-thread`)
   classifies it, with the thread id + bounded outcome summary as evidence.

The Overseer `drain_recent()`s the sink once per Observe pass and lifts each
diagnosis into a `Signal::StepFailureDiagnosed { cause, exit_code, evidence }`.
So a thread error is never swallowed: it shows up as a counter increment **and**
a durable signal the Overseer acts on.

```rust
// src/cognitive_threads/mind.rs (execute, !success branch — illustrative)
telemetry::record_error(&id, &outcome.summary); // bumps `failures`
overseer::failure_sink::record_step_failure(FailureDiagnosis {
    cause: FailureCause::CognitiveThread,
    exit_code: None,
    evidence: bounded(&format!("thread {id}: {}", outcome.summary)),
});
```

> **Security / honesty guardrails.** Thread ids in metric names come only from
> the compile-time id enum (validated `^[a-z0-9_]{1,32}$` if ever runtime-derived).
> Every string surfaced to the snapshot, an anomaly, or a diagnosis is truncated
> (≤256 chars) and control-char-stripped to prevent log-forging / terminal-escape
> injection into the operator TUI. Anomaly emissions are capped per cycle and ride
> the existing signal dedup so a flapping thread cannot flood notifications. No
> secrets/PII ever appear in a metric name, attribute, or evidence string.

## Configuration

There are **no new configuration knobs** — per-thread telemetry uses the same
[telemetry configuration](./telemetry-metrics.md#configuration) as every other
Simard metric:

| Variable | Effect | Default |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Enables OTLP export for traces **and** metrics (including `simard.thread.*`). Unset = in-process/registry only. | unset (export off) |
| `SIMARD_STATE_ROOT` | State root holding `telemetry/metrics_snapshot.json` and `ooda.log`. | `$HOME/.simard` |

Whether a given thread emits at all is governed by its existing enable gate
(`CognitiveThread::enabled`) and the per-thread env gates documented in the
[cognitive-threads catalog](./cognitive-threads-catalog.md) — a disabled thread
never ticks and therefore emits no series.

## Examples

### Compute per-thread health from the snapshot

```rust
use simard::telemetry::snapshot::{self, snapshot_path};

let snap = snapshot::read(&snapshot_path(state_root)).unwrap_or_else(snapshot::MetricsSnapshot::empty);

for id in ["ooda", "metacognition", "reflection"] {
    let runs = snap.counter(&format!("simard.thread.{id}.runs"), &[]).unwrap_or(0);
    let fails = snap.counter(&format!("simard.thread.{id}.failures"), &[]).unwrap_or(0);
    let last = snap.gauge(&format!("simard.thread.{id}.last_run_epoch"), &[]);
    println!(
        "{id}: {runs} runs, {fails} failures, last_run_epoch={last:?}",
    );
}
```

### Inspect the raw series on disk

```console
$ jq '.counters[] | select(.name | startswith("simard.thread."))' \
    ~/.simard/telemetry/metrics_snapshot.json
{
  "name": "simard.thread.ooda.runs",
  "attrs": [],
  "value": 42
}
{
  "name": "simard.thread.ooda.failures",
  "attrs": [],
  "value": 1
}
```

### Read the Overseer-observed anomalies

Thread anomalies surface in `ObservedState.anomalies` and as
`Signal::Anomaly { detail }`. Each string is **stable per (thread, condition)**
across Observe passes — it never embeds a live, per-cycle-varying magnitude
(seconds-overdue, failure counts, a log excerpt) — so the Overseer's launch /
recurrence / write-back dedup gates collapse a persistently unhealthy thread to
a single investigation instead of re-launching one every cycle. The live
magnitudes are recoverable by the investigation from the telemetry snapshot and
`ooda.log` directly. Examples:

```text
telemetry anomaly: cognitive thread 'reflection' stalled: scheduled run is overdue past its 600s grace window (cadence 300s) — purpose: …
telemetry anomaly: cognitive thread 'salience' failing: the majority of its recorded runs have failed — purpose: …
telemetry anomaly: ooda.log tail contains recent ERROR line(s) — see the daemon log for detail
```

## Testing

The feature ships with focused tests (see `src/telemetry/snapshot.rs`,
`src/overseer/thread_oversight.rs`, and `src/overseer/` test modules):

- **Snapshot presence** — after simulated runs, `capture()` contains the
  per-thread `runs` / `successes` / `failures` counters, the `duration_seconds`
  histogram, and the `last_run_epoch` / `next_run_epoch` gauges, asserted via
  `MetricsSnapshot::counter()` / `gauge()` / `histogram()`.
- **Error path** — an injected thread failure increments
  `simard.thread.<id>.failures` **and** records a
  `FailureDiagnosis { cause: FailureCause::CognitiveThread, .. }` that drains
  into `ObservedState` as a `Signal::StepFailureDiagnosed`.
- **Stall detection** — an overdue `next_run_epoch` yields exactly one anomaly
  string from `detect_thread_anomalies`, and malformed input yields none (the
  rail degrades to "no anomaly", never a panic).

Quality gates are the standard project gates: `cargo build`,
`cargo clippy -- -D warnings`, targeted `cargo test`, and the full pre-commit /
required CI — all green, no `--no-verify` / `--admin`.

## See also

- [Telemetry metrics reference](./telemetry-metrics.md) — the facade, registry,
  snapshot, and OTLP gating these series ride on.
- [Cognitive-threads catalog](./cognitive-threads-catalog.md) — each thread's
  id, kind, cadence, and env gate.
- [Cognitive-thread scheduling](./cognitive-thread-scheduling.md) — `SchedulePolicy`
  and how `next_run_epoch` / cadence are computed.
- [Unified telemetry and one `simard status`](../concepts/unified-telemetry-and-status.md)
  — the design rationale for the facade.
