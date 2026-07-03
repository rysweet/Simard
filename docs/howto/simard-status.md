---
title: Read Simard's status with `simard status`
description: How to run the unified `simard status` report from the CLI, dashboard, or TUI; read every section (daemon, resources, LLM usage, memory/brain, gym, goals, workstreams, completed work, self-improvement, telemetry signals); consume the --json form; and interpret freshness, the two-books cost view, and anomaly flags.
last_updated: 2026-07-03
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/status-snapshot-api.md
  - ../reference/telemetry-metrics.md
  - ../concepts/unified-telemetry-and-status.md
  - ../reference/simard-cli.md
  - ../dashboard.md
  - ../reference/simard-tui.md
  - ./run-ooda-daemon.md
---

# Read Simard's status with `simard status`

`simard status` prints one consolidated report of what Simard is doing right
now — the daemon, resources, LLM spend, memory and brain health, gym state,
goals, active and completed work, and any unexpected telemetry signals. The
**same** report is available three ways:

- `simard status` — the canonical terminal layout (this guide).
- The dashboard **Status** tab (`GET /api/status/snapshot`).
- The TUI **Status** tab.

All three render the same [`StatusSnapshot`](../reference/status-snapshot-api.md),
assembled from **durable sources** (the metrics snapshot file — including the
daemon-sampled memory gauges — the cost ledger, and `systemctl show` + `/proc`)
— **not** by grepping journald. So you get the same numbers whether Simard's
daemon is on this host or you are reading from a separate shell.

## Prerequisites

- The Simard binary on `PATH`.
- For the richest report, the Simard **daemon** running (`simard ooda run` or
  the systemd unit) so `~/.simard/telemetry/metrics_snapshot.json` is fresh.
  Without the daemon, live sections degrade to `stale`/`absent` — the command
  still succeeds.
- Optional: `gh` authenticated, for the **Completed work** and
  **Self-improvement** sections. Without it those render
  `unavailable (gh: <reason>)`.

## 1. Run it

```bash
simard status
```

A full report on a healthy host looks like this:

```text
SIMARD STATUS  ·  2026-07-03T03:55:05Z

DAEMON / UPTIME
  state             active (running)
  version           0.1.0  (deployed commit e5764c6d)
  main PID          48291
  this-instance up  2h 14m 33s   (running since 2026-07-03T01:40:31Z)
  NRestarts         0

RESOURCE SNAPSHOT
  daemon CPU / RSS  3.2%  ·  184 MiB   (cgroup mem peak 402 MiB)
  load avg          0.41 / 0.55 / 0.60   (1 / 5 / 15m)
  system mem        6.1 / 16.0 GiB used   (9.4 GiB avail)
  disk /home        118 GiB free / 256 GiB      disk /tmp   14 GiB free / 16 GiB
  live engineers    2

LLM USAGE
  copilot turn      in 4,120  cached 1,900  out 880   ·  AI-credits 12
  ledger today      $1.87    in 412,000  out 88,000
  ledger 7d         $11.42   in 2,740,000  out 610,000
  ledger all-time   $208.91  in 51,300,000  out 9,900,000
  daily budget      $1.87 / $25.00  (OK — 7% used)
  reconciliation    ledger $1.87  vs  credits 940   ·  OK (within tolerance)

MEMORY / BRAIN
  store             /home/azureuser/.simard/cognitive  ·  38 MiB  (amplihack-memory-lib)
  nodes             1,842 total
                      episodic 1,204  ·  semantic (facts) 380  ·  prospective (triggers) 44
                      working 12  ·  procedural 190  ·  sensory 12
  edges             DERIVES_FROM 512  ·  SIMILAR_TO 233  ·  SUPERSEDES 61
  cognitive         distillation OK  ·  consolidation OK  ·  introspection OK
  brains            LLM-backed 3/3  ·  fallbacks 0  ·  decide ladder_exhausted 0

GYM
  SIMARD_SKIP_GYM   unset (gym enabled)
  scenarios         7 configured
  self-eval         idle

GOAL BOARD
  [p0] in-progress   Rationalize telemetry onto OpenTelemetry      (goal-2f9c)
  [p1] in-progress   Fix auth token refresh                        (goal-a13b)
  [p2] not-started   Add retry logic to bridge                     (goal-77de)

ACTIVE WORKSTREAMS
  recipe   ooda-cycle              running — decide phase, cycle 47
  engineer eng-alpha (goal-2f9c)   running — 0h4m, editing src/telemetry/
  engineer eng-beta  (goal-a13b)   running — 0h1m, tests

COMPLETED WORK (merged PRs, last ~24h)
  rysweet/Simard
    #2526  char-boundary-safe truncation on recovery/UI paths     merged
    #2525  clamp SIMARD_OODA_INTERVAL_SECS=0 + reload-gate         merged

SELF-IMPROVEMENT
  merged   #2523  char-boundary-safe truncation (engineer,knowledge)
  running  self-quality-audit      auditing exception handling
  pending  —

TELEMETRY / UNEXPECTED SIGNALS (last 1h)
  parse-fix holding   yes (distill parse-fail 0%)
  restart churn       none (0 restarts/1h)
  gym skipped         no
  budget              OK
  anomalies           none (no panics / segv / corruption / fallback / budget)
```

