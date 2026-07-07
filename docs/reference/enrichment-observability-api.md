---
title: Enrichment observability API reference
description: The authoritative contract for proving — with live evidence — that recalled memory reaches Simard's OODA decisions (#2942). Covers the enrich_turn_input / EnrichmentSource::resolve instrumentation seam, the per-turn simard::enrichment INFO line and the fail-loud degrade WARN, the simard.enrichment.* metric catalog (decisions{attached}, degraded{reason}, and the preamble_bytes / facts_injected / procedures_injected histograms), the per-OODA-cycle EnrichmentRollup drain into metrics_snapshot.json, the auth-gated GET /api/enrichment endpoint and its degrade-safe schema, the dashboard Memory-tab "Recall reaching decisions" panel, the enrichment_ablation_delta feed into the hybrid self-measurement (#2644), and the simard gym enrichment-ablation recall-on-vs-recall-off eval.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/enrichment-observability.md
  - ../howto/verify-recall-reaches-decisions.md
  - ./telemetry-metrics.md
  - ./recall-precision-hybrid-api.md
  - ./base-type-adapters.md
  - ./dashboard-memory-tab.md
  - ../concepts/hybrid-cognition-measurement.md
  - ../../src/base_type_turn.rs
  - ../../src/enrichment_observability/mod.rs
  - ../../src/telemetry/names.rs
  - ../../src/operator_commands_dashboard/enrichment.rs
---

# Enrichment observability API reference

