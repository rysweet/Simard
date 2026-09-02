---
title: Telemetry metrics reference
description: The unified Simard telemetry facade and OpenTelemetry metrics pipeline — the counter_add/gauge_set/histogram_record facade, the simard.<area>.<name> metric catalog with attributes and types, the in-process registry and metrics_snapshot.json flush, SdkMeterProvider init and endpoint-gated OTLP export, cardinality bounding, and the configuration knobs.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/unified-telemetry-and-status.md
  - ./status-snapshot-api.md
  - ../howto/simard-status.md
  - ./runtime-contracts.md
  - ./daily-budget-display-guard.md
  - ../architecture/distillation-semantic-handoff.md
  - ./distill-write-boundary-gate.md
  - ./disk-reclaim-telemetry.md
  - ./cognitive-thread-observability.md
---

# Telemetry metrics reference

Simard exposes one typed telemetry facade at `src/telemetry/` and one
OpenTelemetry `SdkMeterProvider` installed in `init_tracing()` alongside the
tracer. This page is the authoritative catalog of the metric set, the facade
API, the in-process registry, the on-disk snapshot, OTLP export gating, and the
configuration.

> **Modules:** `src/telemetry/mod.rs` (facade), `src/telemetry/names.rs`
> (constants), `src/telemetry/registry.rs` (in-process registry),
> `src/telemetry/otel.rs` (`MeterProvider`), `src/telemetry/snapshot.rs`
> (registry → JSON flush). Wiring: `src/main.rs::init_tracing()`.

## Design in one paragraph

The facade **dual-writes** every metric operation: into a lightweight
in-process **atomic registry** (the source of truth read by
[`simard status`](./status-snapshot-api.md)) and into **OpenTelemetry
instruments** on an `SdkMeterProvider`. The registry is **always** installed so
current values are readable in-process with no external collector. OTLP export
is installed **only** when `OTEL_EXPORTER_OTLP_ENDPOINT` is set — identical
gating to traces — so the default deployment stays fully local.

## Facade API

```rust
use simard::telemetry;
use simard::telemetry::names; // metric-name & attribute-key constants

// Counter: monotonically add.
telemetry::counter_add(names::DISTILL_RUNS, 1, &[("result", "ok")]);

// Gauge: set current value.
telemetry::gauge_set(names::ENGINEER_ACTIVE, live_count as i64, &[]);

// Histogram: record an observation (seconds, bytes, etc.).
telemetry::histogram_record(names::DAEMON_CYCLE_DURATION_SECONDS, elapsed.as_secs_f64(), &[]);
```

| Function | Instrument | Semantics |
|---|---|---|
| `counter_add(name, value: u64, attrs)` | monotonic counter (`u64`) | adds `value`; registry keeps a running total per attribute set |
| `gauge_set(name, value: i64, attrs)` | synchronous gauge (`i64`) | replaces the current value for the attribute set |
| `histogram_record(name, value: f64, attrs)` | explicit-bucket histogram (`f64`) | records one observation; registry keeps count/sum/bucket tallies |

`attrs` is a slice of `(&str, &str)` key/value pairs. **All attribute values are
fixed low-cardinality enums** (see each metric below). The facade **normalizes**
every attribute value — caps length, strips control characters — and **bounds
series cardinality**: an unexpected value is folded into an `other` bucket and
increments an internal overflow counter, protecting both the registry `HashMap`
and any OTLP export.

The facade is safe to call from any thread and from hot paths; writes are
atomic and lock-light.

> **Gauges are last-write-wins.** `gauge_set` writes the current value into the
> in-process registry and into a **synchronous** OTel gauge (`i64_gauge`) in the
> same call. A gauge therefore reflects the **last value written**; counters and
> histograms accumulate per write.

## Emission status

Every metric name below is a stable constant in `src/telemetry/names.rs` — the
single-sourced catalog other tooling keys off. The migration wires the
operational signals that were previously ad-hoc text; a few catalog entries are
**reserved** (constant defined, emission is a follow-up) because the
[`simard status`](./status-snapshot-api.md) report currently sources the same
fact from a more authoritative place:

