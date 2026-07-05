---
title: "The Simard Journal — a daily diary written from memory"
description: >
  Why Simard keeps a daily journal: a diary-style, first-person-steward narrative of
  what the one Brain (including the Overseer) did each day, written largely from episodic
  memories, jargon-scrubbed in a mandatory two-pass review, and stored durably in cognitive
  memory as a first-class journal record — never as per-day files committed to the repo.
last_updated: 2026-07-05
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ../reference/journal-api.md
  - ../howto/browse-the-simard-journal.md
  - ../tutorials/read-simards-daily-journal.md
  - ../architecture/cognitive-memory.md
  - ../reference/cognitive-memory-episodic-recall.md
  - ../reference/cognitive-memory-fact-recall.md
  - ../reference/cognitive-thread-scheduling.md
  - ../design/overseer.md
  - ../dashboard.md
  - ../reference/simard-tui.md
---

# The Simard Journal — a daily diary written from memory

The **Journal** gives Simard a daily diary. Once per day (rolling, regenerated as
the day unfolds) Simard writes a short, plain-language story of what it did — the
goals it worked, the engineers it ran, the pull requests it opened and merged, the
deploys it made, what the **Overseer** steward was up to, how its memory grew, and
any notable events. It reads like a diary entry written by the system about its own
day, in a voice a non-engineer can follow.

The journal exists so a human who was not watching the daemon minute-by-minute can
open one page and understand *what happened today* — without reading logs, without
knowing what "OODA", "episodic recall", or "CI" mean, and without scrolling a wall
of telemetry.

