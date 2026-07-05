---
title: Watch what the Overseer is doing
description: How to see the acting Overseer's recent activity — what it observed, what it changed, and why it held — from the dashboard Overseer tab, the TUI Overseer pane, simard status, and the GET /api/overseer endpoint; how to read the honest disabled/observing/absent states; and how to turn the steward up, down, or off.
last_updated: 2026-07-05
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/overseer-activity-feed.md
  - ../reference/status-snapshot-api.md
  - ./simard-status.md
  - ../dashboard.md
  - ../reference/simard-tui.md
  - ./run-ooda-daemon.md
  - ../reference/cognitive-thread-scheduling.md
---

# Watch what the Overseer is doing

Simard runs two kinds of work side by side. The **engineer** side picks up
goals and writes code — you already see that on the dashboard (Goals, live
engineers) and in the TUI. The **steward** side — the acting **Overseer** — runs
on its own clock, watching the whole system and quietly stepping in: filing
issues, launching fixes, verifying and merging green PRs, running guarded
deploys, escalating to you, or **waiting** when a gate says "not yet".

This guide shows how to *watch* that steward activity — the last ~100 Overseer
ticks and their outcomes, plus per-thread status — in four places, all showing
the **same** data:

- the dashboard **Overseer** tab,
- the TUI **Overseer** pane,
- the `OVERSEER` section of `simard status`, and
- the `GET /api/overseer` endpoint (for scripting).

For the data model, file contract, and endpoint schema, see the
[Overseer activity feed reference](../reference/overseer-activity-feed.md).

## Prerequisites

- The Simard **daemon** running (`simard ooda run` or the systemd unit) so the
  Overseer ticks and writes `~/.simard/overseer/activity.json`. See
  [Run the OODA daemon](./run-ooda-daemon.md).
- For the browser view, the dashboard served: `simard dashboard serve --port=8080`
  (see [Dashboard](../dashboard.md)).
- The acting Overseer enabled — which is the **default**. It is off only if you
  explicitly set `SIMARD_OVERSEER_ENABLED` to a falsey value.

> **Fresh install / just merged?** Until the daemon has ticked at least once you
> will see **"Overseer: no ticks recorded yet"**. That is the correct, honest
> state — not a bug.

## Option 1 — the dashboard Overseer tab

1. Open `http://localhost:8080/` and log in.
2. Click the **Overseer** tab in the top navigation.

You will see three things, newest-first, auto-refreshing every ~30 seconds:

