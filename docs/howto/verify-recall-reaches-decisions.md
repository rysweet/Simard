---
title: "How to verify recalled memory is reaching Simard's decisions"
description: The operator playbook for #2942 — read the dashboard Memory-tab "Recall reaching decisions" panel, watch the per-turn simard::enrichment INFO/WARN lines, and run the recall-on-vs-recall-off ablation to get a reproducible yes/no on "recalled memory influences decisions". Includes what a silent-degrade warning looks like and how to confirm the attach-rate is 100%.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/enrichment-observability.md
  - ../reference/enrichment-observability-api.md
  - ./measure-recall-precision-hybrid.md
  - ./run-ooda-daemon.md
  - ./simard-status.md
---

# How to verify recalled memory is reaching decisions

Simard recalls memory on every OODA turn — but *recalling* is not *using*. This
guide shows how to prove, live, that recalled memory actually reaches Simard's
decisions, and how to spot the moment it silently stops (a degraded memory
bridge). For the rationale see the
[concept](../concepts/enrichment-observability.md); for the full contract see the
[API reference](../reference/enrichment-observability-api.md).

There are three checks, cheapest to strongest: the **dashboard panel** (at a
glance), the **per-turn logs** (per decision), and the **ablation eval** (hard
proof).

## Prerequisites

- A built `simard` binary (or run through `cargo run --`).
- For the dashboard/log checks: the OODA daemon running — see
  [Run the OODA daemon](./run-ooda-daemon.md).
- The daemon and any CLI checks run from the **same `SIMARD_STATE_ROOT`** so they
  read the same `metrics_snapshot.json` / `metrics/metrics.jsonl`.

## Check 1 — the dashboard panel (at a glance)

1. Start the dashboard: `simard dashboard serve --port=8080`.
2. Open the **Memory** tab: `http://localhost:8080/#memory`.
3. Find the **"Recall reaching decisions"** panel.

Read it as follows:

| Panel field | Healthy | What a problem looks like |
|---|---|---|
| **Attach-rate** | `100%` (green) — every recent decision received recalled memory | Below `100%` (amber/red) — some decisions ran with **no** recalled memory |
| **Avg facts / procedures per decision** | Non-zero when the store has content | `0` across a populated store suggests recall is not landing |
| **Avg preamble bytes** | Non-zero | `0` means nothing was injected |
| **Degrade breakdown** | `memory_ipc: 0`, `knowledge_launch: 0` | Any non-zero (shown red) names the failing bridge |
| **Freshness** | `live` | `stale` (daemon paused) or `missing` (fresh brain / daemon down) — **not** a real `0%` |

> **Freshness first.** A `missing` or `stale` badge is **not** a `0%` attach-rate
> — it means there is no fresh snapshot to read yet. Start/resume the daemon and
> let a cycle complete before trusting the numbers.

