---
title: Configure the Overseer's memory recall
description: >
  How to enable, disable, and verify the acting Overseer's use of Simard's cognitive
  memory graph in its observe/orient loop. Covers the SIMARD_OVERSEER_MEMORY_RECALL
  opt-out flag and its interaction with SIMARD_OVERSEER_ENABLED, what the Overseer recalls
  (semantic / episodic / procedural / prospective) and writes back, how recurring-signature
  detection changes decisions, how to read the memory_recalls / memory_writes / memory_errors
  counters from simard status, the dashboard, the TUI, and GET /api/overseer, and how to
  confirm the no-silent-fallback error behaviour.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/overseer-memory-recall-api.md
  - ../design/overseer.md
  - ./watch-overseer-activity.md
  - ../reference/overseer-activity-feed.md
  - ./run-ooda-daemon.md
  - ../memory.md
  - ./simard-status.md
---

# Configure the Overseer's memory recall

The acting **Overseer** watches how Simard runs and quietly steps in — filing
issues, launching fixes, verifying/merging green PRs, escalating, or holding. It
now does that with **memory**: each cycle it recalls what Simard already learned
about the problems it is looking at, and records its own observations back into
the cognitive-memory graph.

This guide shows how to turn that on or off, what it changes, and how to confirm
it is working. For the API, data model, and security model, see the
[Overseer memory-recall reference](../reference/overseer-memory-recall-api.md).

## What it does

When memory recall is enabled, in every Overseer cycle the Overseer:

- **Reads the graph** for content relevant to the problems it just detected —
  **semantic** facts (prior root-causes), **episodic** memories (prior
  occurrences and their outcomes), **procedural** know-how (a stored runbook),
  and **prospective** triggers/ideas.
- **Detects recurring problems from memory**, not just in-process counters: when
  it recalls two or more past episodes sharing a problem's failure signature, it
  raises that problem's priority and surfaces the prior procedure.
- **Writes its observation back** as one episodic memory (de-duplicated within a
  15-minute window), so its stewardship activity becomes part of the graph the
  rest of Simard can recall.

It reuses the **same** cognitive-memory handle the daemon already shares — no
second store is created. Recall is bounded and runs on the panic-isolated tick
thread, so it can never stall or crash the loop, and if memory is unreachable the
error is **surfaced** (never silently ignored).

## Prerequisites

- A running OODA daemon (see [Run the OODA daemon](./run-ooda-daemon.md)).
- The acting Overseer enabled — it is **on by default**; it is off only when
  `SIMARD_OVERSEER_ENABLED` is an explicit falsey value.

## Enable or disable memory recall

Memory recall is **on by default** whenever the Overseer runs. Control it with a
single opt-out flag:

```bash
# Default: recall is ON — nothing to set.

# Turn recall OFF (Overseer keeps running, but stops reading/writing the graph):
export SIMARD_OVERSEER_MEMORY_RECALL=0     # 0 / false / no / off all work

# Turn it back ON explicitly:
export SIMARD_OVERSEER_MEMORY_RECALL=1     # or true / yes / on, or just unset it
```

Interaction with the master switch:

| `SIMARD_OVERSEER_ENABLED` | `SIMARD_OVERSEER_MEMORY_RECALL` | Recall active? |
| ------------------------- | ------------------------------- | -------------- |
| unset / truthy (default)  | unset / truthy (default)        | **yes**        |
| unset / truthy            | `0` / `false` / `no` / `off`    | no             |
| `0` / `false` / `no` / `off` | anything                     | no (Overseer off) |

A malformed value never crashes the daemon: anything that is not an explicit
falsey value leaves recall **enabled**.

When recall is off, the Overseer behaves exactly as before — it still sees the
memory-node **count**, but `ObservedState.recall` stays empty, nothing is written
back, and the recall counters stay `0`.

## Verify it is working

Memory recall is fully observable through the existing
[Overseer activity feed](../reference/overseer-activity-feed.md). Three per-tick
counters are surfaced everywhere the feed is:

| Counter          | Meaning                                                     |
| ---------------- | ----------------------------------------------------------- |
| `memory_recalls` | **1** when this tick's recall pass completed (all sub-reads succeeded); **0** otherwise — at most 1 per tick. |
| `memory_writes`  | Observations actually persisted (dedup suppressions excluded). |
| `memory_errors`  | Recall/write attempts that failed and were **surfaced**.    |

### From the daemon log

Each Overseer tick logs one structured line; the memory counters sit next to the
existing tallies:

```text
[simard] overseer tick: problems=2 issues_filed=0 recipes_launched=1 \
  prs_merged=0 … memory_recalls=1 memory_writes=1 memory_errors=0
```

### From `simard status`

The `OVERSEER` section of [`simard status`](./simard-status.md) reflects the same
tick report, including the memory counters, so you can confirm recall is running
and error-free at a glance.

### From the dashboard, TUI, and API

The dashboard **Overseer** tab, the TUI **Overseer** pane, and
`GET /api/overseer` all read the same feed (see
[Watch what the Overseer is doing](./watch-overseer-activity.md)). A healthy
steward shows `memory_recalls` advancing with `memory_errors = 0`.

```bash
# Scripting: pull the latest Overseer ticks (auth-gated endpoint)
curl -s -H "Authorization: Bearer $SIMARD_DASHBOARD_TOKEN" \
  http://localhost:8080/api/overseer | jq '.records[0]'
```

## Reading the signals

- **`memory_recalls` climbing, `memory_errors = 0`** — recall is healthy; the
  Overseer is consulting the graph each cycle.
- **`memory_writes` far below `memory_recalls`** — expected. Write-back is
  de-duplicated within a 15-minute window, so a stable situation produces few
  writes even though every cycle recalls.
- **`memory_errors > 0`** — the memory graph was unreachable or erroring on some
  ticks. This is **surfaced on purpose** (no silent fallback): the tick still
  completes, and a matching `tracing::warn!` names the failure. Check the
  cognitive-memory backend / memory-IPC socket health (see
  [Cognitive memory](../memory.md)). For any tick whose recall pass errors the
  **whole** pass is discarded (`ObservedState.recall` stays empty — never a
  partial snapshot) and `memory_recalls` does not advance, so the Overseer never
  orients on a half-read graph rather than making decisions on wrongly-empty data.

## Example: a recurring problem the Overseer now remembers

1. Distillation parse failures spike; the Overseer detects a
   `ProcessHealth` problem and files/launches a fix. It records an episodic
   observation keyed on that failure signature.
2. Days later the same signature recurs. This cycle the Overseer **recalls** the
   earlier episodes, sees ≥2 sharing the signature, and raises a
   `RecurringSignature` — promoting the problem to `Priority::High` and surfacing
   the **procedure** it recalled for that shape, instead of treating it as brand
   new.
3. The advisory problem still passes every existing gate (dedup / whisper /
   priority / autonomy) before any action — recall *informs* the decision, it
   never commands a merge or deploy.

## Turn it off safely

Disabling recall is always safe and immediate — it is purely additive:

```bash
export SIMARD_OVERSEER_MEMORY_RECALL=off
# restart the daemon; the Overseer runs exactly as it did before recall existed.
```

## See also

- [Overseer memory-recall API (reference)](../reference/overseer-memory-recall-api.md)
- [Watch what the Overseer is doing](./watch-overseer-activity.md)
- [Overseer — operator/observer co-process (design)](../design/overseer.md)
- [Cognitive memory](../memory.md)
- [Read Simard status](./simard-status.md)