> **Status: implemented.** The instrumentation lives at the
> [`enrich_turn_input`](https://github.com/rysweet/Simard/blob/main/src/base_type_turn.rs)
> and `launch_enrichment_bridges` seam, emits through the
> [`enrichment_observability`](https://github.com/rysweet/Simard/blob/main/src/enrichment_observability/mod.rs)
> module, drains a per-cycle rollup into `metrics_snapshot.json`, and surfaces on
> the dashboard Memory tab via the auth-gated
> [`GET /api/enrichment`](#endpoint-get-apienrichment). The hard-proof ablation
> ships as `simard gym enrichment-ablation`.

This reference specifies the API of the enrichment-observability surface: what is
emitted, where, in what shape, and with what guarantees. For the rationale, see
the [concept](../concepts/enrichment-observability.md); for the operator
playbook, see the [how-to](../howto/verify-recall-reaches-decisions.md).

## Contents

- [The instrumented seam](#the-instrumented-seam)
- [What `attached` means](#what-attached-means)
- [Per-turn tracing contract](#per-turn-tracing-contract)
- [Metric catalog: `simard.enrichment.*`](#metric-catalog-simardenrichment)
- [Per-cycle rollup: `EnrichmentRollup` → `metrics_snapshot.json`](#per-cycle-rollup-enrichmentrollup-metrics_snapshotjson)
- [Endpoint: `GET /api/enrichment`](#endpoint-get-apienrichment)
- [Dashboard Memory-tab panel](#dashboard-memory-tab-panel)
- [Ablation eval: `simard gym enrichment-ablation`](#ablation-eval-simard-gym-enrichment-ablation)
- [Hybrid self-measurement feed (#2644)](#hybrid-self-measurement-feed-2644)
- [Configuration](#configuration)
- [Security properties](#security-properties)
- [Tests](#tests)
- [Guarantees and non-guarantees](#guarantees-and-non-guarantees)
- [What is unchanged](#what-is-unchanged)
- [See also](#see-also)

## The instrumented seam

Enrichment is applied at one shared entry point used by every base-type adapter,
[`base_type_turn::enrich_turn_input`](./base-type-adapters.md):

```rust
pub fn enrich_turn_input(
    input: &BaseTypeTurnInput,
    memory_client: Option<&dyn CognitiveMemoryOps>,
    knowledge_client: Option<&KnowledgeClient>,
    // Added for #2942. Was enrichment *configured* for this session
    // (`EnrichmentSource::Native`)? This separates an expected-but-degraded
    // bridge (`expected=true, attached=false` → WARN, counted) from a session
    // that never wired one (`expected=false` → INFO, uncounted). It is a
    // required parameter because `attached = memory_client.is_some()` alone
    // cannot distinguish the two: a fully-degraded `Native` source collapses
    // both clients to `None` exactly like `Disabled` does.
    expected: bool,
) -> SimardResult<BaseTypeTurnInput>;
```

> **Signature change.** The `expected` parameter is the one addition #2942 makes
> to the shared seam (today it is the 3-argument form). See
> [threading `expected`](#threading-expected) for how the flag is sourced without
> recomputing it from the post-resolve bridges.

Two instrumentation calls are added, both routed through the
`enrichment_observability` module so the emit logic is a single, unit-testable
choke point:

| Site | When | Call | Emits |
|---|---|---|---|
| `enrich_turn_input` (after render) | Every turn (the log line always; the metrics only when enrichment was *expected* — see [population](#which-turns-are-counted-population)) | `enrichment_observability::observe(EnrichmentObservation { .. })` | The per-turn `INFO`/`WARN` line + (for expected turns) `simard.enrichment.decisions` + the three magnitude histograms |
| `launch_enrichment_bridges` (each degrade arm) | On a bridge launch failure | `enrichment_observability::observe_degrade(DegradeReason, &error)` | A `WARN` line + `simard.enrichment.degraded{reason}` |

`observe` is called **after** `render_enrichment_block`, so the counts and byte
size reflect exactly what was injected into `prompt_preamble` — not the candidate
set that preparation gathered.

```rust
/// One decision's enrichment outcome. Content-free by construction:
/// carries counts and sizes, never fact/procedure text.
pub struct EnrichmentObservation<'a> {
    /// The decision's objective *text* (not a normalized slug). Truncated +
    /// control-stripped before it reaches the log line; NEVER a metric attribute.
    pub objective: &'a str,
    /// True iff the cognitive-memory bridge resolved to `Some`.
    pub attached: bool,
    /// Whether enrichment was *configured* for this session
    /// (`EnrichmentSource::Native`), independent of whether any bridge attached.
    /// Threaded in from the caller (see [threading `expected`](#threading-expected)) —
    /// never derived from `attached`, because a fully-degraded `Native` source
    /// and an unconfigured session are indistinguishable post-resolve.
    /// `expected && !attached` is the degrade that raises a `WARN` and is
    /// counted; `!expected` is a benign unconfigured turn (`INFO`, uncounted).
    pub expected: bool,
    /// Facts actually rendered into the preamble (post-cap, post-render).
    pub facts_injected: usize,
    /// Procedures actually rendered into the preamble (post-cap, post-render).
    pub procedures_injected: usize,
    /// Byte length of the rendered enrichment block injected into the preamble.
    /// This is the *full* block — memory facts + procedures **and** any domain
    /// knowledge — so a knowledge-only turn has `preamble_bytes > 0` while
    /// `facts_injected == procedures_injected == 0`. See the note under the
    /// metric catalog.
    pub preamble_bytes: usize,
}

pub enum DegradeReason {
    /// `crate::ooda_loop::connect_memory` failed (e.g. memory-ipc Broken pipe).
    MemoryIpc,
    /// `launch_knowledge_client_native` failed.
    KnowledgeLaunch,
}
```

## What `attached` means

`attached` is deliberately narrow:

```text
attached  ==  memory_client.is_some()
```

It is **true only when the cognitive-memory bridge resolved to `Some`** — i.e.
recalled *memory* reached the decision. It is **not** the combined
memory-or-knowledge bundle: a turn enriched with domain *knowledge* but no memory
recall is `attached=false`, because #2942 is about *recalled memory* reaching
decisions. `facts_injected`/`procedures_injected` can still be `0` while
`attached=true` (the bridge is up but the store is empty for this objective);
that is a true, useful signal and is reported as such.

### Which turns are counted (population)

`enrich_turn_input` is the **shared** seam
([`base_types::enrich_input`](./base-type-adapters.md)) that every adapter turn
flows through. Two distinct call paths reach it with **no** configured
enrichment, and both are benign (`expected=false`):

- Adapters that never override `enrichment()` take the default
  `enrich_input` no-bridge branch: `enrich_turn_input(input, None, None, /*expected=*/ false)`.
- Enrichment-capable adapters built **without** `with_enrichment` resolve the
  `EnrichmentSource::Disabled` default into an *empty* `EnrichmentClients`, whose
  `enrich` still calls the seam — but with `expected=false` (see
  [threading `expected`](#threading-expected)).

Those turns are `attached=false` **by configuration, not by degrade**, so the
instrumentation must not treat them as failures. Two rules keep the signal
honest:

- **`WARN` is reserved for `expected && !attached`.** A per-turn degrade `WARN`
  is emitted only when the session *configured* enrichment
  (`EnrichmentSource::Native`) yet the memory bridge resolved to `None`. A
  session that never configured enrichment logs an `INFO`
  (`expected=false`), not a `WARN` — otherwise every non-enriching adapter would
  cry wolf on every turn and bury the real memory-IPC degrade.
- **The attach-rate population is the *expected* turns only.** The
  `simard.enrichment.*` metrics (and therefore the dashboard attach-rate) are
  recorded only for expected turns; unconfigured turns emit the log line
  but do **not** increment the counters. Without this, a healthy daemon's
  attach-rate would drop below 100% merely because some other adapter ran
  without enrichment.

#### Threading `expected`

These two rules require the seam to know whether enrichment was *expected*, and
that flag must be **captured before resolution collapses the distinction**.
`attached = memory_client.is_some()` alone cannot tell "bridge degraded" from
"never configured", because `EnrichmentSource::Native` with *both* bridges
degraded resolves to the same empty `(None, None)` bundle as
`EnrichmentSource::Disabled`. So `expected` is sourced structurally, not
inferred:

1. `EnrichmentSource::resolve` records provenance on the `EnrichmentClients` it
   returns — an `expected: bool` set `true` for `Native` and `false` for
   `Disabled`. This is set at launch time, so a fully-degraded `Native` source
   still carries `expected=true`.
2. `EnrichmentClients::enrich` forwards `self.expected` as the new
   `enrich_turn_input(.., expected)` argument.
3. The default `BaseTypeSession::enrich_input` no-bridge branch passes
   `expected=false` explicitly.

`enrich_turn_input` then places this flag on the `EnrichmentObservation` it hands
to `observe`, so the emit choke point decides `INFO`-vs-`WARN` and counted-vs-not
from a single, trustworthy field. This is the one behavioural addition to
`EnrichmentSource` / `EnrichmentClients`; it changes no recall, ranking, or
dispatch behaviour.

## Per-turn tracing contract

Every turn emits one structured line under the tracing target
`simard::enrichment`:

**Attach path (`INFO`):**

```text
INFO simard::enrichment: enrichment applied attached=true facts=7 procedures=3 preamble_bytes=812 objective="raise unit-test coverage on the goal-board store"
```

**Attach path with an empty store (`INFO`, still emitted):**

```text
INFO simard::enrichment: enrichment applied attached=true facts=0 procedures=0 preamble_bytes=0 objective="triage stale pull requests"
```

**Memory *expected* but degraded (`WARN`, per turn):** emitted only when the
session configured enrichment (`EnrichmentSource != Disabled`) yet the memory
bridge resolved to `None`.

```text
WARN simard::enrichment: enrichment degraded — memory bridge expected but not attached; decision proceeding without recalled memory attached=false expected=true facts=0 procedures=0 preamble_bytes=0 objective="triage stale pull requests"
```

**Enrichment *not configured* (`INFO`, per turn):** the benign `Disabled`-source
case (lightweight callers, non-enriching adapters, tests). Logged for
completeness but **not** a degrade and **not** counted in the attach-rate.

```text
INFO simard::enrichment: enrichment not configured for this session attached=false expected=false facts=0 procedures=0 preamble_bytes=0 objective="run local-harness smoke"
```

**Degrade at bridge launch (`WARN`, with concrete reason), from `launch_enrichment_bridges`:**

```text
WARN simard::enrichment: cognitive-memory bridge unavailable — memory enrichment disabled for this session reason=memory_ipc
WARN simard::enrichment: knowledge bridge unavailable — knowledge enrichment disabled for this session reason=knowledge_launch
```

Contract:

- The `objective` value is **truncated to ≤120 bytes and control-character
  stripped** before emission, and appears **only on the log line** — never as a
  metric attribute.
- The per-turn line's **level encodes the outcome**: `INFO` for `attached=true`,
  `INFO` (`expected=false`) for an unconfigured session, and `WARN` **only** for
  an *expected-but-degraded* memory bridge. "Any `WARN` under `simard::enrichment`
  is a real degrade" is therefore a safe operator rule (see
  [population](#which-turns-are-counted-population)).
- The raw underlying error string on a degrade is emitted at **`DEBUG`** only
  (`raw=…`); the `WARN` carries the bounded `reason` enum, not the free-text
  error, to avoid log injection/forging.
- The two previous bare `eprintln!` degrade paths in `launch_enrichment_bridges`
  are **removed** — this is now the only degrade output, and it is structured.
  No stray `println!`/`eprintln!` remains; everything is `[simard]`/tracing.

## Metric catalog: `simard.enrichment.*`

Registered in
[`telemetry::names`](https://github.com/rysweet/Simard/blob/main/src/telemetry/names.rs)
and emitted through the standard
[telemetry facade](./telemetry-metrics.md) (`counter_add` / `histogram_record`),
so each also flows to OTLP when `OTEL_EXPORTER_OTLP_ENDPOINT` is set.

| Metric name | Type | Attributes | Meaning |
|---|---|---|---|
| `simard.enrichment.decisions` | counter (`u64`) | `attached = "true" \| "false"` | One increment per instrumented decision. The `attached` split is the **attach-rate numerator/denominator**: `attach_rate = decisions{attached=true} / decisions{*}`. This is where the required per-decision `attached` bool lives, at a bounded cardinality of 2. |
| `simard.enrichment.degraded` | counter (`u64`) | `reason = "memory_ipc" \| "knowledge_launch"` | One increment per bridge-launch degrade, tagged with the concrete cause. |
| `simard.enrichment.preamble_bytes` | histogram (`f64`) | *(none)* | Rendered enrichment-block size injected per decision. Histogram (count+sum) so the dashboard can compute the **average bytes per decision** with zero attribute cardinality. |
| `simard.enrichment.facts_injected` | histogram (`f64`) | *(none)* | Facts rendered into the preamble per decision. count+sum → **avg facts/decision**. |
| `simard.enrichment.procedures_injected` | histogram (`f64`) | *(none)* | Procedures rendered into the preamble per decision. count+sum → **avg procedures/decision**. |

Design notes:

- **Magnitudes are histograms, not counters**, precisely so an average
  (`sum/count`) is computable without carrying the value as a high-cardinality
  attribute.
- **The `objective` is never a metric attribute.** Only fixed low-cardinality
  enums (`attached`, `reason`) are attached, matching the facade's cardinality
  contract; an unexpected value is folded into `other` by the facade.
- **Attach-rate is over the *expected* population.** The counters/histograms are
  recorded only for turns where enrichment was configured
  (`EnrichmentSource != Disabled`); benign `Disabled`-source turns are excluded
  (see [population](#which-turns-are-counted-population)), so
  `attach_rate = decisions{attached=true} / decisions{*}` cannot be dragged below
  100% by an adapter that legitimately runs without enrichment.
- **`preamble_bytes` is the *full* enrichment block** (memory facts + procedures
  **and** any domain knowledge), so it is not a pure memory-recall measure: a
  knowledge-only turn records `preamble_bytes > 0` with
  `facts_injected = procedures_injected = 0`.
- **No fact/procedure content** is ever emitted — counts and byte sizes only.

## Per-cycle rollup: `EnrichmentRollup` → `metrics_snapshot.json`

The dashboard reads the **live** store, not the OTLP pipeline. To make the
numbers dashboard-readable without an external collector, an in-process
`EnrichmentRollup` accumulates the per-turn observations and is **drained once
per OODA cycle** into `metrics_snapshot.json`, mirroring
[`flush_recall_precision_metric`](./recall-precision-hybrid-api.md#live-rail).

The rollup adds one additive section to the snapshot (the snapshot
`SCHEMA_VERSION` is unchanged; the field is `#[serde(default)]` so old readers
tolerate it):

```jsonc
{
  "enrichment": {
    "window_start": "2026-07-07T18:00:00Z",
    "window_end":   "2026-07-07T20:00:00Z",
    "decisions": 42,
    "attached": 40,
    "attach_rate": 0.9524,
    "degraded": { "memory_ipc": 2, "knowledge_launch": 0 },
    "avg_facts_injected": 6.3,
    "avg_procedures_injected": 2.8,
    "avg_preamble_bytes": 771.5,
    "last": {
      "attached": true,
      "facts_injected": 7,
      "procedures_injected": 3,
      "preamble_bytes": 812,
      "at": "2026-07-07T19:58:11Z"
    }
  }
}
```

- Drained per cycle, unconditionally; **skipped under `cfg!(test)`** for the
  file write (the aggregate is still drained so counters do not leak across
  tests), matching the recall-precision precedent.
- Best-effort: a snapshot write failure is logged, never propagated into the
  cycle.
- Written `0600` under a `0700` directory via the existing atomic-write
  convention. Reading the snapshot never mutates the recall corpus.

## Endpoint: `GET /api/enrichment`

Registered **inside** the dashboard's `require_auth` scope (fail-closed 401), the
handler live-reads the state root via `resolve_state_root()`, reads the
`enrichment` section of `metrics_snapshot.json`, and — for the trailing-window
figures — performs one **bounded** scan of `metrics/metrics.jsonl`. It is a
**total function**: it always returns HTTP `200` with a degrade-safe body, never
`4xx`/`5xx` for bad input.

### Query parameters

| Param | Type | Default | Clamp | Meaning |
|---|---|---|---|---|
| `window_hours` | uint | `24` | `1..=8760` | Trailing window for the attach-rate/averages. Out-of-range values are clamped, not rejected. |
| `limit` | uint | `500` | `1..=1000` | Max `metrics.jsonl` records to scan within the window (independent byte + line caps also apply). |

### Response schema (200)

```jsonc
{
  "available": true,               // false when the snapshot is missing
  "freshness": "live",             // "live" | "stale" | "missing"
  "snapshot_age_seconds": 41,      // null when missing
  "window_hours": 24,
  "decisions": 42,
  "attached": 40,
  "attach_rate": 0.9524,           // null when decisions == 0
  "degraded": { "memory_ipc": 2, "knowledge_launch": 0 },
  "avg_facts_injected": 6.3,       // null when decisions == 0
  "avg_procedures_injected": 2.8,  // null when decisions == 0
  "avg_preamble_bytes": 771.5,     // null when decisions == 0
  "last": {
    "attached": true,
    "facts_injected": 7,
    "procedures_injected": 3,
    "preamble_bytes": 812,
    "at": "2026-07-07T19:58:11Z"
  }
}
```

### Freshness semantics

| `freshness` | Condition | Panel renders |
|---|---|---|
| `live` | Snapshot present and newer than the cycle interval | Numbers + green freshness dot |
| `stale` | Snapshot present but older than the staleness threshold | Numbers + amber "stale" note |
| `missing` | No snapshot yet (fresh brain, or daemon not running) | `available:false`, `Not tracked yet` |

### Failure contract

- Missing snapshot → `available:false`, `freshness:"missing"`, all magnitudes
  `null`. No panic.
- Corrupt `metrics.jsonl` lines are **skipped**, not fatal.
- Any unexpected internal error returns `200` with a generic `{"error": "..."}`
  field; specifics go to the logs, never to the HTTP client.

## Dashboard Memory-tab panel

The Memory tab renders a **"Recall reaching decisions"** panel bound to
`GET /api/enrichment`:

- **Attach-rate** as a percentage (`95% of decisions received recalled memory`),
  colour-tiered (green ≥ threshold, amber/red below).
- **Averages per decision**: facts, procedures, and preamble bytes.
- **Degrade breakdown**: `memory_ipc` / `knowledge_launch` counts, shown loud
  (red) whenever non-zero — the operator's cue that a bridge is down.
- **Freshness indicator**: `live` / `stale` / `missing`, so a stale or missing
  snapshot is never mistaken for `0%`.
- The most recent decision's raw `{attached, facts, procedures, preamble_bytes}`
  for spot-checking.

The panel is prefetched and refreshed like every other Memory-tab component and
degrades to `Not tracked yet` when `available:false`.

## Ablation eval: `simard gym enrichment-ablation`

The hard proof. Reusing the [gym harness](../howto/run-the-coin-gym-harness.md)
(`src/gym`, base type `local-harness`), the eval seeds a hermetic in-memory
cognitive store with representative facts/procedures and runs one representative
decision **twice**:

- **recall-on** — memory bridge attached.
- **recall-off** — recall suppressed (`memory_client = None`).

It asserts a **measurable difference** and prints a reproducible verdict:

```console
$ simard gym enrichment-ablation
cognition/enrichment_ablation: recall_on_bytes=812 recall_off_bytes=0 delta_bytes=812 facts=7 procedures=3 preambles_differ=true verdict=influences
```

| Field | Meaning |
|---|---|
| `recall_on_bytes` / `recall_off_bytes` | Rendered enrichment-block size with recall on vs suppressed |
| `delta_bytes` | `recall_on_bytes - recall_off_bytes`; the reproducible magnitude of the difference |
| `facts` / `procedures` | Counts injected on the recall-on run |
| `preambles_differ` | Whether the two prompt preambles are non-identical |
| `verdict` | `influences` when `delta_bytes > 0` **and** `preambles_differ`; otherwise `no-influence` |

If a full end-to-end decision ablation is too heavy for CI, the eval satisfies at
minimum the **fallback bar**: it asserts that `enrich_turn_input` produces a
**non-empty, correctly-rendered** preamble (with `## Relevant Memory Facts` and
`## Known Procedures`) when the bridge attaches and the seeded store has
facts/procedures — and an **empty** enrichment block when recall is suppressed.
Both the full and fallback forms yield the same reproducible yes/no.

## Hybrid self-measurement feed (#2644)

On each ablation run the `delta_bytes` result is recorded via
[`self_metrics::record_metric`](./recall-precision-hybrid-api.md) so it feeds the
[hybrid cognition self-measurement](../concepts/hybrid-cognition-measurement.md):

```rust
self_metrics::record_metric(
    "enrichment_ablation_delta",
    delta_bytes as f64,
    r#"{"site":"enrichment_ablation","verdict":"influences"}"#,
)?;
```

This is a **feed only** — enrichment observability neither computes nor gates the
hybrid verdict; it contributes the "recall reaches — and moves — decisions" live
data point that `recall_precision_at_k` cannot, since precision says nothing
about whether recall was plumbed through at all.

## Configuration

| Variable | Effect | Default |
|---|---|---|
| `SIMARD_STATE_ROOT` | Root under which `metrics_snapshot.json` and `metrics/metrics.jsonl` (and thus `/api/enrichment`) are read/written | `$HOME/.simard` |
| `RUST_LOG` / tracing filter | Set `simard::enrichment=info` (or lower) to see the per-turn lines; `=debug` also surfaces the raw degrade `error` string | inherits global |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | When set, the `simard.enrichment.*` metrics also export via OTLP (identical gating to all telemetry) | unset (local-only) |

No opt-out flag exists: the instrumentation is always-on and additive. Nothing
about recall, ranking, rendering, or dispatch is configurable through this
feature.

## Security properties

- **AUTHZ.** `GET /api/enrichment` is registered **inside** the `require_auth`
  scope; an unauthenticated request receives `401`. A dedicated test asserts this
  (a route registered outside the scope would silently bypass auth).
- **INPUT (total function).** `window_hours` (`1..=8760`) and `limit`
  (`1..=1000`) are **clamped**, never rejected. The `metrics.jsonl` scan is
  bounded by independent **byte and line caps**; no unbounded `read_to_string`.
- **PATH.** The state root comes from the environment only — no request-controlled
  path — so there is no traversal surface.
- **DATA — no content leak.** Only counts and byte sizes are logged/metered;
  fact/procedure **text is never emitted**.
- **DATA — no log forging.** The `objective` is truncated (≤120 B) and
  control-character stripped and only ever a structured field, never
  string-interpolated. Degrade `WARN`s carry the bounded `reason` enum; the raw
  error goes to `DEBUG` only.
- **DATA — feed-file permissions.** Any new feed/snapshot bytes are written
  `0600` under a `0700` directory via the existing atomic-write convention.
- **ERROR — no info leak.** The endpoint's top-level handler returns a generic
  `error` field only; specifics stay in the logs.

## Tests

Real, hermetic, no network:

| Test | Asserts |
|---|---|
| `enrichment_observability::observe` emission | Given facts/procedures rendered, emits `attached=true` and **non-zero** `facts_injected`/`procedures_injected`/`preamble_bytes`; the objective field is truncated + control-stripped |
| Attach path (`tests/base_type_enrichment.rs`) | A seeded store + attached bridge yields `attached=true`, a non-empty preamble with both sections, and non-zero injected counts |
| Degrade path (thread-scoped `WARN` capture) | An injected failing memory connector produces `attached=false` **and** a `WARN` (not a silent `None`) with `reason=memory_ipc`, and increments `simard.enrichment.degraded{reason=memory_ipc}` |
| Ablation (`tests/enrichment_ablation.rs`) | recall-on vs recall-off shows `delta_bytes > 0`, `preambles_differ=true`, `verdict=influences`; the fallback form asserts a non-empty correctly-rendered preamble on attach and an empty block on suppression |
| Endpoint auth (`tests/dashboard_enrichment_endpoint.rs`) | Unauthenticated `GET /api/enrichment` → `401`; authenticated → `200`; missing snapshot → `available:false`/`freshness:"missing"` with no panic; out-of-range params are clamped, still `200` |
| Population / `expected` (`base_type_turn` unit) | A `Disabled`-source (or no-bridge) turn emits `INFO expected=false` and records **no** `simard.enrichment.*`; a `Native` source whose memory bridge fully degrades still emits `expected=true` (a `WARN`) and **is** counted — proving a degrade is never misread as "unconfigured" and cannot silently drop out of the attach-rate |

## Guarantees and non-guarantees

**Guarantees**

- Every in-process OODA decision emits an attach/degrade signal and injected-payload counts.
- A degrade is always a `WARN` + a `degraded{reason}` counter — never a silent `None`.
- The dashboard attach-rate/averages read the **live** store; a missing/stale snapshot is labelled, never rendered as a false `0%`.
- The ablation eval is reproducible and hermetic.

**Non-guarantees**

- **Scope.** Instrumentation covers the **daemon's own in-process OODA
  decisions**. Enrichment performed inside an **engineer subprocess** reaches
  OTLP through that subprocess's telemetry but not this in-process registry or
  the daemon's `metrics_snapshot.json`. This boundary is intentional and
  documented; a future issue may aggregate subprocess enrichment.
- **Not a quality metric.** `attached=true` with `facts=0` means the bridge is up
  but the store had nothing for this objective — a true signal, not a failure.
  Recall *quality* is `recall_precision_at_k`'s job, not this feature's.

## What is unchanged

- Recall, ranking (`recall_facts_ranked` / `recall_procedures_for_objective`),
  the `MAX_MEMORY_FACTS` (10) / `MAX_PROCEDURES` (5) caps, `render_enrichment_block`,
  and turn dispatch are byte-for-byte unchanged.
- `EnrichmentSource` / `EnrichmentClients` keep their honest-degradation
  contract and unchanged recall behaviour; the only additions are that the
  degrade is now **loud** and that each resolved bundle carries the `expected`
  provenance bit (`Native` ⇒ `true`, `Disabled` ⇒ `false`) described under
  [threading `expected`](#threading-expected).
- The telemetry `SCHEMA_VERSION` is unchanged (the `enrichment` snapshot section
  is additive and `#[serde(default)]`).

## See also

- [Concept: enrichment observability](../concepts/enrichment-observability.md) — the why.
- [How to verify recall is reaching decisions](../howto/verify-recall-reaches-decisions.md) — the operator playbook.
- [Telemetry metrics reference](./telemetry-metrics.md) — the facade, registry, and `metrics_snapshot.json` flush this builds on.
- [Recall-precision hybrid API](./recall-precision-hybrid-api.md) — the per-cycle drain precedent and the `metrics.jsonl` plumbing.
- [Base-type adapters reference](./base-type-adapters.md) — the shared `enrich_turn_input` seam.
- [Dashboard Memory tab](./dashboard-memory-tab.md) — the tab this panel joins.