!!! quote "What an entry sounds like"
    *"Today I focused on tightening how I recall my own past work. I ran two
    engineers on the auth module; one opened a code-change proposal (a "pull
    request", or PR) that added tests, and I merged it after the automated checks
    passed. My steward — the Overseer — flagged a stale proposal from last week and
    nudged it along. My memory grew by 14 new moment-by-moment records of what I
    did. A quiet, productive day."*

## The core ideas

### One Brain, one voice

Simard is a single mind. The journal is written in **first person, singular** — "I
worked", "I merged", "I decided" — never as a committee and never as separate
"bridges" or subsystems narrating themselves. The **Overseer** is not a second
narrator; it is Simard's **steward**, and its actions appear *inside* Simard's
story ("my steward flagged…", "the Overseer paused engineer #3 because…"). There is
exactly one point of view.

This matters because the whole point is legibility. A reader should feel they are
reading one coherent diary, not stitching together the outputs of many processes.

### Written largely from episodic memory

Simard already remembers its day as **episodes** — moment-by-moment records of what
it observed and did (see
[Episodic recall](../reference/cognitive-memory-episodic-recall.md) and the
[cognitive-memory architecture](../architecture/cognitive-memory.md)). The journal
is built *primarily* from those episodes: the day's episodes are the raw material,
and the narrative is their retelling in prose.

Episodes are the **primary, required** source. Everything else — the day's goals,
engineer runs, pull requests, deploys, Overseer activity, memory-growth counts,
notable events — is **augmentation**. Each augmentation source is best-effort: if
the daemon cannot supply the day's deploy list, the entry simply omits deploys
rather than inventing them or failing. Episodes carry the story; the rest adds
colour and structure.

### Two passes: draft, then a mandatory jargon review

Generation is deliberately **two-pass**:

1. **Draft.** Assemble the day's context (episodes first, then augmentation) and
   write a first-person narrative plus a pull-request table. The default drafter is
   deterministic template assembly; a language-model drafter is a drop-in swap. This
   pass produces honest but *engineer-flavoured* prose — it may still say "PR", "CI",
   "idempotent", "daemon".
2. **Review / rewrite.** A **mandatory** second pass hands the draft to a
   **reviewer** whose job is to make the text safe for a layperson: it **explains
   jargon on first use** ("a code-change proposal (a "pull request", or PR)"),
   strips corporate jargon, and redacts anything that looks like a secret. The
   generator *always* runs this pass — an entry is never stored un-reviewed.

The review pass is a first-class, structurally-required stage, not an optional
polish. Tests assert both that the reviewer *ran* and that sampled jargon terms are
gone or explained in the stored entry. See
[Journal API — the review pass](../reference/journal-api.md#the-review-pass).

!!! note "Why explain-on-first-use rather than ban words outright"
    Some terms (PR, CI, deploy, merge) are unavoidable when describing engineering
    work. Deleting them would make entries vague. Instead the reviewer keeps the
    term but teaches it once, so the reader learns the vocabulary as they read. The
    existing dashboard `BANNED_JARGON` list targets consultant-speak and insider
    acronyms; the journal adds its own **explain-glossary** for the everyday
    engineering words that list intentionally leaves alone.

### Stored in memory, not in the repo

Journal entries are **not** files committed to the repository. There are no per-day
markdown snapshots checked into git. Instead each entry is a **first-class record in
cognitive memory**, keyed by date:

- One entry per day, stored as a **semantic fact** under the stable key
  `journal:YYYY-MM-DD` (tag `journal`), with the full `JournalEntry` serialized as
  the fact's content.
- Because the store keeps **at most one live fact per key**
  ([`store_fact_with_caller_key`](../reference/cognitive-memory-fact-recall.md)),
  regenerating today's entry **supersedes** the previous version rather than piling
  up duplicates. That is what makes the "rolling, regenerate as the day progresses"
  behaviour safe to repeat.
- Entries **survive restarts** (they live in the durable cognitive-memory store) and
  are **searchable** by date range and by free text.

This reuses the datastore Simard already has. The journal does **not** invent a
parallel database, a new file format, or a second source of truth. It is one more
record type in cognitive memory, sitting alongside facts, episodes, and procedures.

### Browseable by date, searchable by text — everywhere

A single query path over journal records (by **date range** and **text search**,
newest-first) backs both operator surfaces:

- a **Journal tab** in the [operator dashboard](../dashboard.md), and
- a **Journal pane** in the [`simard-tui`](../reference/simard-tui.md) terminal
  dashboard.

Both let the operator pick a date (newest-first list/picker) or search across all
entries, and both render the narrative plus the pull-request table jargon-free and
XSS-safe. The Journal tab is a **distinct, additive** tab with its own `journal`
slug; it does not collide with any existing dashboard tab.

## What a journal entry contains

Every entry is a small, self-describing record:

| Part | What it is |
| --- | --- |
| **Date** | The calendar day (UTC) the entry describes; also its key. |
| **Narrative** | The first-person diary story, jargon-scrubbed. |
| **PR table** | Every pull request of the day: number, a plain-language "what changed & why it matters", and the outcome (merged / open / closed). |
| **Quiet-day flag** | If nothing meaningful happened, the entry says so honestly ("a quiet day") rather than fabricating activity. |
| **Reviewed flag** | Records that the mandatory jargon-review pass ran. |

The pull-request table is the backbone of the "what shipped today" story. It is
described in prose *and* rendered as a table so a non-engineer can skim outcomes at
a glance. See the exact shape in the
[Journal API reference](../reference/journal-api.md#data-model).

## Honesty on quiet days

Not every day is busy. When the day's episodes and augmentation sources are empty,
Simard does **not** pad the entry with filler. It writes a short, honest
placeholder — *"A quiet day. Nothing of note to report."* — and marks the entry as
quiet. Both the dashboard and the TUI render that honestly. Truthfulness beats
volume; a fabricated busy day would be worse than an empty one. This mirrors
Simard's broader [truthful-runtime-metadata](./truthful-runtime-metadata.md)
principle.

## How it fits the rest of Simard

The journal is a thin narrative layer over subsystems that already exist:

- **Cognitive memory** supplies the episodes (the story) and stores the finished
  entry (the record). No new datastore.
- **The OODA daemon** already knows the day's goals, engineer runs, deploys, and
  Overseer activity; the journal reads those as augmentation.
- **A daily background task** in the OODA daemon regenerates today's entry on a fixed
  cadence (default hourly), running *after* the decision cycle so it never competes
  with it — narrating the day is a lightweight form of memory consolidation. It runs
  by default and can be turned off with a single environment variable.
- **The dashboard and TUI** already have a tab/pane pattern, styling, and an
  escaping discipline; the Journal surfaces slot into those.

Nothing here changes the live daemon's decision-making. The journal only *reads*
what Simard did and *writes* a human-readable record of it.

## Where to go next

- **Use it:** [Browse the Simard Journal](../howto/browse-the-simard-journal.md).
- **First walkthrough:** [Read Simard's daily journal](../tutorials/read-simards-daily-journal.md).
- **Build/extend it:** [Journal API reference](../reference/journal-api.md).
- **Background:** [Episodic recall](../reference/cognitive-memory-episodic-recall.md),
  [the Overseer steward](../design/overseer.md).
