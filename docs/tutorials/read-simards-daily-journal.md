---
title: "Read Simard's daily journal"
description: >
  A guided first read of the Simard Journal: open the Journal tab, read a day's narrative
  and pull-request table, jump to another date, search across days, glance at a quiet-day
  entry, and peek at the raw stored record — so a newcomer understands what Simard did
  without reading logs.
last_updated: 2026-07-05
review_schedule: as-needed
owner: simard
doc_type: tutorial
related:
  - ../concepts/simard-journal.md
  - ../reference/journal-api.md
  - ../howto/browse-the-simard-journal.md
  - ../dashboard.md
  - ../reference/simard-tui.md
---

# Read Simard's daily journal

**Goal of this tutorial:** in about ten minutes, read a day of Simard's life the way
a newcomer would — as a plain-language diary — using both the dashboard and the
terminal. You will not need to read a single log line, and you will not need to know
what "OODA" or "episodic memory" means going in.

If you want the background first, skim
[The Simard Journal](../concepts/simard-journal.md). Otherwise, just follow along.

## What you need

- A running Simard daemon with a cognitive-memory store and at least one day of
  activity recorded.
- Access to the operator dashboard (signed in) and, optionally, the `simard-tui`
  binary.

That's it. Reading the journal needs nothing else configured.

## Step 1 — Open today's entry

Open the dashboard and click the **Journal** tab. The newest day is selected for
you, and you'll see something like this:

> **2026-07-05**
>
> *Today I concentrated on getting better at remembering my own past work. I ran two
> engineers on the sign-in code. One of them opened a code-change proposal (a "pull
> request", or PR) that added tests; after the automated checks (called "CI") went
> green, I accepted the change (a "merge"). My steward — the Overseer — noticed a
> proposal from last week that had gone quiet and nudged it forward. Along the way I
> added 14 new moment-by-moment memories of what I did. A productive, unhurried day.*

Read it top to bottom. Notice three things:

1. It's written in **first person** — one Simard, one voice.
2. The **Overseer** shows up as *Simard's steward*, inside the same story — not as a
   separate narrator.
3. Every bit of jargon is **explained the first time it appears** ("a pull request",
   "called CI", "a merge"). That's the mandatory review pass doing its job.

## Step 2 — Read the pull-request table

Below the narrative is the day's **pull-request table** — the "what shipped today"
summary:

| PR # | What changed & why it matters | Outcome |
| --- | --- | --- |
| 2606 | Added a daily diary so a human can see what Simard did each day without reading logs. | merged |
| 2604 | Made sign-in remember you a little longer, so you re-log-in less often. | merged |
| 2601 | Started measuring how well Simard finds its own past work. | open |

You can skim outcomes at a glance (merged / open / closed) and still get a
plain-language reason each change matters. This is the same information an engineer
would read from GitHub — just retold so anyone can follow it.

## Step 3 — Jump to another day

Pick an earlier date from the **newest-first** list on the left (or use the date
picker). The entry for that day loads: its own narrative, its own PR table. Days are
always ordered most-recent-first, so scrolling back in time is scrolling down the
list.

## Step 4 — Search across days

Type a topic into the **search** box — try `sign-in` or `memory`. The list filters
to the days whose narrative or PR summaries mention it, newest-first. Search is
plain and case-insensitive; no special syntax. This is how you answer questions like
*"when did we last touch sign-in?"* without opening any code.

## Step 5 — See an honest quiet day

Find a day with little activity (or wait for one). Instead of padding, the entry
says so:

> **2026-07-04**
>
> *A quiet day. Nothing of note to report.*

Simard would rather tell you a day was quiet than invent a busy one. Both the
dashboard and the TUI render quiet days this way.

## Step 6 — Read the same journal in the terminal

If you have the terminal dashboard, launch it and open the Journal pane:

```bash
simard-tui
# press Alt+8 (or Ctrl+8) to open the Journal pane — bare digits are ignored
```

- `↑` / `↓` move through the newest-first date list.
- `/` starts a search; type a word and the list filters.
- The selected day's narrative and PR table fill the pane.

It's the same entries as the dashboard — the two surfaces read the one shared
journal store — just rendered for a terminal.

## Step 7 (optional) — Peek at the raw stored record

The journal is stored **in Simard's memory**, not as files in a repo. Each day is a
single record keyed by its date. If you have an operator token, you can fetch the
structured record the surfaces are built from:

```bash
curl -s -H "Authorization: Bearer $SIMARD_DASHBOARD_TOKEN" \
  http://127.0.0.1:8080/api/journal/entry/2026-07-05
```

```json
{
  "date": "2026-07-05",
  "generated_at": "2026-07-05T18:00:00Z",
  "narrative": "Today I concentrated on getting better at remembering ...",
  "draft": "Dear diary — here is what I, Simard, got up to ...",
  "prs": [
    { "number": 2606, "plain_summary": "Added a daily diary so a human can see ...", "outcome": "merged" },
    { "number": 2604, "plain_summary": "Made sign-in remember you a little longer ...", "outcome": "merged" }
  ],
  "quiet_day": false
}
```

Notice both `draft` (the raw first pass) and `narrative` (the reviewed, jargon-free
prose) — the difference between them is proof the mandatory jargon-review pass ran
before this entry was stored. Because the record is keyed by date and kept as a
single live entry, re-reading later in the day shows an updated version of *the same*
entry, never a pile of duplicates.

## What you learned

- Simard keeps a **daily diary**, written in one voice, with the Overseer as its
  steward.
- Entries are **plain-language** (jargon explained on first use) and pair a
  **narrative** with a **pull-request table**.
- You can **browse by date** (newest-first) and **search by text** from both the
  dashboard and the TUI.
- **Quiet days are honest.**
- Entries live **in cognitive memory**, keyed by date, and survive restarts — there
  are no per-day files in the repo.

## Where to go next

- [Browse the Simard Journal](../howto/browse-the-simard-journal.md) — the task
  reference, including configuration.
- [Journal API](../reference/journal-api.md) — the full interface, routes, and
  security guarantees.
- [The Simard Journal](../concepts/simard-journal.md) — the design rationale.
