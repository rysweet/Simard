---
title: "Browse the Simard Journal"
description: >
  How an operator reads Simard's daily journal report: open the Journal tab in the dashboard
  or the Journal pane in simard-tui, browse entries newest-first by date, search across all
  entries by text, read the structured report (overview, sections, timestamped moments) and
  the plain-language pull-request table, and configure or disable the daily generation thread.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/simard-journal.md
  - ../reference/journal-api.md
  - ../tutorials/read-simards-daily-journal.md
  - ../dashboard.md
  - ../reference/simard-tui.md
  - ./monitor-simard-with-tui.md
  - ../reference/state-root-resolution.md
  - ./configure-cognitive-thread-scheduling.md
---

# Browse the Simard Journal

This guide shows how to read Simard's daily **narrative engineering & research report** —
the **Journal** — from either operator surface, how to find a specific day, how to search
across days, and how to control when entries are generated.

For what the journal *is*, see [The Simard Journal](../concepts/simard-journal.md). For the
interface details, see the [Journal API reference](../reference/journal-api.md).

!!! warning "Implementation status — target design under issue #2606"
    This guide describes the journal as it is being rebuilt under issue #2606 (a third-person
    report with structured sections, timestamped moments, and a plain-language proposal
    table). Some of what it describes is the intended end state and may not yet match the
    shipped build until the #2606 work lands.

## Prerequisites

- A Simard daemon whose cognitive-memory store is reachable (the same store used by the
  dashboard and TUI). Reading entries needs only the store — not a running generation
  thread.
- For the dashboard: a valid operator session/token (the Journal routes are auth-gated).
- For the TUI: the `simard-tui` binary and an interactive terminal (see
  [Monitor Simard with the TUI](./monitor-simard-with-tui.md)).

## What an entry looks like

Each entry is a **report**, not a personal diary. It is laid out for skimming:

- a short **Overview** paragraph — the "what happened today" in two or three sentences;
- clearly headed **sections** — *Engineering work*, *Research*, *Key observations*,
  *Decisions*, *Outcomes* (only the ones with content appear);
- the day's **remembered moments**, each shown **with a timestamp** and in the order they
  happened;
- a verbose **prepared-context** summary that describes *what* the day's key facts,
  triggers, and procedures were (not just how many); and
- a **pull-request table** with three columns — **PR # / What changed & why it matters /
  Outcome** — one row per open code-change proposal, whose outcome is a plain-language
  readiness phrase ("ready to combine into the main code", "automated checks still running",
  or "not ready yet"), written so a non-engineer understands each one.

Everything is rendered **jargon-free** (acronyms expanded, raw code identifiers and insider
terms removed, unavoidable terms explained in plain words) and **XSS-safe** (entry text is
escaped, never executed).

## Read the journal in the dashboard

1. Open the [operator dashboard](../dashboard.md) and sign in.
2. Click the **Journal** tab (or navigate to `#journal`). It is a distinct tab, separate
   from the Operator/Overseer-activity view.
3. The tab shows a **date list, newest-first**. The most recent day is selected by default
   and its entry renders on the right: the structured report (overview + sections +
   timestamped moments) followed by the three-column pull-request table.

### Browse by date

- Pick any date from the newest-first list (or the date picker) to load that day's entry.
- A day with no activity shows an honest **"quiet day"** entry rather than a blank page or
  invented activity.

### Search across all days

- Type into the **search** box to filter entries by text. The search runs over the report
  narrative and the PR summaries and returns matching days newest-first.
- Search is a plain, case-insensitive keyword match — no special syntax needed. Queries are
  length-bounded and results are capped; you will always get the newest matches first.

### What the routes are (optional)

The tab is backed by read-only, auth-gated endpoints — handy for scripting or debugging:

```bash
# List days that have an entry (newest-first).
curl -s -H "Authorization: ******" \
  http://127.0.0.1:8080/api/journal/dates

# Fetch one day's structured entry.
curl -s -H "Authorization: ******" \
  http://127.0.0.1:8080/api/journal/entry/2026-07-06

# Full-text search (POST a JSON body; optional from/to date bounds).
curl -s -H "Authorization: ******" \
  -H 'Content-Type: application/json' \
  -X POST http://127.0.0.1:8080/api/journal/search \
  -d '{"query":"sign-in"}'

# Server-rendered, HTML-escaped entry (what the tab embeds) — report headings + PR table.
curl -s -H "Authorization: ******" \
  http://127.0.0.1:8080/api/journal/render/2026-07-06
```

