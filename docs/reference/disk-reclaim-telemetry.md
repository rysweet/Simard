---
title: Disk reclaim telemetry
description: Reference for the simard.disk.reclaim.* metrics emitted by the agentic disk-reclamation capability — bytes_freed, paths_removed, and candidates_skipped counters plus the used_pct_before/after gauges, their low-cardinality attribute enums, and how they ride the unified telemetry facade.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/agentic-disk-reclamation.md
  - ../howto/configure-disk-reclamation.md
  - ./disk-reclaim-api.md
  - ./telemetry-metrics.md
---

# Disk reclaim telemetry

The agentic disk-reclamation capability emits its outcome through the unified
Simard telemetry facade (`src/telemetry/`). Every metric name is a stable
constant in `src/telemetry/names.rs` and rides the same registry → snapshot →
optional-OTLP path as every other Simard metric. See the
[telemetry metrics reference](./telemetry-metrics.md) for the facade, the
in-process registry, the on-disk snapshot, and OTLP export gating.

All emission is **additive** — no existing metric is changed.

## Metric catalog — `simard.disk.reclaim.*`

Emitted once per reclamation run (both the daemon self-heal path and
`simard disk-reclaim`). Dry-run emits the gauges and `candidates_skipped` (the
would-be-skipped counts described below); only `--apply` runs increment
`bytes_freed` / `paths_removed`, and only for actually-removed paths.

Every series below also carries a `source` attribute distinguishing the daemon
self-heal path from operator CLI runs, so an operator dry-run cannot be mistaken
for daemon activity on a dashboard:

- **`source`** = `daemon` (the self-healing trigger) \| `cli` (`simard disk-reclaim`).

| Metric | Type | Attributes | Meaning |
| ------ | ---- | ---------- | ------- |
| `simard.disk.reclaim.bytes_freed` | counter | `source` | bytes actually reclaimed this run (sum of freshly-measured sizes of removed paths). `0` on a dry-run or a no-op run. |
| `simard.disk.reclaim.paths_removed` | counter | `source`, `kind` = `tracked_worktree` \| `orphan_dir` \| `stale_build_cache` | paths actually removed this run, tagged by the reclamation primitive that applied. |
| `simard.disk.reclaim.candidates_skipped` | counter | `source`, `reason` = `protected_path` \| `live_process` \| `uncommitted_or_unpushed` \| `active_worktree` \| `outside_allow_root` \| `unknown_pr_state` | candidates a rail refused, tagged by `RejectReason`. These are the human-review list; **every increment is a candidate that was NOT deleted.** |
| `simard.disk.reclaim.used_pct_before` | gauge | `source` | home-partition `%-used` measured at the start of the run (0–100). |
| `simard.disk.reclaim.used_pct_after` | gauge | `source` | home-partition `%-used` after the run (0–100). On a dry-run this equals `used_pct_before` (nothing removed). |

### Attribute enums

Attribute values are **fixed low-cardinality enums** — the facade coerces
anything outside the set to `other` and never uses paths, PR bodies, or agent
free-text as labels.

- **`kind`** mirrors `disk_reclaim::CandidateKind`:
  `tracked_worktree`, `orphan_dir`, `stale_build_cache`.
- **`reason`** mirrors `disk_reclaim::guard::RejectReason`, a **closed six-variant
  enum**: `protected_path`, `live_process`, `uncommitted_or_unpushed`,
  `active_worktree`, `outside_allow_root`, `unknown_pr_state`. Because
  `RejectReason` is exhaustive, no reclaim skip ever maps to the facade's
  generic `other` overflow bucket — it stays reserved for facade-level coercion
  and is never emitted by this capability.
- **`source`** is `daemon` or `cli` (fixed).

> **Anti log-forging:** the agent's free-text `reason` field on a candidate is
> **never** emitted as an attribute. Only the enum `RejectReason` is. Agent
> strings are sanitized (length-capped, control-characters stripped) before any
> `tracing` log line, matching the facade's normalization.

## Reading the values

### In-process (`simard status`)

The gauges and counters are readable with no external collector via the
in-process registry that backs the status snapshot:

```bash
simard status | grep -i reclaim
```

### From the on-disk snapshot

The daemon flushes `~/.simard/telemetry/metrics_snapshot.json` once per OODA
cycle; the disk-reclaim series appear there like any other metric:

```bash
jq '.series[] | select(.name | startswith("simard.disk.reclaim"))' \
  ~/.simard/telemetry/metrics_snapshot.json
```

### From the run report

Every value is also present in the `ReclaimReport` returned by
`run_disk_reclaim` and printed by `simard disk-reclaim --report-json`
(`used_pct_before`, `used_pct_after`, `bytes_freed`, `removed[]`, `skipped[]`).
The telemetry counters are derived from that same report, so the snapshot and
the report never disagree. See [Disk reclaim API](./disk-reclaim-api.md).

## OTLP export

Export is gated identically to every other Simard metric: installed **only when
`OTEL_EXPORTER_OTLP_ENDPOINT` is set**, off by default. The disk-reclaim
attributes (`source`, `kind`, `reason`) are non-PII fixed enums and are safe to export.

## Suggested dashboards / alerts

| Signal | Expression (conceptual) | Why it matters |
| ------ | ----------------------- | -------------- |
| Reclamation is keeping up | `used_pct_after < SIMARD_DISK_RECLAIM_PCT` after runs | if `used_pct_after` stays ≥ target, reclamation cannot free enough — investigate the human-review list |
| Human-review backlog | rate of `candidates_skipped` by `reason` | a rising `uncommitted_or_unpushed` or `unknown_pr_state` rate means work is piling up that reclamation refuses to touch |
| Space recovered | `bytes_freed{source="daemon"}` over time | the self-healing yield (filter to `daemon` to exclude operator dry-runs) |
| Protected-path attempts | `candidates_skipped{reason="protected_path"}` | the agent nominating protected paths is expected occasionally; a spike may indicate a prompt regression (the guard still refuses them) |

> **Filter by `source` on all reclaim dashboards.** An operator dry-run
> (`source="cli"`) increments the same `candidates_skipped` / gauge series as the
> daemon. Alerts on "prompt regression" or "human-review backlog" should scope to
> `source="daemon"` so a manual `simard disk-reclaim` preview does not page anyone.

## Related

- [Agentic disk reclamation (concept)](../concepts/agentic-disk-reclamation.md) — design rationale
- [Configure disk reclamation (how-to)](../howto/configure-disk-reclamation.md) — operator usage
- [Disk reclaim API (reference)](./disk-reclaim-api.md) — the `ReclaimReport` these metrics derive from
- [Telemetry metrics reference](./telemetry-metrics.md) — the facade, registry, snapshot, and OTLP gating
