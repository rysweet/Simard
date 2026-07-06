---
title: "Read Simard's daily journal"
description: >
  A guided first read of the Simard Journal: open the Journal tab, read a day's structured
  report (overview, sections, timestamped moments, verbose prepared-context) and its
  plain-language pull-request table, jump to another date, search across days, glance at a
  quiet-day entry, and peek at the raw stored record — so a newcomer understands what Simard
  did without reading logs.
last_updated: 2026-07-06
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

**Goal of this tutorial:** in about ten minutes, read a day of Simard's work the way a
newcomer would — as a plain-language **report** — using both the dashboard and the terminal.
You will not need to read a single log line, and you will not need to know what "OODA" or
"episodic memory" means going in.

If you want the background first, skim
[The Simard Journal](../concepts/simard-journal.md). Otherwise, just follow along.

!!! warning "Implementation status — target design under issue #2606"
    This walkthrough shows the journal as it is being rebuilt under issue #2606 — a
    third-person report with structured sections, timestamped moments, and a plain-language
    proposal table. Some of what you see here is the intended end state and may not yet
    match the shipped build until the #2606 work lands.

## What you need

- A running Simard daemon with a cognitive-memory store and at least one day of activity
  recorded.
- Access to the operator dashboard (signed in) and, optionally, the `simard-tui` binary.

That's it. Reading the journal needs nothing else configured.

## Step 1 — Open today's entry

Open the dashboard and click the **Journal** tab. The newest day is selected for you, and
you'll see a structured report like this:

> ## 2026-07-06
>
> **Overview.** Simard spent the day improving how it recalls its own past work and
> checking those changes against the live system. Two engineering efforts landed and a
> measurement study began.
>
> ### Engineering work
> A change that adds automated tests to the sign-in code was prepared; after the automated
> checks passed, it was combined into the main code. A second change that keeps you signed
> in a little longer was also combined in.
>
> ### Research
> A study was started to measure how reliably Simard finds its own past work, so future
> changes can be judged against a baseline.
>
> ### Key observations
> Sign-in changes are the most common source of repeated work this week. Older proposals
> tend to stall unless someone nudges them.
>
> ### Decisions
> The Overseer (Simard's steward) flagged a proposal from last week that had gone quiet and
> chose to push it forward rather than close it.
>
> ### Remembered moments
> - **2026-07-06 09:14 UTC** — Started reviewing the sign-in test change.
> - **2026-07-06 11:02 UTC** — Automated checks passed on the sign-in change.
> - **2026-07-06 14:33 UTC** — Combined the sign-in change into the main code.
>
> ### Context prepared for the day
> Simard drew on 10 key facts — chiefly that sign-in changes recur and that the automated
> checks are the gate before combining code — 2 triggers (a stalled proposal and a failing
> check earlier in the week), and 5 procedures it has learned, such as re-running the checks
> before combining a change.

Read it top to bottom. Notice four things:

1. It reads as a **third-person report**, not a personal diary — there is no "Dear diary"
   and no first-person confession.
2. It has a clear **structure**: an overview, then headed sections you can skim.
3. The **remembered moments carry timestamps** and run oldest-to-newest, so you can place
   events in the day.
4. The **prepared-context** summary tells you *what* the day's facts and triggers were, not
   just a count.

## Step 2 — Read the pull-request table

Below the report is the day's **pull-request table** — the "what's in flight today" summary
of the day's open code-change proposals, with three columns:

| PR # | What changed & why it matters | Outcome |
| --- | --- | --- |
| 2606 | A daily written report of Simard's work, so people can tell what it did each day without reading logs. | still open — ready to combine into the main code |
| 2604 | Keeps you signed in longer, so you are not asked to sign in again so often. | still open — automated checks still running |
| 2601 | Measures how well Simard finds its past work, giving a baseline to judge future memory changes against. | still open — not ready yet |

You can skim each change's readiness at a glance and still get a plain-language summary of
what it does and why it matters. This is the same information an engineer would read from
GitHub's open pull-request list — just retold so anyone can follow it. (The table reports the
*open* proposals and how ready each is, rather than lifecycle states like "merged".)