Every label and section above is stable — the dashboard and TUI tabs mirror this
exact section model.

> **What's live today.** `DAEMON / UPTIME`, `RESOURCE SNAPSHOT`, `LLM USAGE`,
> `MEMORY / BRAIN` (from the daemon-sampled memory gauges), `GYM`, and
> `TELEMETRY / UNEXPECTED SIGNALS` are populated by `simard status` from durable
> sources. `GOAL BOARD`, `ACTIVE WORKSTREAMS`, `COMPLETED WORK`, and
> `SELF-IMPROVEMENT` render `unavailable (<reason>)` from the process-agnostic
> CLI in this release — the goal board is surfaced live in the daemon-hosted
> dashboard and TUI goal tabs. The example above shows the full target layout;
> the frame (headers + honest `unavailable` markers) is always complete and a
> missing count is never shown as a fabricated `0`. Some example numbers
> (`cgroup mem peak`, per-turn `AI-credits`, `cognitive` health) are likewise
> illustrative of the layout.

## 2. Read each section

### DAEMON / UPTIME

Service `state`, `version` and the **deployed commit** it was built from, the
main PID, **this instance's** uptime and start time, and `NRestarts`. A rising
`NRestarts` is **churn** — investigate before trusting the other live sections.
Sourced from `systemctl show simard.service` + `/proc` (not log scraping).

### RESOURCE SNAPSHOT

Daemon CPU / RSS and cgroup memory peak, 1/5/15-minute load average, system
memory, free disk on `/home` and `/tmp`, and the live engineer count. From
`/proc`, cgroup files, and `sysinfo`-style reads.

### LLM USAGE

