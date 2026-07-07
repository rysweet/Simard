---
title: "The Simard Journal — a daily narrative engineering & research report"
description: >
  Why Simard keeps a daily journal: a professional, third-person NARRATIVE REPORT of the
  day's engineering and research — an overview paragraph, clearly delineated sections, a
  plain-language pull-request summary table, timestamped remembered moments, and a verbose
  "what context was prepared" summary. Built largely from episodic memories, rewritten into
  genuinely jargon-free language by a mandatory two-pass (draft then de-jargon) pipeline,
  and stored durably in cognitive memory — never as per-day files committed to the repo.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ../reference/journal-api.md
  - ../reference/journal-narrative-result-channel.md
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

# The Simard Journal — a daily narrative engineering & research report

The **Journal** gives Simard a daily written record of its work. Once per day (rolling,
regenerated as the day unfolds) Simard produces a short, plain-language **narrative
report** of what happened — the goals worked, the engineering built or investigated,
the research conducted, the pull requests worked on, the updates shipped to the
live system, what the **Overseer** steward did, how memory grew, and the day's key
observations, decisions, and outcomes.

It is written as a **professional, third-person report**, the way an engineering team
might summarise a day for a mixed audience — factual, structured, and readable. It is
**not** a personal diary: there is no "Dear diary", no first-person confessional voice,
and no diary framing. The reader gets a clear account of *what was worked on, what was
built or investigated, what was observed, what was decided, and how it turned out.*

The journal exists so a human who was not watching the daemon minute-by-minute can open
one page and understand *what happened today* — without reading logs, without knowing
what "OODA", "episodic recall", or "CI" mean, and without scrolling a wall of telemetry.

!!! warning "Implementation status — target design under issue #2606"
    This page describes the Simard Journal as it is being **rebuilt** under issue #2606: a
    third-person narrative report, structured sections, timestamped moments, a verbose
    prepared-context summary, and a prompt-first generation pipeline. Parts of this are the
    intended end state and are **not yet reflected in the shipped code** — the pre-fix #2618
    build still writes a first-person "Dear diary" narrative. Read this as the design we are
    building toward, not as a description of current behaviour.

!!! quote "What an entry sounds like"
    *"**Overview.** Simard spent the day improving how it recalls its own past work and
    validating those changes against the live system. Two engineering efforts landed and
    a measurement study was started."*

    *"**Engineering work.** A change that adds automated tests to the sign-in code was
    prepared and, once the automated checks passed, combined into the main code…"*

    *(followed by Research, Key observations, Decisions and Outcomes sections, then a
    plain-language table summarising each pull request.)*

## The core ideas

### A narrative report, not a personal diary

Simard is a single system, and the journal is written about that system's day in the
**third person** — "Simard prepared…", "the change was combined into the main code…",
"the Overseer flagged a stale proposal…". There is no diary voice and no personal
framing; the register is that of a clear engineering/research report a non-engineer can
still follow.

The **Overseer** is Simard's **steward**, and its actions appear *inside* the report
("the Overseer flagged a stale proposal and nudged it forward") rather than as a second
narrator. There is one coherent account, not a stitched-together set of subsystem logs.

This matters because the whole point is legibility. A reader should feel they are reading
one well-structured report, not a private diary and not raw telemetry.

### Structured: overview, sections, and a PR table

Every entry has a **deliberate, readable structure** rather than a wall of prose:

1. A short **Overview** paragraph — the two-or-three-sentence "what happened today".
2. Clearly delineated **sections**, each under its own heading, covering (as applicable):
   **Engineering work**, **Research**, **Key observations**, **Decisions**, and
   **Outcomes**. Empty sections are simply omitted (honest degradation) rather than
   padded.