## Step 3 — Jump to another day

Pick an earlier date from the **newest-first** list on the left (or use the date picker).
The report for that day loads: its own overview, sections, moments, and PR table. Days are
always ordered most-recent-first, so scrolling back in time is scrolling down the list.

## Step 4 — Search across days

Type a topic into the **search** box — try `sign-in` or `memory`. The list filters to the
days whose report or PR summaries mention it, newest-first. Search is plain and
case-insensitive; no special syntax. This is how you answer questions like *"when did we
last touch sign-in?"* without opening any code.

## Step 5 — See an honest quiet day

Find a day with little activity (or wait for one). Instead of padding, the entry says so:

> ## 2026-07-04
>
> A quiet day. Nothing of note to report.

Simard would rather tell you a day was quiet than invent a busy one. Both the dashboard and
the TUI render quiet days this way.

## Step 6 — Read the same report in the terminal

If you have the terminal dashboard, launch it and open the Journal pane:

```bash
simard-tui
# press Alt+8 (or Ctrl+8) to open the Journal pane — bare digits are ignored
```

- `↑` / `↓` (or `j`/`k`) move through the newest-first date list.
- `/` starts a search; type a word and the list filters; `Esc` clears it; `r` reloads.
- The selected day's report and its aligned PR table fill the pane.

It's the same entries as the dashboard — the two surfaces read the one shared journal store
and use the one shared renderer — just drawn for a terminal.

## Step 7 (optional) — Peek at the raw stored record

The journal is stored **in Simard's memory**, not as files in a repo. Each day is a single
record keyed by its date. If you have an operator token, you can fetch the structured record
the surfaces are built from:

```bash
curl -s -H "Authorization: ******" \
  http://127.0.0.1:8080/api/journal/entry/2026-07-06
```

```json
{
  "date": "2026-07-06",
  "generated_at": "2026-07-06T18:00:00Z",
  "narrative": "## 2026-07-06\n\n**Overview.** Simard spent the day improving how it recalls ...",
  "draft": "Overview: worked the OODA loop; merged a PR after CI went green; episodic recall ...",
  "prs": [
    { "number": 2606,
      "plain_summary": "A daily written report of Simard's work, so people can tell what it did each day without reading logs.",
      "outcome": "still open — ready to combine into the main code" },
    { "number": 2604,
      "plain_summary": "Keeps you signed in longer, so you are not asked to sign in again so often.",
      "outcome": "still open — automated checks still running" }
  ],
  "quiet_day": false
}
```

Notice both `draft` and `narrative`. The `draft` still contains raw engineering jargon
("OODA loop", "PR", "CI", "episodic recall"); the `narrative` has none of it. The difference
between them is proof the mandatory de-jargon rewrite pass **ran and materially changed the
text** before this entry was stored — it is not a no-op. Each PR row carries a plain-language
`plain_summary` ("what changed & why it matters") and a readiness `outcome`. Because the
record is keyed by date and kept as a single live entry, re-reading later in the day shows an
updated version of *the same* entry, never a pile of duplicates.

## What you learned

- Simard keeps a **daily report** — third-person and structured (overview + sections), not a
  personal diary.
- Entries are **plain-language** (acronyms expanded, raw identifiers and insider terms
  removed, unavoidable terms explained) and pair the report with a three-column
  **pull-request table** of the day's open proposals and how ready each is.
- Remembered moments carry **timestamps** and run in order, and the prepared-context summary
  describes **substance, not counts**.
- You can **browse by date** (newest-first) and **search by text** from both the dashboard
  and the TUI.
- **Quiet days are honest.**
- Entries live **in cognitive memory**, keyed by date, and survive restarts — there are no
  per-day files in the repo.

## Where to go next

- [Browse the Simard Journal](../howto/browse-the-simard-journal.md) — the task reference,
  including configuration.
- [Journal API](../reference/journal-api.md) — the full interface, routes, and security
  guarantees.
- [The Simard Journal](../concepts/simard-journal.md) — the design rationale.