(Replace host/port with your dashboard's. See the
[Journal API reference](../reference/journal-api.md#http-routes) for the exact shapes.)

## Read the journal in the TUI

1. Launch the terminal dashboard:

   ```bash
   simard-tui
   ```

2. Press **Alt+8** (or **Ctrl+8**, or cycle with **Tab** / **Shift+Tab** / **←** **→**) to
   open the **Journal** pane. The TUI ignores bare digit keys, so plain `8` does nothing.
3. The pane shows a **newest-first date list** on one side and the selected entry — the same
   report (headings + aligned three-column PR table) — on the other.
   - Use **`↑` / `↓`** (or **j/k**) to move through days.
   - Press **`/`** to enter search, type a needle, and filter entries by text; **Esc**
     clears it; **r** reloads.
   - A day with no entry renders the same honest **"quiet day"** placeholder.

The TUI reads the same entries as the dashboard through the shared journal store and renders
them with the same shared renderer, so the two surfaces always agree.

## Configure or disable daily generation

Entries are (re)generated by a **daily journal task** that runs inside the daemon and
rewrites *today's* entry on a cadence. It runs **by default** (after the authoritative
decision cycle, so it never stalls it). You control it with environment variables (see
[Journal API — configuration](../reference/journal-api.md#configuration)):

| Variable | Default | Effect |
| --- | --- | --- |
| `SIMARD_JOURNAL_ENABLED` | `1` (on) | The daily journal runs by default. Set to a falsey value (`0`/`false`/`no`/`off`) to stop generating new entries; existing entries stay browseable. |
| `SIMARD_JOURNAL_INTERVAL_SECS` | `3600` | How often today's entry is regenerated (clamped to a 60-second floor). |

Set these where the daemon reads its environment, then restart the daemon so it picks up the
change. Regeneration is idempotent — running more often updates the single live entry for
the day; it never creates duplicate days.

!!! note "Reading needs no configuration"
    Disabling the task only stops *new* entries. The dashboard tab and TUI pane keep working
    against whatever entries already exist in memory.

!!! note "LLM-written vs. offline reports"
    When `recipe-runner-rs` and the journal recipes are available, the report is written and
    de-jargoned by prompts (richer prose). When they are not — for example in a minimal
    deployment — generation falls back to a deterministic offline pipeline that still
    produces the same structured, jargon-free report. Either way the layout is identical.

## Troubleshoot

- **The Journal tab/pane is empty.** The store may have no entries yet (a fresh daemon, or
  generation disabled). Confirm `SIMARD_JOURNAL_ENABLED` is not set to a falsey value, then
  wait one interval (default 1 hour) or restart the daemon.
- **A specific day is missing.** Only days with recorded activity get a full entry; quiet
  days still render a placeholder. Use search to confirm the day exists.
- **`/api/journal/*` returns `{"status":"error"}` with HTTP 200.** That is the dashboard
  convention — check the `error` field. A malformed date (`{date}` must be `YYYY-MM-DD`) or
  an over-long search query are the usual causes.
- **Entries look stale.** Today's entry is rolling; it updates each interval. Lower
  `SIMARD_JOURNAL_INTERVAL_SECS` if you want fresher regeneration, and restart the daemon.
- **An entry still reads like jargon.** File it as a bug — the de-jargon pass is a
  hard guarantee, and its banned-token list is covered by tests. Include the date so the
  offending narrative can be reproduced.

## Related

- [The Simard Journal](../concepts/simard-journal.md) — concept.
- [Journal API](../reference/journal-api.md) — routes, config, data model.
- [Read Simard's daily journal](../tutorials/read-simards-daily-journal.md) — guided first
  read.
- [Monitor Simard with the TUI](./monitor-simard-with-tui.md) — TUI basics.