3. A **pull-request summary table** — a markdown table with one row per open code-change
   proposal: its **number**, a plain-language **"what changed & why it matters"** summary,
   and a plain-language **outcome** — a readiness phrase for the open proposal ("ready to
   combine into the main code", "automated checks still running", or "not ready yet"). Each
   row is written so a layperson understands what the change was for. (The journal reflects
   the *open* proposal readiness view, so it reports readiness rather than lifecycle states
   like "merged" or "closed".)

The same structure renders cleanly in **both** operator surfaces — the dashboard Journal
tab and the TUI Journal pane — because both are produced by one shared renderer (see
[Journal API — rendering](../reference/journal-api.md#rendering-the-shared-renderer)).

### Built largely from timestamped episodic memory

Simard already remembers its day as **episodes** — moment-by-moment records of what it
observed and did (see
[Episodic recall](../reference/cognitive-memory-episodic-recall.md) and the
[cognitive-memory architecture](../architecture/cognitive-memory.md)). The report is
built *primarily* from those episodes: the day's episodes are the raw material, and the
narrative is their retelling in report form.

Two properties make the episodic material legible:

- **Timestamps.** Each remembered moment referenced in the report shows **when it
  occurred** (a human-readable time label), so the reader can place events in the day —
  not a bare, undated list.
- **Chronological order.** Remembered moments are presented **oldest-to-newest**, so the
  report reads as the day actually unfolded.

Episodes are the **primary, required** source. Everything else — goals, engineering runs,
pull requests, deploys, Overseer activity, memory-growth counts, notable events, and the
prepared-context summary — is **augmentation**. Each augmentation is best-effort: if the
daemon cannot supply the day's deploy list, the entry simply omits deploys rather than
inventing them or failing.

### A verbose "prepared context" summary — substance, not counts

Earlier journal output emitted a bare line of totals — *"Prepared context: 10 facts, 2
triggers, 5 procedures, 5 remembered moments"* — which told the reader nothing useful.
The report now summarises **what those items actually were**: a brief, readable
description of the day's key **facts**, **triggers**, **procedures**, and **remembered
moments**, so the reader learns the substance rather than a total. Counts alone are never
the whole story.

### Two passes: draft, then a mandatory de-jargon rewrite that has teeth

Generation is deliberately **two-pass** so the jargon-free guarantee is structural, not
incidental:

1. **Draft.** Assemble the day's context (episodes first, then augmentation) and write
   the structured, third-person report plus the pull-request table.
2. **De-jargon rewrite.** A **mandatory** second pass rewrites the draft into language a
   non-engineer can genuinely read: it **expands acronyms** ("PR" → "pull request", "CI"
   → "the automated checks"), **removes raw code identifiers, internal code names, and
   insider terms**, and **explains any unavoidable technical term in plain words**.

Finally — regardless of whether that rewrite ran as a prompt or as the offline glossary
pass — a **mandatory secret-redaction step** runs over the reviewed text, so anything that
looks like a token, key, or private-key block is scrubbed before the entry is stored. The
LLM reviewer's output is never trusted to be secret-free on its own.

The rewrite pass is **effective by design, not a no-op**. The generator *always* runs it,
it **materially changes** the text, and the tests prove it: an entry retains both its
pre-review `draft` and its final `narrative`, and the test suite asserts (a) that a
representative list of banned jargon tokens does **not** appear in the final narrative and
(b) that the narrative genuinely **differs** from the draft. See
[Journal API — the de-jargon pass](../reference/journal-api.md#the-de-jargon-rewrite-pass).

!!! note "Why the earlier de-jargon step was strengthened"
    #2618 added a de-jargon review step, but in production the final entries were still
    full of jargon — the pass was effectively a no-op. #2606 gives it teeth: a
    prompt-first rewrite (below) plus a strengthened deterministic glossary, verified by
    tests that fail if banned tokens survive into the narrative.

### Prompt-first generation, with an honest offline fallback

Following the project's *agentic-over-brittle-parsing* guideline (G3), the report is
written and de-jargoned by **prompts**, not by fragile string manipulation:

- **Primary (prompt-first).** A recipe-runner-backed **drafter** and **de-jargon
  reviewer** run the `journal-narrative` and `journal-plain-language`
  [recipes](../reference/journal-api.md#the-generation-recipes). The prompts own the
  narrative-report shaping and the plain-language rewrite. Recipe assets **hot-reload**
  from `~/.simard/prompt_assets/simard/…` so a deploy can update them without a rebuild.
- **Fallback (offline).** When `recipe-runner-rs` (or a recipe asset) is unavailable —
  as in the hermetic test suite — generation degrades **honestly** to a deterministic
  template drafter plus a strengthened glossary reviewer that still produce a structured,
  jargon-free report. No network, no LLM, still readable.

The generator that a deployment uses is selected by
`JournalGenerator::for_repo(repo_root)`, which prefers the prompt-first pipeline and falls
back to the deterministic one — so the feature always works, just with an LLM ceiling when
one is available.

### Stored in memory, not in the repo

Journal entries are **not** files committed to the repository. There are no per-day
markdown snapshots checked into git. Instead each entry is a **first-class record in
cognitive memory**, keyed by date:

- One entry per day, stored as a **semantic fact** under the stable key
  `journal:YYYY-MM-DD` (tag `journal`), with the full `JournalEntry` serialized as the
  fact's content.
- Because the store keeps **at most one live fact per key**
  ([`store_fact_with_caller_key`](../reference/cognitive-memory-fact-recall.md)),
  regenerating today's entry **supersedes** the previous version rather than piling up
  duplicates. That is what makes the "rolling, regenerate as the day progresses"
  behaviour safe to repeat.
- Entries **survive restarts** (they live in the durable cognitive-memory store) and are
  **searchable** by date range and by free text.

This reuses the datastore Simard already has. The journal does **not** invent a parallel
database, a new file format, or a second source of truth.

### Browseable by date, searchable by text — everywhere

A single query path over journal records (by **date range** and **text search**,
newest-first) backs both operator surfaces:

- a **Journal tab** in the [operator dashboard](../dashboard.md), and
- a **Journal pane** in the [`simard-tui`](../reference/simard-tui.md) terminal
  dashboard.

Both let the operator pick a date (newest-first list/picker) or search across all entries,
and both render the structured report plus the pull-request table jargon-free and XSS-safe.
This searchable, browse-by-date behaviour is **preserved unchanged** from #2618.

## What a journal entry contains

Every entry is a small, self-describing record:

| Part | What it is |
| --- | --- |
| **Date** | The calendar day (UTC) the entry describes; also its key. |
| **Narrative** | The final, reviewed, jargon-free **report**: an overview paragraph, sectioned narrative, timestamped chronological remembered moments, and the verbose prepared-context summary. |
| **PR table** | Every open code-change proposal of the day: number, a plain-language "what changed & why it matters" summary, and a plain-language readiness outcome. |
| **Draft** | The pre-review draft, retained so the de-jargon pass is provably not a no-op (`draft` ≠ `narrative`). |
| **Quiet-day flag** | If nothing meaningful happened, the entry says so honestly ("a quiet day") rather than fabricating activity. |

The pull-request table is the backbone of the "what shipped today" story. It is described
in prose *and* rendered as a table so a non-engineer can skim outcomes at a glance. See
the exact shape in the [Journal API reference](../reference/journal-api.md#data-model).

## Honesty on quiet days

Not every day is busy. When the day's episodes and augmentation sources are empty, Simard
does **not** pad the entry with filler. It writes a short, honest placeholder — *"A quiet
day. Nothing of note to report."* — and marks the entry as quiet. Both the dashboard and
the TUI render that honestly. Truthfulness beats volume; a fabricated busy day would be
worse than an empty one. This mirrors Simard's broader
[truthful-runtime-metadata](./truthful-runtime-metadata.md) principle.

## How it fits the rest of Simard

The journal is a thin narrative layer over subsystems that already exist:

- **Cognitive memory** supplies the timestamped episodes (the story) and stores the
  finished entry (the record). No new datastore.
- **The OODA daemon** already knows the day's goals, engineer runs, deploys, and Overseer
  activity; the journal reads those as augmentation.
- **The prompt-first recipes** (`journal-narrative`, `journal-plain-language`) own the
  report shaping and the de-jargon rewrite; the deterministic pipeline is the offline
  fallback.
- **A daily background task** in the OODA daemon regenerates today's entry on a fixed
  cadence (default hourly), running *after* the decision cycle so it never competes with
  it. It runs by default and can be turned off with a single environment variable.
- **The dashboard and TUI** already have a tab/pane pattern, styling, and an escaping
  discipline; the Journal surfaces slot into those and share one renderer.

Nothing here changes the live daemon's decision-making. The journal only *reads* what
Simard did and *writes* a human-readable report of it.

## Where to go next

- **Use it:** [Browse the Simard Journal](../howto/browse-the-simard-journal.md).
- **First walkthrough:** [Read Simard's daily journal](../tutorials/read-simards-daily-journal.md).
- **Build/extend it:** [Journal API reference](../reference/journal-api.md).
- **Background:** [Episodic recall](../reference/cognitive-memory-episodic-recall.md),
  [the Overseer steward](../design/overseer.md).