> **Scope.** These figures cover only sessions that **configured** enrichment
> (the daemon's OODA decisions). Adapters, CLIs, and tests that run without an
> enrichment bridge are *not* counted, so a healthy daemon reads `100%` even when
> other non-enriching work is running.

You can hit the endpoint directly (it is auth-gated — reuse your dashboard token):

```console
$ curl -s -H "Authorization: Bearer $SIMARD_DASHBOARD_TOKEN" \
    "http://localhost:8080/api/enrichment?window_hours=6" | jq
{
  "available": true,
  "freshness": "live",
  "snapshot_age_seconds": 41,
  "window_hours": 6,
  "decisions": 42,
  "attached": 40,
  "attach_rate": 0.9524,
  "degraded": { "memory_ipc": 2, "knowledge_launch": 0 },
  "avg_facts_injected": 6.3,
  "avg_procedures_injected": 2.8,
  "avg_preamble_bytes": 771.5,
  "last": { "attached": true, "facts_injected": 7, "procedures_injected": 3, "preamble_bytes": 812, "at": "2026-07-07T19:58:11Z" }
}
```

An `attach_rate` below `1.0` with `degraded.memory_ipc > 0` is the signature of a
memory bridge that failed to launch (classically a `memory-ipc` **Broken pipe**)
— decisions in that window ran without recalled memory.

## Check 2 — the per-turn logs (per decision)

Every turn emits one line under the `simard::enrichment` target. Enable it and
watch:

```console
$ RUST_LOG=simard::enrichment=info simard ooda daemon
```

Healthy attach:

```text
INFO simard::enrichment: enrichment applied attached=true facts=7 procedures=3 preamble_bytes=812 objective="raise unit-test coverage on the goal-board store"
```

A **loud** degrade — the whole point of #2942. This is never silent, and it
fires only when the session *expected* a memory bridge:

```text
WARN simard::enrichment: cognitive-memory bridge unavailable — memory enrichment disabled for this session reason=memory_ipc
WARN simard::enrichment: enrichment degraded — memory bridge expected but not attached; decision proceeding without recalled memory attached=false expected=true facts=0 procedures=0 preamble_bytes=0 objective="triage stale pull requests"
```

> **`INFO … expected=false` is not a degrade.** A session that never configured
> enrichment (a non-enriching adapter, a CLI, a test) logs an `INFO` line with
> `expected=false`, **not** a `WARN`, and is excluded from the attach-rate. So
> the rule holds: **any `WARN` under `simard::enrichment` is a real degrade.**

To see the raw underlying error on a degrade, drop to debug:

```console
$ RUST_LOG=simard::enrichment=debug simard ooda daemon
```

Grep a running daemon's log for degrades:

```console
$ grep 'simard::enrichment' "$SIMARD_LOG" | grep -i 'degraded\|unavailable'
```

No matches = no degrades. Any match names the reason (`memory_ipc` /
`knowledge_launch`) and the affected decision.

> **`attached=true facts=0`** is not a failure — it means the bridge is up but the
> store had nothing to recall for that objective. Recall *quality* is a separate
> metric; see [measure recall precision](./measure-recall-precision-hybrid.md).

## Check 3 — the ablation eval (hard proof)

The logs prove recall was *injected*; the ablation proves it *matters*. It runs a
representative decision **with recall** vs **with recall suppressed** and reports
a reproducible verdict. It is hermetic — no daemon, no network:

```console
$ simard gym enrichment-ablation
cognition/enrichment_ablation: recall_on_bytes=812 recall_off_bytes=0 delta_bytes=812 facts=7 procedures=3 preambles_differ=true verdict=influences
```

- `verdict=influences` (with `delta_bytes > 0` and `preambles_differ=true`) is a
  reproducible **yes** on "recalled memory influences decisions".
- `verdict=no-influence` (`delta_bytes=0`) is an honest **no** — recall was inert
  for that decision; investigate the store contents or the seam.

Each run records an `enrichment_ablation_delta` sample into `metrics.jsonl`,
feeding the [hybrid cognition self-measurement](./measure-recall-precision-hybrid.md)
so the claim is tracked over time. Confirm the sample landed:

```console
$ grep '"enrichment_ablation_delta"' \
    "${SIMARD_STATE_ROOT:-$HOME/.simard}/metrics/metrics.jsonl" | tail -1
{"timestamp":"2026-07-07T20:05:00Z","metric_name":"enrichment_ablation_delta","value":812.0,"context":"{\"site\":\"enrichment_ablation\",\"verdict\":\"influences\"}"}
```

## Putting it together

| You want to know | Use |
|---|---|
| "Is recall reaching decisions **right now**?" | Check 1 — the Memory-tab panel / `GET /api/enrichment` |
| "Which decision missed recall, and **why**?" | Check 2 — the `simard::enrichment` `INFO`/`WARN` lines |
| "Does recalled memory actually **change** a decision?" | Check 3 — `simard gym enrichment-ablation` |

If the attach-rate is `100%`, no degrade `WARN`s appear, and the ablation returns
`verdict=influences`, you have live evidence — not just wired code — that recalled
memory is reaching and moving Simard's OODA decisions (#2942).

## See also

- [Concept: enrichment observability](../concepts/enrichment-observability.md)
- [Enrichment observability API reference](../reference/enrichment-observability-api.md)
- [How to measure recall precision on both rails](./measure-recall-precision-hybrid.md)
- [Run the OODA daemon](./run-ooda-daemon.md)
