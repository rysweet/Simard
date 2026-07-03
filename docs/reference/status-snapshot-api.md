---
title: StatusSnapshot API reference
description: The one typed StatusSnapshot the CLI, dashboard, and TUI all render — its section model, per-section Availability/Freshness envelopes, the process-agnostic status::assemble() provider and its durable sources, the --json schema, and the auth-gated /api/status/snapshot dashboard endpoint.
last_updated: 2026-07-03
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/unified-telemetry-and-status.md
  - ./telemetry-metrics.md
  - ../howto/simard-status.md
  - ./simard-cli.md
  - ./simard-tui.md
  - ../dashboard.md
  - ./operator-read-state-root-contract.md
---

# StatusSnapshot API reference

`StatusSnapshot` is the single typed value that answers "how is Simard doing
right now?". One provider assembles it; three surfaces render it — the
[`simard status`](../howto/simard-status.md) CLI, the dashboard **Status** tab
(`GET /api/status/snapshot`), and the TUI **Status** tab. They show the same
numbers because they consume the same serialized snapshot.

> **Modules:** `src/status/mod.rs` (types), `src/status/provider.rs`
> (`assemble()`), `src/status/sources/*` (readers), `src/status/json.rs`
> (`--json` / HTTP body), `src/status/render.rs` (rich terminal). Surfaces:
> `src/operator_cli/status.rs`, `src/operator_commands_dashboard/status.rs`,
> `src/bin/simard_tui/tabs/status.rs`.

## Provider

```rust
use simard::status;

/// Assemble the full snapshot from durable, process-agnostic sources.
/// Never panics; degraded sources become unavailable/absent sections.
let snapshot: status::StatusSnapshot = status::assemble(&opts);

// Render to the canonical terminal layout, or serialize.
let text = status::render::to_terminal(&snapshot);
let json = status::json::to_string_pretty(&snapshot)?;
```

`assemble()` is **process-agnostic**: it reads durable on-disk and system
sources, so it returns the same result from the daemon, the CLI, or the TUI.
Each section is assembled in **isolation** — one failing source degrades one
section, never the whole report, and never panics.

### Durable sources

All sections are assembled in `src/status/provider.rs`. Each reads a durable,
process-agnostic source and degrades to `unavailable`/`absent` (with a note)
rather than fabricating data:

| Section | Source (`src/status/provider.rs`) | Not this |
|---|---|---|
| Daemon / uptime | `systemctl show simard.service` (`LoadState`, `ActiveState`, `MainPID`, `NRestarts`, `ExecMainStartTimestamp`) | ~~`journalctl \| grep`~~ |
| Resource snapshot | `/proc/loadavg`, `/proc/meminfo`, `/proc/<daemon-pid>/status` (RSS), `statvfs` (disk), `pgrep` (live engineers) | — |
| LLM usage | `cost_tracking` ledger (`costs/ledger.jsonl`) + `$SIMARD_DAILY_BUDGET_USD` | — |
| Memory / brain | `metrics_snapshot.json` — the daemon-sampled `simard.memory.nodes` / `.edges` gauges | ~~LadybugDB open from the CLI~~ |
| Gym | `$SIMARD_SKIP_GYM` | — |
| Goal board | *deferred* — rendered `unavailable`; surfaced live in the daemon-hosted dashboard / TUI goal tabs | — |
| Active workstreams | *deferred* — rendered `unavailable` (engineer registry) | — |
| Completed work | *deferred* — rendered `unavailable`; `gh` is not queried on the process-agnostic path | — |
| Self-improvement | *deferred* — rendered `unavailable` | — |
| Telemetry / anomalies | derived from `metrics_snapshot.json` (distill fail %, ladder-exhausted, cardinality overflow) + `systemctl` `NRestarts` + budget | ~~`journalctl \| grep`~~ |

Sections marked *deferred* still render their header and an honest
`unavailable (<reason>)` line — the frame is always complete, and no section
ever invents a `0`.

The counters, gauges, and histograms come from
`~/.simard/telemetry/metrics_snapshot.json` (see
[telemetry reference](./telemetry-metrics.md)), so the report reflects the same
values the daemon exports — without reading daemon RAM.

## Types