| Metric | Emitted today | Notes |
|---|---|---|
| `simard.distill.*` | ✅ `distillation.rs` | ok + facts/procedures/episodes_marked (the `parse_fail` result was **removed** in #2679 — there is no parse) |
| `simard.brain.decision` / `.escalations` | ✅ `judgment_record::push` | every decision, phase + parse outcome |
| `simard.brain.ladder_exhausted` | ✅ `recipe_brain::run_brain_ladder` | decide/orient ladder fully exhausted |
| `simard.engineer.spawned` / `.exited` / `.active` | ✅ `agent_spawn.rs` | brackets the in-process session |
| `simard.daemon.cycle` / `.cycle_duration_seconds` | ✅ `daemon/mod.rs` | per OODA cycle |
| `simard.daemon.restart` | ⏳ reserved | status reads the authoritative `NRestarts` from `systemctl show`; the in-process counter would reset on each restart |
| `simard.memory.nodes` / `.edges` | ✅ `daemon/mod.rs` | daemon-sampled per cycle |
| `simard.llm.tokens` | ✅ `cost_tracking::record_cost` | in/out throughput |
| `simard.llm.cost_usd` / `.credits` | ⏳ reserved | integral counters lose fractional cents; status reads $ + AI-credits from the cost ledger (authoritative) |
| `simard.goal.active` | ✅ `daemon/mod.rs` | per cycle |
| `simard.goal.completed` / `.progress` | ⏳ reserved | goal transitions are a follow-up; the board itself is read directly |

## Metric catalog

All metric names are dotted `simard.<area>.<name>`. Attribute values are the
enumerated sets shown; anything outside the set is coerced to `other`.

### Distillation — `simard.distill.*`

| Metric | Type | Attributes | Meaning |
|---|---|---|---|
| `simard.distill.runs` | counter | `result` = `ok` | one distillation run whose agentic commit completed. The former `parse_fail` value was **removed** in [#2679](https://github.com/rysweet/Simard/issues/2679): the distiller now writes facts directly into memory via [`simard memory remember`](./simard-memory-remember-cli.md), so there is no scraped document to parse and no parse to fail. |
| `simard.distill.facts` | counter | — | facts **accepted by the write-boundary gate** for a run, sourced from the per-`pass_id` write ledger (not a parsed array length). |
| `simard.distill.procedures` | counter | — | procedures written by a run |
| `simard.distill.episodes_marked` | counter | — | episodes marked processed by a run |

> **Removed in #2679:** the `simard.distill.runs{result="parse_fail"}` series and
> the derived `distill_parse_success_rate`. Dashboards/alerts keyed on either
> will read empty; track `result="ok"` and `simard.distill.facts` instead. See
> [Distillation semantic handoff](../architecture/distillation-semantic-handoff.md)
> and [Distill write-boundary gate](./distill-write-boundary-gate.md).

Migrated from the human line
`[simard] distill: N episodes -> F facts, P procedures, M marked`, which is
still emitted verbatim.

**Consumed by the Status snapshot.** `simard.distill.runs{result="ok"}` also
feeds the unified Status snapshot's MEMORY / BRAIN **cognitive** line
(`GET /api/status/snapshot` → `data.memory.data.cognitive_processes.distillation`,
Overview "System Status", `simard status`, TUI Status tab). It renders `idle`
for a flushed-but-zero counter, `N runs` once runs have completed, and stays
honestly `absent` until the daemon first flushes the counter — the same
counter the Telemetry section already derives `distill_fail_pct` from, so the
two never contradict each other. (`consolidation` and `introspection` on that
line have no published counter yet and remain `absent`.)

### Brain — `simard.brain.*`

| Metric | Type | Attributes | Meaning |
|---|---|---|---|
| `simard.brain.decision` | counter | `phase` = `decide` \| `orient` \| `act` \| `merge_judge`; `result` = `parsed` \| `default_malformed` \| `error` | one brain decision, tagged by phase and parse outcome (derived from the judgment record's `fallback` / `parse_failure`) |
| `simard.brain.ladder_exhausted` | counter | — | the `decide` ladder was exhausted with no keyword match |
| `simard.brain.escalations` | counter | — | a decision escalated (degraded/quarantine/SIGTERM path) |

Migrated from the ad-hoc `default_malformed` / `ladder_exhausted` /
escalation log lines.

### Engineer — `simard.engineer.*`

| Metric | Type | Attributes | Meaning |
|---|---|---|---|
| `simard.engineer.spawned` | counter | — | an engineer subprocess was spawned |
| `simard.engineer.exited` | counter | `outcome` = `success` \| `failure` \| `killed` \| `timeout` | an engineer subprocess exited |
| `simard.engineer.active` | gauge | — | live engineer subprocess count |

### Daemon — `simard.daemon.*`

| Metric | Type | Attributes | Meaning |
|---|---|---|---|
| `simard.daemon.restart` | counter | — | incremented on the daemon's **self-restart / recovery-restart** path (the degraded → SIGTERM → relaunch escalation) — restarts the daemon *initiates*, not those the supervisor performs. The authoritative cross-restart count for the **systemd unit** is read separately from `systemctl show simard.service` (`NRestarts`) and shown in the status `DAEMON / UPTIME` section; the two are deliberately distinct facts. |
| `simard.daemon.cycle` | counter | — | one OODA cycle completed |
| `simard.daemon.cycle_duration_seconds` | histogram | — | wall-clock seconds per OODA cycle |

**Histogram buckets** for `simard.daemon.cycle_duration_seconds` (explicit
boundaries, seconds): `[0.5, 1, 2, 5, 10, 30, 60, 120, 300]`.

### Memory graph — `simard.memory.*`

| Metric | Type | Attributes | Meaning |
|---|---|---|---|
| `simard.memory.nodes` | gauge | `type` = `episodic` \| `semantic` \| `prospective` \| `working` \| `procedural` \| `sensory` | node count per memory type |
| `simard.memory.edges` | gauge | `type` = `DERIVES_FROM` \| `SIMILAR_TO` \| `SUPERSEDES` | edge count per relationship type |

These are **daemon-sampled** gauges: once per OODA cycle the daemon reads the
memory graph statistics (the same `get_statistics()` / `graph_stats()` the
dashboard uses) and republishes the counts through the facade, so they ride the
registry/snapshot/OTLP path like every other metric. The
[`simard status`](./status-snapshot-api.md) `MEMORY / BRAIN` section reads its
node/edge counts **from these snapshot gauges** — process-agnostic and requiring
no LadybugDB open from the CLI. When the daemon has not yet flushed memory
gauges the section renders `absent`, never a fabricated zero.

> **Grounding coverage.** The raw `simard.memory.edges{type=DERIVES_FROM}` gauge
> is complemented by a durable **`fact_provenance_coverage`** self-metric — the
> grounded *fraction* (`facts_with_provenance / facts_total`) emitted per cycle
> to the `metrics.jsonl` series so a graph-memory grounding regression is
> comparable and regressable, not just a raw count. See
> [Cognitive-memory provenance § Observability](./cognitive-memory-provenance.md#observability-grounding-coverage-self-metric).

> **Goal-board snapshot hygiene.** A sibling durable
> **`goal_board_snapshot_dedup_ratio`**
> self-metric emits, from the same per-cycle `graph_stats()` snapshot, the
> average *liveness* of goal-board snapshot revisions
> (`distinct_snapshot_caller_keys / snapshot_facts_total` ∈ `[0, 1]`, higher is
> healthier). It falls when superseded snapshot revisions accumulate faster than
> controlled forgetting (`prune_superseded`) reclaims them, turning a pruning
> regression into a durable time series for operator and future automated
> analysis rather than only a raw count. See
> [Cognitive-memory provenance § Snapshot dedup hygiene](./cognitive-memory-provenance.md#observability-snapshot-dedup-hygiene-self-metric).

### LLM usage — `simard.llm.*`

Mirrored from `cost_tracking` (the ledger format is unchanged; these are
additive emissions).

| Metric | Type | Attributes | Meaning |
|---|---|---|---|
| `simard.llm.tokens` | counter | `dir` = `in` \| `out`; `cached` = `true` \| `false` | token throughput by direction and cache status |
| `simard.llm.cost_usd` | counter | — | dollar cost from the ledger |
| `simard.llm.credits` | counter | — | Copilot AI-credits consumed |

### Goals — `simard.goal.*`

| Metric | Type | Attributes | Meaning |
|---|---|---|---|
| `simard.goal.active` | gauge | — | active goals on the board |
| `simard.goal.completed` | counter | — | goals marked completed |
| `simard.goal.progress` | gauge | — | aggregate progress signal (0–100) |

Like the memory gauges, `simard.goal.active` is **daemon-sampled** from the goal
board each OODA cycle. `simard.goal.completed` / `.progress` are reserved (see
**Emission status**). The status `GOAL BOARD` list is rendered from the goal
board itself in the daemon-hosted surfaces (dashboard / TUI goal tabs).

### Disk reclaim — `simard.disk.reclaim.*`

Emitted once per agentic disk-reclamation run (the daemon self-heal path and
`simard disk-reclaim`). Full details and dashboard suggestions live in the
dedicated [disk reclaim telemetry reference](./disk-reclaim-telemetry.md).

| Metric | Type | Attributes | Meaning |
|---|---|---|---|
| `simard.disk.reclaim.bytes_freed` | counter | — | bytes actually reclaimed this run (0 on dry-run / no-op) |
| `simard.disk.reclaim.paths_removed` | counter | `kind` = `tracked_worktree` \| `orphan_dir` \| `stale_build_cache` | paths removed, by reclamation primitive |
| `simard.disk.reclaim.candidates_skipped` | counter | `reason` = `protected_path` \| `live_process` \| `uncommitted_or_unpushed` \| `active_worktree` \| `outside_allow_root` \| `unknown_pr_state` \| `other` | candidates a hard rail refused (the human-review list) — every increment is a path that was **not** deleted |
| `simard.disk.reclaim.used_pct_before` | gauge | — | home-partition `%-used` at run start (0–100) |
| `simard.disk.reclaim.used_pct_after` | gauge | — | home-partition `%-used` after the run (0–100) |

The agent's free-text candidate `reason` is **never** used as an attribute; only
the enum `RejectReason` (`reason=`) is. See
[Agentic disk reclamation](../concepts/agentic-disk-reclamation.md).

### Cognitive threads — `simard.thread.*`

Emitted per cognitive-thread run through the thread telemetry seam
(`src/cognitive_threads/telemetry.rs`). Thread identity is embedded in the
**metric name** (`simard.thread.<id>.<suffix>`), never as an attribute, so these
series carry no attributes. Full details, the Overseer oversight rail, and the
durable error path live in the dedicated
[cognitive-thread observability reference](./cognitive-thread-observability.md).

| Metric | Type | Meaning |
|---|---|---|
| `simard.thread.<id>.runs` | counter | one run the thread actually performed |
| `simard.thread.<id>.successes` | counter | runs that succeeded |
| `simard.thread.<id>.failures` | counter | runs that failed/panicked (mirrored to a durable `FailureDiagnosis` the Overseer drains) |
| `simard.thread.<id>.duration_seconds` | histogram | per-run wall-clock seconds (shared cycle buckets) |
| `simard.thread.<id>.last_run_epoch` | gauge | epoch of the last completed run (last-run age is derived) |
| `simard.thread.<id>.next_run_epoch` | gauge | epoch of the next scheduled run (stall signal) |
| `simard.thread.<id>.active` | gauge | `1` while mid-tick, `0` otherwise |

`<id>` is the stable `snake_case` [`CognitiveThread::id`](./cognitive-threads-catalog.md)
(e.g. `ooda`, `metacognition`, `reflection`); `runs = successes + failures`, so
success/failure rate is derivable at zero attribute cardinality.

## In-process registry and the on-disk snapshot

`src/telemetry/registry.rs` holds the current value of every series keyed by
`(metric_name, sorted_attributes)`. It is bounded:

- fixed-enum attribute values only; overflow → `other` bucket + overflow
  counter,
- per-value length cap and control-character stripping,
- a global series cap so a bug cannot grow it without bound.

`src/telemetry/snapshot.rs` serializes the registry to a `MetricsSnapshot` JSON
document and, **in the daemon only**, flushes it to
`~/.simard/telemetry/metrics_snapshot.json` once per OODA cycle (via
`telemetry::flush_snapshot`, called at the end of each cycle in
`src/operator_commands_ooda/daemon/mod.rs`). The write is **atomic and
private**:

1. write a temp file created `0600`,
2. `fsync`,
3. `rename` over the target,

with the parent directory `~/.simard/telemetry/` created `0700`. The file is
never briefly world-readable, and no other process ever writes it (single
writer, no contention). CLI and TUI **read** this file; they never write it.

The snapshot document carries a `schema_version`. Readers are size-capped and
schema-checked and **degrade to `stale`/`absent` rather than panicking** on a
missing, truncated, or corrupt file.

## OpenTelemetry init and OTLP export gating

`init_tracing()` builds an `SdkMeterProvider` **symmetric with the tracer**:

- **In-process registry** — always the source of truth, so current metric
  values back the status snapshot and the dashboard's live view with no
  collector. (The `SdkMeterProvider` is always installed as the global meter
  provider so the facade's instruments are real, not the global no-op.)
- **OTLP `PeriodicReader`** — installed **only when
  `OTEL_EXPORTER_OTLP_ENDPOINT` is set**, exporting over OTLP with
  `service.name = simard` (matching the tracer's resource).
- **Shutdown** — `telemetry::shutdown_metrics()` runs on process exit to flush
  the final export.

```text
OTEL_EXPORTER_OTLP_ENDPOINT unset  ->  registry + in-process reader only (default)
OTEL_EXPORTER_OTLP_ENDPOINT set    ->  registry + OTLP PeriodicReader (export ON)
```

### Exported-attribute safety

OTLP metric attributes are **low-cardinality, non-PII fixed enums only**. Goal
text, prompt/episode content, filesystem paths, session UUIDs, PR bodies, and
raw error strings are **never** used as labels. Export is gated identically to
traces and is **off by default**.

## Configuration

| Variable | Effect | Default |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Enables OTLP export for **both** traces and metrics. Unset = local/in-process only. | unset (export off) |
| `SIMARD_LOG_JSON` | `1` switches the fmt log layer to JSON; unrelated to metrics but part of the same `init_tracing()`. | text |
| `SIMARD_STATE_ROOT` | Overrides the state root (`$HOME/.simard`), which is where `telemetry/metrics_snapshot.json`, the cost ledger, and `self_metrics` live. | `$HOME/.simard` |
| `SIMARD_DAILY_BUDGET_USD` | The daily budget guard the LLM-usage section compares spend against. Single-sourced through `overseer::config::daily_budget_usd()`, so the displayed ceiling always matches the Overseer `BudgetGate` even when unset; see the [daily-budget display guard](./daily-budget-display-guard.md). | `500` (always guarded) |
| `SIMARD_SKIP_GYM` | Respected by the gym section of the status snapshot. | unset |

## Relationship to the four legacy stores

The unified facade **reconciles** the pre-existing metric stores rather than
replacing them:

- `self_metrics/` — remains the durable JSONL mirror; the daemon additionally
  emits `simard.daemon.cycle*` through the facade each cycle.
- `cost_tracking.rs` — `record_cost` additionally emits `simard.llm.tokens`;
  ledger format unchanged (the status LLM section still reads $ + credits from
  the ledger).
- `cognitive_memory/metrics.rs` — the in-process silent-drop counters are
  unchanged (no regression); node/edge counts are sampled separately via
  `get_statistics()` / `graph_stats()`.
- `gym/executor_metrics.rs`, `operator_commands_dashboard/metrics.rs` — surface
  tallies preserved; no writer-path rewrites.

All migrations are additive: existing writers and readers keep working, so the
running daemon does not regress.

## See also

- [Unified telemetry and one `simard status`](../concepts/unified-telemetry-and-status.md)
  — the design rationale.
- [StatusSnapshot API reference](./status-snapshot-api.md) — how these metrics
  are read back into the status report.
- [How to read `simard status`](../howto/simard-status.md) — operator usage.