The most recent Copilot per-turn tokens (in / cached / out) and AI-credits; the
cost **ledger** for today / 7d / all-time (cost + in/out tokens); and the daily
budget guard versus `SIMARD_DAILY_BUDGET_USD`. The **reconciliation** line shows
the **two books** — dollar ledger and Copilot AI-credits — side by side with a
delta; it flags `under-count`/`over-count` if they diverge beyond tolerance.
See the [two-books design](../concepts/unified-telemetry-and-status.md#two-books-of-cost-told-honestly).

### MEMORY / BRAIN

Store path, size, and backend (the `amplihack-memory-lib` store at
`<state_root>/cognitive`); total nodes and the per-type breakdown (episodic /
semantic (facts) / prospective (triggers) / working / procedural / sensory);
edges per type (DERIVES_FROM / SIMILAR_TO / SUPERSEDES); cognitive-process health
(distillation / consolidation / introspection); and brain health — how many
brains are LLM-backed, the fallback count, and `decide` `ladder_exhausted`.
Rising fallbacks or ladder-exhausted counts mean the brain is degrading to empty
or malformed decisions.

### GYM

Whether `SIMARD_SKIP_GYM` is set, how many scenarios are configured, and whether
self-eval is idle or active.

### GOAL BOARD

Active goals with priority (`p0` highest), status, and a short id you can pass to
`simard goal …`.

### ACTIVE WORKSTREAMS

Operator recipes (e.g. the OODA cycle) and engineer workstreams, each with a
one-line status. Engineers are matched to their goal id.

### COMPLETED WORK

Merged PRs from roughly the last 24h across governed repos, grouped
`repo -> #PR summary -> status`. Requires `gh`; renders
`unavailable (gh: <reason>)` otherwise.

### SELF-IMPROVEMENT

Self-fix and audit workstreams and PRs, split into merged / running / pending.

### TELEMETRY / UNEXPECTED SIGNALS

A rolling-window anomaly summary: is the distill parse-fix holding (parse-fail
%), any panics / segv / corruption / fallback / budget events, restart churn,
and whether gym is skipped. This section is derived from the **structured**
metrics — never from `journalctl | grep`.

## 3. Machine-readable output

```bash
simard status --json
```

Emits the serialized [`StatusSnapshot`](../reference/status-snapshot-api.md).
Every section is wrapped in an envelope with `availability`, `freshness`,
`as_of`, and `note`, so scripts can distinguish "zero" from "unknown". Examples:

```bash
# Today's ledger dollar cost
simard status --json | jq '.llm.data.ledger_today.cost_usd'

# Fail loudly in CI if the daemon isn't live
simard status --json | jq -e '.daemon.freshness == "live"'

# Alert on distill parse failures in the window
simard status --json | jq '.telemetry.data.distill_fail_pct'

# Is cost reconciliation flagging a divergence?
simard status --json | jq -r '.llm.data.reconciliation.delta_flag'
```

Always check `availability` / `freshness` before reading `data` — a degraded
source sets `data` to `null` rather than inventing a value.

## 4. Reading it in the dashboard or TUI

- **Dashboard:** open the **Status** tab. It calls `GET /api/status/snapshot`
  (behind the dashboard login) and renders the same sections. To read that
  endpoint **programmatically** (the HTTP twin of `simard status --json`),
  authenticate with an `Authorization: Bearer $SIMARD_DASHBOARD_TOKEN` header —
  see the [endpoint reference](../reference/status-snapshot-api.md#dashboard-endpoint-get-apistatussnapshot).
  The endpoint rides the **same bind and auth** as the rest of the dashboard, so
  if you serve it on `0.0.0.0`, protect the network path accordingly. See
  [Dashboard](../dashboard.md).
- **TUI:** launch `simard-tui` and select the **Status** tab. It refreshes on
  the slow cycle so `gh`/IPC reads never stall the UI. See
  [simard-tui](../reference/simard-tui.md).

## 5. Interpreting freshness

Because the report is assembled cross-process, each section can be `live`,
`stale`, or `absent` independently:

| You see | Meaning | What to do |
|---|---|---|
| a value | `live` — source read within its window | trust it |
| value + `(stale)` | last-known value; source older than its window (daemon not flushing, DB locked) | check the daemon is running and cycling |
| `absent` / `unavailable (<reason>)` | source missing (no snapshot yet, `gh` unauthenticated) | start the daemon; run `gh auth login` |

A missing number is **never** rendered as `0`. "0 restarts" and "restarts
unknown" are different facts, and `simard status` keeps them different.

## Configuration

| Variable | Effect |
|---|---|
| `SIMARD_STATE_ROOT` | State root to read (`telemetry/metrics_snapshot.json`, cost ledger, memory). Defaults to `$HOME/.simard`. |
| `SIMARD_DAILY_BUDGET_USD` | Sets the daily budget the LLM-usage section compares spend against. |
| `SIMARD_SKIP_GYM` | Reflected in the GYM section. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Enables OTLP export of metrics **and** traces; unrelated to reading `simard status`, which is always local. Off by default. |

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Most sections `stale`/`absent` | daemon not running, so `metrics_snapshot.json` is old/missing | start the daemon: `simard ooda run` |
| DAEMON section `unavailable` | non-systemd host or `systemctl` unreachable | expected off systemd; other sections still render |
| COMPLETED / SELF-IMPROVEMENT `unavailable (gh: …)` | `gh` missing or unauthenticated | `gh auth status`, then `gh auth login` |
| MEMORY section `stale` | memory DB held under exclusive lock | transient; retry — last-known counts shown meanwhile |
| Reconciliation flags `under/over-count` | ledger $ and AI-credits diverged beyond tolerance | expected when the two accounting systems drift; both numbers are shown so you can see which |
| Numbers differ from the dashboard | one surface read an older snapshot flush | they converge on the next OODA cycle flush |

## See also

- [StatusSnapshot API reference](../reference/status-snapshot-api.md) — the
  types, provider, `--json` schema, and dashboard endpoint.
- [Telemetry metrics reference](../reference/telemetry-metrics.md) — the metric
  catalog behind the report.
- [Unified telemetry and one `simard status`](../concepts/unified-telemetry-and-status.md)
  — why it is one report, three surfaces.
- [Simard CLI reference](../reference/simard-cli.md) — the full command tree.