Every section is wrapped in a `SectionEnvelope` so freshness and availability
travel with the data. All structs use `#[serde(default)]` throughout so a
partial or older snapshot still deserializes.

```rust
pub struct StatusSnapshot {
    pub schema_version: u32,
    pub generated_at: String,           // RFC3339
    pub daemon: SectionEnvelope<Daemon>,
    pub resources: SectionEnvelope<Resources>,
    pub llm: SectionEnvelope<LlmUsage>,
    pub memory: SectionEnvelope<MemoryBrain>,
    pub gym: SectionEnvelope<Gym>,
    pub goals: SectionEnvelope<GoalBoard>,
    pub workstreams: SectionEnvelope<Workstreams>,
    pub completed: SectionEnvelope<CompletedWork>,
    pub self_improvement: SectionEnvelope<SelfImprovement>,
    pub telemetry: SectionEnvelope<TelemetrySignals>,
}

pub struct SectionEnvelope<T> {
    pub availability: Availability,     // ok | unavailable | error
    pub freshness: Freshness,           // live | stale | absent
    pub as_of: Option<String>,          // RFC3339 of the underlying source
    pub note: Option<String>,           // e.g. "gh: not authenticated"
    pub data: Option<T>,                // None when unavailable/absent
}

pub enum Availability { Ok, Unavailable, Error }
pub enum Freshness    { Live, Stale, Absent }
```

### Freshness semantics

| Value | Meaning | Renders as |
|---|---|---|
| `live` | source read successfully within its freshness window | the value |
| `stale` | last-known value returned; source older than its window (e.g. snapshot not flushed recently, DB locked) | value + `(stale)` marker |
| `absent` | source missing entirely (no snapshot yet, `gh` unauthenticated) | `absent` / `unavailable (<reason>)` — **never** `0` |

**No silent zeros.** A missing count is `absent`, not `0`. This distinction is
load-bearing for operators: "zero restarts" and "restart count unknown" are
different facts.

### Section payloads (summary)

| Struct | Key fields |
|---|---|
| `Daemon` | `state`, `version`, `main_pid`, `deployed_commit`, `instance_uptime`, `n_restarts`, `running_since` |
| `Resources` | `cpu_pct`, `rss`, `cgroup_mem_peak`, `load_1/5/15`, `sys_mem_used/total/avail`, `disk_home`, `disk_tmp`, `live_engineers` |
| `LlmUsage` | `copilot_turn` (tokens in/cached/out + AI-credits), `ledger_today/7d/all_time` (cost + in/out tokens), `daily_budget_usd`, `reconciliation` (two-books delta + `under/over-count` flag) |
| `MemoryBrain` | `store_path` (the `amplihack-memory-lib` store at `<state_root>/cognitive`), `store_size`, `backend`, `nodes_total` + per-type counts, `edges` per type, `cognitive_processes` (distillation/consolidation/introspection health), `brains_llm_backed`, `brain_fallbacks`, `decide_ladder_exhausted` |
| `Gym` | `skip_gym`, `configured_scenarios`, `self_eval_state` (idle/active) |
| `GoalBoard` | `active[]` (priority, status, short id) |
| `Workstreams` | `operator_recipes[]`, `engineer_workstreams[]` (one-line status) |
| `CompletedWork` | merged PRs (last ~24h) grouped `repo -> #pr -> summary -> status` |
| `SelfImprovement` | self-fix / audit workstreams + PRs (merged/running/pending) |
| `TelemetrySignals` | `window`, `distill_fail_pct`, `restart_churn`, `gym_skipped`, `budget_flag`, `parse_fix_holding`, `anomalies[]` (panics/segv/corruption/fallback/budget) |

### Cost reconciliation (two books)

