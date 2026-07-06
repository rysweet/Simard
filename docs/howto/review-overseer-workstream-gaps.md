---
title: Review the Overseer's workstream gaps
description: >
  How to read, act on, and tune the Overseer's recurring "what workstreams are we
  missing?" gap-scan — the backlog-coverage gaps it flags each tick (uncovered
  high-priority goals, high-signal issues, and unaddressed anomalies), where the
  deduped operator notification and filed issue show up, and how to turn the scan
  up, down, or off with SIMARD_OVERSEER_GAP_SCAN.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/overseer-workstream-gap-scan.md
  - ./watch-overseer-activity.md
  - ../reference/overseer-activity-feed.md
  - ../design/overseer.md
  - ../reference/stewardship-api.md
  - ./simard-status.md
---

# Review the Overseer's workstream gaps

The acting **Overseer** now asks, on every tick, the question you would ask in a
standup:

> **"What workstreams are we missing?"**

It surveys the whole picture — the goal board, open GitHub issues, and live
telemetry — and flags **backlog-coverage gaps**: important work that *should*
have an active workstream but does not. Then it tells you (email + Signal) and
files a deduped issue, **once** per gap. This guide shows how to see those gaps,
act on them, and dial the scan in.

For the full data model, config semantics, and guarantees, see the
[workstream gap-scan reference](../reference/overseer-workstream-gap-scan.md).

## What the Overseer flags

A gap is **genuine uncovered work** — the Overseer only flags something after
checking it is **not** already being worked (no active workstream, no open PR).
It looks at three sources:

- **Goal board** — a **p1/p2** goal that is active but has **no** engineer and
  **no** PR (nobody is actually driving it).
- **Open issues** in `rysweet/Simard` — a **high-signal** issue (`bug`, `P1`, or
  `workflow:default`) with **no** open PR and **no** workstream.
- **Live anomalies** — a standing problem in telemetry (distill parse-fail rate
  high, restart churn, ladder exhausted) with **no** fix in flight.

Genuinely *blocked* goals ("needs human review") are handled by the separate
goal-board health path and are **not** re-flagged here — so you never get two
pings for the same stuck goal.

## Where the gaps show up

### 1. The operator notification (push)

When the Overseer finds a genuine, not-yet-seen gap, you get **one** notification
on **both** channels — email and Signal — kind `workstream-gap`:

```
Subject: [Overseer] workstream-gap: 2 uncovered workstream(s)

The Overseer autonomously flagged backlog-coverage gaps in rysweet/Simard.

Uncovered work:
  • goal g-1873 — harden distill parser: p1 goal with no engineer and no PR
  • anomaly distill_parse_fail — distill parser: parse-fail rate high with no fix in flight
```

It is **deduped**: the same gap is not re-sent on the next tick. You will only
hear about it again if it is still uncovered after the dedup window, or if it
recurs after being resolved.

### 2. The filed issue (durable)

For each gap the Overseer also files (or updates) a **deduped** stewardship issue
in `rysweet/Simard`, authored by its own identity. Find them with:

```bash
gh issue list --repo rysweet/Simard \
  --author 'simard-overseer[bot]' \
  --search 'workstream-gap in:body' \
  --state open
```

Because filing goes through the same dedup path as every other Overseer issue, a
recurring gap maps to **one** issue that gets updated — not a new issue each tick.

### 3. The Overseer activity surfaces (pull)

Each tick's gap count shows up wherever you already watch the Overseer (see
[watch what the Overseer is doing](./watch-overseer-activity.md)):

- **Dashboard → Overseer tab** (`http://localhost:8080/`, click **Overseer**)
- **TUI → Overseer pane** (`Alt+8`)
- **`simard status`** → the **OVERSEER** section

A tick that found gaps reads in plain language, for example:

> saw 3 problems, flagged 2 workstream gaps

(The gap count is its **own** clause — filing/notifying a gap does not add to the
generic "filed N issues" or "escalated N to the operator" clauses.)

A clean board adds **no** line — "observing, 0 interventions" is the honest,
correct state.

### 4. As JSON (scripting)

The gap counter rides the existing auth-gated endpoint — no new route:

```bash
curl -s -H "Authorization: ****** \
  http://localhost:8080/api/overseer \
| jq '.section.data | {
    total_gaps: .totals.workstream_gaps_detected,
    last_tick_gaps: .recent[0].report.workstream_gaps_detected
  }'
```

```json
{ "total_gaps": 3, "last_tick_gaps": 2 }
```

## How to respond to a gap

Each gap tells you **what** is uncovered and **why it matters**. Typical
responses:

- **Uncovered p1/p2 goal** — assign an engineer or promote it, or (if it is no
  longer important) demote/close it so it stops being flagged.
- **High-signal issue with no PR** — pick it up, or launch a workstream for it.
- **Unaddressed anomaly** — investigate the telemetry (distill parse failures,
  restart churn, ladder exhaustion) and start a fix.

Once real work exists — an assigned engineer, an open PR, or an active workstream
referencing the item — it lands in the Overseer's **coverage set** and is no
longer flagged. You do not need to close the Overseer's notification manually;
covering the work is what clears it.

The gap-scan itself only **surfaces** work — it notifies and files, it does not
launch. If an underlying anomaly separately warrants a fix, the Overseer may
launch it through its **existing** anomaly-fix path (subject to the same budget,
concurrency, and conflict gates as every other action), and you will see a
`launched N workstream(s)` clause from *that* path — not attributed to the
gap-scan. No new autonomous launch path is opened.

## Turn the scan up, down, or off

The gap-scan is **on by default** whenever the acting Overseer runs. Two env
knobs (see the [reference](../reference/overseer-workstream-gap-scan.md#configuration)):

| Env var | What it does | Default |
|---|---|---|
| `SIMARD_OVERSEER_GAP_SCAN` | Set to a falsey value (`0`/`false`/`no`/`off`) to **turn the scan off**. Unset or truthy → on. | on |
| `SIMARD_OVERSEER_GAP_SCAN_EVERY_N` | Run the scan every *Nth* Overseer tick — e.g. `4` ≈ hourly at the default 15-minute cadence. Clamped to a floor of `1`. | `1` (every tick) |

Examples:

```bash
# Turn the gap-scan off entirely (Overseer keeps doing everything else).
export SIMARD_OVERSEER_GAP_SCAN=off

# Scan roughly hourly instead of every 15-minute tick.
export SIMARD_OVERSEER_GAP_SCAN_EVERY_N=4
```

Disabling the acting Overseer (`SIMARD_OVERSEER_ENABLED=0`) also disables the
gap-scan — a gap-scan only makes sense while the Overseer runs. Changes take
effect on the next daemon start.

## FAQ

**Will I get spammed if a gap stays open for days?**
No. Notifications and issues are **deduped** by a stable per-gap signature, so a
recurring gap produces **one** notification and **one** (updated) issue, not one
per tick.

**Why didn't a blocked goal show up as a gap?**
Genuinely blocked / "needs human review" goals are handled by the goal-board
health path and escalated there, so the gap-scan deliberately does **not**
re-flag them. You still get told — just through that path, once.

**A gap looks stale — the work is already in progress.**
If an engineer, PR, or workstream references the item, it is in the coverage set
and will drop off on the next scan. If it lingers, confirm the PR/workstream
actually references the goal or issue (the correlation is by reference).

**Can external issue titles inject anything into my inbox or a `gh` command?**
No. External titles are treated as untrusted: they are escaped, truncated, and
provenance-labelled before rendering, and every `gh` call uses argument-safe
invocation — see the reference's
[security notes](../reference/overseer-workstream-gap-scan.md#security-notes).

## See also

- [Workstream gap-scan reference](../reference/overseer-workstream-gap-scan.md)
  — the data model, dedup grammar, config, and guarantees.
- [Watch what the Overseer is doing](./watch-overseer-activity.md) — the four
  Overseer surfaces this scan renders on.
- [Overseer activity feed reference](../reference/overseer-activity-feed.md) — the tick
  report and totals the gap counter joins.
- [Overseer design](../design/overseer.md) — the meta-OODA loop and guardrail
  model behind the scan.