- **Status line** — whether the Overseer is enabled, its cadence ("every 15
  min"), the identity it acts under, and when it last ran.
- **Operator threads** — a row per steward thread with its name, whether it is
  enabled, its cadence, last run, next due, and a one-word health
  (`ok` / `idle` / `erroring` / `backoff` / `disabled`).
- **Recent activity** — a timeline of ticks in plain language: what it *saw* and
  what it *did*, or why it *held*.

A healthy, working tab reads something like:

```
Overseer — enabled · every 15 min · acting as simard-overseer[bot] · last run 2 min ago

Operator threads
  overseer                 enabled   every 15 min   last 2 min ago   next in 13 min   ok

Recent activity
  15:30  observed 2 problems → filed 1 issue, launched 1 fix, held 1 (waiting on a gate)   0.8s
  15:15  observed 1 problem  → merged 1 green PR                                            1.2s
  15:00  observing, 0 interventions                                                        0.4s
```

The language stays plain on purpose — the tab is meant to be understandable
without knowing Simard's internals.

## Option 2 — the TUI Overseer pane

1. Start the TUI: `simard-tui`.
2. Press **`Alt+8`** (the footer shows `Alt+1–8`) to open the **Overseer** pane.

It shows the identical status line, operator-thread rows, and recent-activity
timeline as the dashboard tab, in the TUI's layout. See
[`simard-tui`](../reference/simard-tui.md).

## Option 3 — inline with `simard status`

The feed is also a section of the unified status report, so you get it for free
in the CLI, the dashboard **Status** tab, and the TUI **Status** tab:

```console
$ simard status
SIMARD STATUS  ·  2026-07-05T15:31:39Z
…
OVERSEER
  Overseer          enabled · every 15 min · acting as simard-overseer[bot]
  last tick         2026-07-05T15:30:00Z (live)
  threads           overseer: ok (last 15:30, next 15:45)
  recent
    15:30  observed 2 · filed 1 · launched 1 · held 1
    15:15  observed 1 · merged 1
    15:00  observing, 0 interventions
```

See [Read Simard's status](./simard-status.md) for the full report and how
freshness (`live` / `stale` / `absent`) works.

## Option 4 — script it with `GET /api/overseer`

The endpoint is auth-gated behind the **same** credentials as every other
dashboard `/api/*` route — a session cookie or a bearer token. Use the token for
scripts:

```bash
# The HTTP form of the Overseer tab.
curl -fsS -H "Authorization: Bearer $SIMARD_DASHBOARD_TOKEN" \
  http://localhost:8080/api/overseer | jq '.section.freshness'
# → "live"

# How many green PRs has the steward merged across the retained window?
curl -fsS -H "Authorization: Bearer $SIMARD_DASHBOARD_TOKEN" \
  http://localhost:8080/api/overseer | jq '.section.data.totals.prs_merged'
# → 1

# The three most recent ticks, one line each.
curl -fsS -H "Authorization: Bearer $SIMARD_DASHBOARD_TOKEN" \
  http://localhost:8080/api/overseer \
  | jq -r '.section.data.recent[:3][]
      | "\(.timestamp)  obs=\(.report.problems) filed=\(.report.issues_filed) merged=\(.report.prs_merged) held=\(.report.held)"'
```

`recent` is newest-first and capped at 100 ticks; `totals` is summed over the
records currently retained (a rolling window, not an all-time counter). Always
check `.section.availability` / `.section.freshness` before reading
`.section.data`. A request without valid auth returns **401**; a transient
server-side hiccup returns `{"error": …}` at HTTP 200, never a 500.

You can also read the raw durable file directly on the daemon host:

```bash
jq '.enabled, .cadence_secs, (.recent | length)' ~/.simard/overseer/activity.json
```

## Reading the honest states

The feed never shows a blank or misleading panel. If it looks "empty", read the
status line — it is telling you the truth:

| You see | It means | What to do |
|---|---|---|
| `Overseer: disabled` | `SIMARD_OVERSEER_ENABLED` is set falsey — the steward is intentionally off. | Nothing, unless you want it on (below). |
| `Overseer: no ticks recorded yet` | Enabled, but the daemon hasn't completed an Overseer tick yet (fresh start, or just redeployed). | Wait one cadence (default 15 min), or check the daemon is running. |
| `Overseer: enabled, observing, 0 interventions` | Enabled and ticking, but nothing needed doing — it looked and correctly held. | Nothing. "Watching and not acting" is a real, healthy outcome. |
| `Overseer activity feed unavailable` | The feed file couldn't be read (missing/corrupt/permission). | Check `~/.simard/overseer/` perms and daemon logs (`target: "overseer.activity"`). |

**"Observing, 0 interventions" is not the same as "broken".** Most of the time a
healthy steward *watches and waits*; a quiet timeline is usually good news.

## Turn the steward up, down, or off

The feed has no settings of its own — it records whatever the Overseer does, so
you tune it through the existing Overseer configuration
(`src/overseer/config.rs`):

```bash
# Turn the acting Overseer OFF (surfaces then read "Overseer: disabled"):
SIMARD_OVERSEER_ENABLED=0 simard ooda run

# Tighten the cadence to every 5 minutes (clamped to a 60s floor):
SIMARD_OVERSEER_INTERVAL_SECS=300 simard ooda run

# It's ON by default — an unset or truthy value keeps it enabled.
```

The cadence also drives the feed's `live`/`stale` window: a tick is `live` until
`2 × cadence` has passed, then `stale`.

## Troubleshooting

- **Tab shows "no ticks recorded yet" and never changes.** The daemon isn't
  ticking the Overseer. Confirm it's running (`simard status` → DAEMON) and that
  `SIMARD_OVERSEER_ENABLED` isn't falsey. After a fresh deploy, the first tick
  arrives within one cadence.
- **Dashboard tab is blank but `/api/overseer` returns data.** Hard-refresh the
  browser; the panel polls every ~30 s and the tab-switch fetch may have raced a
  restart.
- **`/api/overseer` returns 401.** Your session expired or the bearer token is
  wrong — use the current `SIMARD_DASHBOARD_TOKEN` or re-log-in.
- **Timeline stopped updating (`stale`).** The daemon paused or the write
  failed; the write is non-fatal and logged under `target: "overseer.activity"`.
  Check the daemon log and disk space in `~/.simard`.

## See also

- [Overseer activity feed reference](../reference/overseer-activity-feed.md) —
  data model, file contract, and endpoint schema.
- [Read Simard's status](./simard-status.md) — the full status report.
- [Dashboard](../dashboard.md) and [`simard-tui`](../reference/simard-tui.md) —
  the surfaces this tab/pane live in.
- [Cognitive thread scheduling](../reference/cognitive-thread-scheduling.md) —
  the per-thread health shown in the operator-threads list.