`LlmUsage.reconciliation` surfaces **both** accounting systems — the dollar
ledger and Copilot AI-credits — computes the delta, and sets a
`under/over-count` flag when they diverge beyond tolerance. It never collapses
them into one number. See the [concept doc](../concepts/unified-telemetry-and-status.md#two-books-of-cost-told-honestly).

## `--json` schema

`simard status --json` and the dashboard endpoint emit the **same** serialized
`StatusSnapshot`. Shape (aconnected; every section follows the envelope pattern):

```json
{
  "schema_version": 1,
  "generated_at": "2026-07-03T03:55:05Z",
  "daemon": {
    "availability": "ok",
    "freshness": "live",
    "as_of": "2026-07-03T03:55:04Z",
    "note": null,
    "data": {
      "state": "active (running)",
      "version": "0.1.0",
      "main_pid": 48291,
      "deployed_commit": "e5764c6d",
      "instance_uptime": "2h14m33s",
      "n_restarts": 0,
      "running_since": "2026-07-03T01:40:31Z"
    }
  },
  "llm": {
    "availability": "ok",
    "freshness": "live",
    "data": {
      "ledger_today": { "cost_usd": 1.87, "tokens_in": 412000, "tokens_out": 88000 },
      "daily_budget_usd": 25.0,
      "reconciliation": { "ledger_usd": 1.87, "credits": 940, "delta_flag": "ok" }
    }
  },
  "completed": {
    "availability": "unavailable",
    "freshness": "absent",
    "note": "gh: not authenticated",
    "data": null
  }
}
```

Consumers should treat any section as optional: check `availability` /
`freshness` before reading `data`. Unknown fields are ignored and missing fields
default, so the schema can grow additively.

## Dashboard endpoint — `GET /api/status/snapshot`

Added under `src/operator_commands_dashboard/`. Returns `Json<StatusSnapshot>`
— byte-identical in shape to `simard status --json`.

- **Auth.** Registered **behind** the existing dashboard `require_auth` layer —
  it is **not** a new auth path, and there is no unauthenticated metrics-scrape
  endpoint or auth-bypass env. It accepts the **same** credentials as every
  other `/api/*` route:
  - a `simard_session` **cookie** (set by the dashboard login), or
  - an `Authorization: Bearer <token>` header whose `<token>` is
    `SIMARD_DASHBOARD_TOKEN` — the path for scripted `curl`/CI reads.

  Anything else returns **401** (a JSON error body for `/api/*` paths).

  ```bash
  # The HTTP twin of `simard status --json`:
  curl -fsS -H "Authorization: Bearer $SIMARD_DASHBOARD_TOKEN" \
    http://localhost:8080/api/status/snapshot | jq '.daemon.freshness'
  ```

- **Network exposure.** The endpoint inherits the dashboard's bind address. If
  the dashboard is served on `0.0.0.0` rather than loopback, this endpoint is
  reachable on the same interface — restrict the network path (firewall / SSH
  tunnel) as you would for any other dashboard route.
- **Query params (optional):**
  - `sections=daemon,llm,memory` — allowlisted against the section enum,
    length-capped, never reflected into the response or logs. Unknown names are
    ignored.
  - `pretty=true|false` — strict bool parse; controls pretty-printing only.
- **Failure isolation.** A degraded source yields an `unavailable`/`error`
  section, not a 5xx. The endpoint returns 200 with per-section envelopes.
- **Secrets.** A serialized snapshot never contains `.dashkey`,
  `SIMARD_DASHBOARD_TOKEN`, or any credential — enforced by test.

The existing dashboard routes and tabs are untouched; the **Status** nav entry
and `fetchStatus()` client call are additive.

## TUI Status tab

The TUI registers `Tab::Status` (in `tabs/mod.rs` + `app.rs`) with a
`StatusCache` that calls `status::assemble()` on the **slow** refresh cycle
(the same cadence as the Stats tab), so the heavier `gh`/IPC reads never block
the UI. It renders the same section model as the CLI. Existing tabs are
unchanged; see [simard-tui](./simard-tui.md).

## Guarantees

- **Never panics.** Every source parser tolerates malformed output and non-zero
  exits; readers are size-capped and schema-checked.
- **No shell.** All subprocesses (`systemctl`, `gh`) use argument arrays
  (`Command::args([...])`), never a shell string; `gh` repositories come from a
  trusted allowlist, never from request input.
- **Process-agnostic.** Identical results from CLI, TUI, and dashboard because
  the sources are durable, not in-RAM.

## See also

- [Telemetry metrics reference](./telemetry-metrics.md) — where the numbers come
  from.
- [How to read `simard status`](../howto/simard-status.md) — the operator
  walkthrough with rendered output.
- [Unified telemetry and one `simard status`](../concepts/unified-telemetry-and-status.md)
  — the design rationale.
