---
title: "Journal API"
description: >
  Reference for the src/journal module: the JournalEntry / PrSummary data model, the
  journal:YYYY-MM-DD key scheme layered over cognitive memory, the injectable two-pass
  generation pipeline (clock, episode & PR sources, drafter, mandatory glossary reviewer),
  the JournalStore query API (by date range + full text, newest-first), the daily journal
  thread, the dashboard HTTP routes and Journal tab, the simard-tui Journal pane,
  configuration environment variables, security guarantees, and telemetry.
last_updated: 2026-07-05
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/simard-journal.md
  - ../howto/browse-the-simard-journal.md
  - ../tutorials/read-simards-daily-journal.md
  - ./cognitive-memory-fact-recall.md
  - ./cognitive-memory-episodic-recall.md
  - ./cognitive-thread-scheduling.md
  - ./cognitive-memory-procedural-idempotency.md
  - ./operator-read-state-root-contract.md
  - ../dashboard.md
  - ./simard-tui.md
  - ./telemetry-metrics.md
  - ../design/overseer.md
---

# Journal API

The **journal** module (`src/journal/`) turns Simard's day into a durable,
searchable, layperson-readable diary entry stored in cognitive memory. This page is
the interface contract: types, traits, the storage-key scheme, the query API, the
two-pass generation pipeline, the operator surfaces, configuration, and the security
and telemetry guarantees.

For the *why*, read [The Simard Journal](../concepts/simard-journal.md). For
day-to-day operator use, read
[Browse the Simard Journal](../howto/browse-the-simard-journal.md).

## Module layout

```text
src/journal/
├── mod.rs         // pub re-exports; `pub mod journal;` is declared in src/lib.rs
├── types.rs       // JournalEntry, PrSummary, MemoryGrowth, DayContext (+ serde)
├── jargon.rs      // JOURNAL_GLOSSARY + scrub_jargon (the default reviewer body)
├── generate.rs    // JournalDrafter / JournalReviewer traits, TemplateDrafter,
│                  //   GlossaryReviewer, JournalGenerator (draft -> ALWAYS review)
├── providers.rs   // JournalClock / EpisodeSource / PrListSource seams, DayExtras,
│                  //   assemble_day_context, generate_and_store[_ops]
├── render.rs      // html_escape, render_entry_html, render_entry_tui_lines
├── store.rs       // JournalStore + borrowed free fns over &dyn CognitiveMemoryOps
└── thread.rs      // run_journal_tick + enable/interval env helpers (daily task)
```

The module is a self-contained brick. Its only hard dependency is the existing
[`CognitiveMemoryOps`](./cognitive-memory-fact-recall.md) trait; everything the
generator needs from the outside world is injected behind a small trait, which is
what makes the whole pipeline testable with no network, no clock, and no live daemon.

## Data model

Defined in `src/journal/types.rs`; both records are `serde` (de)serializable.

```rust
/// A plain-language summary of one code-change proposal (pull request).
pub struct PrSummary {
    pub number: u64,          // rendered as `#123`
    pub plain_summary: String, // "what changed & why it matters", no jargon
    pub outcome: String,       // "merged", "open", "closed", ...
}

/// A single day's diary entry — the durable, persisted record.
pub struct JournalEntry {
    pub date: chrono::NaiveDate,             // UTC day; also the storage key
    pub generated_at: chrono::DateTime<chrono::Utc>, // latest (re)generation time
    pub narrative: String,                   // final, reviewed, jargon-free prose
    pub draft: String,                       // pre-review draft (provenance)
    pub prs: Vec<PrSummary>,                 // backs the plain-language PR table
    pub quiet_day: bool,                     // rendered honestly, never fabricated
}
```

`JournalEntry::merged_pr_count()` returns how many of the day's proposals were
merged. `MemoryGrowth { facts_added, episodes_added }` carries the day's memory
growth when measured.

The transient generation input is `DayContext`:

```rust
pub struct DayContext {
    pub date: chrono::NaiveDate,
    pub episodes: Vec<CognitiveEpisode>, // PRIMARY narrative source
    pub prs: Vec<PrSummary>,
    pub goals: Vec<String>,
    pub deploys: Vec<String>,
    pub overseer_events: Vec<String>,
    pub memory_growth: Option<MemoryGrowth>,
    pub notable: Vec<String>,
}
```

`DayContext::is_quiet()` is `true` when nothing happened worth narrating; the
drafter then emits an honest "quiet day" paragraph rather than an empty or invented
one. Episodics are the primary source; every other field is a best-effort
augmentation that is simply omitted when absent (honest degradation).

## The two-pass generation pipeline

Generation is deliberately **two-pass** so the jargon-free guarantee is structural,
not incidental (see the concept's
[two-pass review](../concepts/simard-journal.md#two-passes-draft-then-a-mandatory-jargon-review)):

```rust
pub trait JournalDrafter: Send + Sync { fn draft(&self, day: &DayContext) -> String; }
pub trait JournalReviewer: Send + Sync { fn review(&self, draft: &str) -> String; }

pub struct JournalGenerator { /* boxed drafter + reviewer */ }
impl JournalGenerator {
    pub fn new(drafter: Box<dyn JournalDrafter>, reviewer: Box<dyn JournalReviewer>) -> Self;
    pub fn default_pipeline() -> Self;                 // TemplateDrafter + GlossaryReviewer
    pub fn generate(&self, day: &DayContext) -> JournalEntry; // draft -> ALWAYS review
}
```

`generate` runs the drafter, then **always** runs the reviewer over the draft, and
stores both `draft` (provenance) and the reviewed `narrative` — there is no code path
that returns an unreviewed narrative.

### The review pass

The mandatory second pass is what makes the entry layperson-readable:

- **`TemplateDrafter`** (default) is deterministic and offline: it leads with the
  day's episodics, folds in goals / live-system updates / Overseer activity / memory
  growth / notable events, then a one-line lead-in to the proposal table.
- **`GlossaryReviewer`** (default) delegates to `scrub_jargon`, a whole-word,
  case-insensitive glossary substitution (`JOURNAL_GLOSSARY`) that *removes or
  explains* engineering terms — e.g. `PR` → "code-change proposal (PR)", `episodic`
  → "moment-by-moment", `deploy` → "ship to the live system". An LLM reasoner can be
  swapped in behind either trait.

### Injectable seams (`providers.rs`)

```rust
pub trait JournalClock: Send + Sync { fn today(&self) -> NaiveDate; }     // SystemClock = UTC
pub trait EpisodeSource: Send + Sync { fn episodes_for_date(&self, d: NaiveDate) -> SimardResult<Vec<CognitiveEpisode>>; }
pub trait PrListSource: Send + Sync { fn prs_for_date(&self, d: NaiveDate) -> SimardResult<Vec<PrSummary>>; }

pub struct DayExtras { pub goals, deploys, overseer_events, notable: Vec<String>, pub memory_growth: Option<MemoryGrowth> }

pub fn assemble_day_context(date, episodes: &dyn EpisodeSource, prs: &dyn PrListSource, extras: DayExtras) -> SimardResult<DayContext>;
pub fn generate_and_store(date, episodes, prs, extras, generator: &JournalGenerator, store: &JournalStore) -> SimardResult<JournalEntry>;
pub fn generate_and_store_ops(date, episodes, prs, extras, generator: &JournalGenerator, mem: &dyn CognitiveMemoryOps) -> SimardResult<JournalEntry>;
```

## Storage & the query API (`store.rs`)

Journal entries live in the **same** cognitive-memory store as the rest of Simard's
knowledge — there is no parallel datastore.

| Property | Value |
| --- | --- |
| Caller key / concept | `journal:YYYY-MM-DD` (UTC date, `%Y-%m-%d`) — `journal_caller_key(date)` |
| Content | the JSON-serialized `JournalEntry` |
| Tag | `journal` (`JOURNAL_TAG`) |
| Write path | [`store_fact_with_caller_key`](./cognitive-memory-fact-recall.md) — "at most one live fact per key", so regenerating a day **supersedes** the prior entry (idempotent rolling update) |

Two equivalent surfaces read/write the same records:

```rust
// Owned handle (holds an Arc<dyn CognitiveMemoryOps>):
pub struct JournalStore { /* Arc<dyn CognitiveMemoryOps> */ }
impl JournalStore {
    pub fn new(mem: Arc<dyn CognitiveMemoryOps>) -> Self;
    pub fn save(&self, entry: &JournalEntry) -> SimardResult<String>;
    pub fn get_by_date(&self, date: NaiveDate) -> SimardResult<Option<JournalEntry>>;
    pub fn all_entries(&self) -> SimardResult<Vec<JournalEntry>>;    // newest day first
    pub fn dates(&self) -> SimardResult<Vec<NaiveDate>>;             // newest first
    pub fn query(&self, range: Option<(NaiveDate, NaiveDate)>, text: Option<&str>)
        -> SimardResult<Vec<JournalEntry>>;                         // date range + full text
}

// Borrowed free functions (for callers holding only a &dyn CognitiveMemoryOps —
// the dashboard reader bridge and the daily thread). JournalStore delegates to these.
pub fn save_entry(mem: &dyn CognitiveMemoryOps, entry: &JournalEntry) -> SimardResult<String>;
pub fn get_entry_by_date(mem: &dyn CognitiveMemoryOps, date: NaiveDate) -> SimardResult<Option<JournalEntry>>;
pub fn all_entries(mem: &dyn CognitiveMemoryOps) -> SimardResult<Vec<JournalEntry>>;
pub fn query_entries(mem: &dyn CognitiveMemoryOps, range, text) -> SimardResult<Vec<JournalEntry>>;
pub fn entry_matches(entry: &JournalEntry, query: &str) -> bool;    // same rule the TUI filters with
```

`query` filters by an inclusive date range and/or a case-insensitive substring over
the narrative, the date, and each proposal's summary/outcome/number — newest day
first. This single path backs both the dashboard and the TUI, so they never diverge.

Enumeration is lenient (a non-journal or unparseable candidate fact is skipped), but a
`journal:`-keyed fact whose JSON is corrupt fails **loud** as
`SimardError::InvalidJournalRecord` on an exact `get_by_date`.

## The daily journal thread (`thread.rs`)

The daemon regenerates *today's* entry on a slow cadence — a rolling update that keeps
the day's entry current as remembered moments accumulate.

```rust
pub fn run_journal_tick(mem: &dyn CognitiveMemoryOps, clock: &dyn JournalClock)
    -> SimardResult<JournalEntry>;
pub fn journal_enabled() -> bool;        // default true (opt-out)
pub fn journal_interval_secs() -> u64;   // default 3600, floor 60
```

`run_journal_tick` reads the day's episodics from the store (primary source), folds in
the active goals (best-effort augmentation), generates the reviewed entry with the
default pipeline, and persists it via `save_entry`. It is **pure and offline** — it
never touches the network — so it runs safely inside the daemon loop. The in-daemon PR
source is offline and degrades honestly to an empty list; the plain-language proposal
table is exercised whenever a richer `PrListSource` is injected (tests, future
adapters).

Wiring: the OODA daemon runs the tick default-on, interval-gated, and panic-isolated,
**after** the authoritative OODA cycle so it can never stall or crash the loop. It
fires on the first iteration so a fresh daemon writes the day's entry immediately.

## Dashboard: HTTP routes and the Journal tab

The [operator dashboard](../dashboard.md) gains a **Journal** tab — a distinct,
additive tab whose `journal` slug and `/api/journal/*` route namespace do not collide
with any other tab (including the in-flight Operator/Overseer activity tab).

### HTTP routes

All routes are read-only, registered before `require_auth`, and reuse the existing
session auth. There are no write/delete endpoints — entries are produced only
in-process by the daily thread.

| Route | Response | Notes |
| --- | --- | --- |
| `GET /api/journal/dates` | `{ "dates": [ {date, quiet_day, pr_count, merged}, … ] }` | Newest day first, for the date picker. |
| `POST /api/journal/search` | `{ "results": [ {date, quiet_day, pr_count, merged, snippet}, … ] }` | Body `{query?, from?, to?}` (dates `YYYY-MM-DD`); newest first. |
| `GET /api/journal/entry/{date}` | `JournalEntry` JSON, or `{status:"error", error}` | `{date}` strictly parsed `%Y-%m-%d`. |
| `GET /api/journal/render/{date}` | `text/html` (server-rendered fragment) | Narrative + PR table, fully HTML-escaped — safe to inject into the panel. |

### Rendering & XSS safety

`render_entry_html` (in `render.rs`) escapes **every** piece of untrusted text
(narrative and each PR summary/outcome) via `html_escape` (`& < > " '`), so a
narrative or PR summary containing `<script>` renders as inert text. A quiet or absent
day renders an honest note and **no** PR table — never a fabricated one. The client
assigns the server-rendered fragment straight into the panel because it is already
escaped.

## simard-tui: the Journal pane

The [`simard-tui`](./simard-tui.md) terminal dashboard gains a **Journal** pane:

- A new `Tab::Journal` variant; `ALL_TABS` grows to `[Tab; 8]`; **Alt+8** / **Ctrl+8**
  (bare digits are ignored) or **Tab** / **Shift+Tab** / **←** / **→** to reach it.
- The pane reads journal facts directly from the cognitive-memory database
  (read-only, via the same `lbug` path the goal board uses — see the
  [operator read state-root contract](./operator-read-state-root-contract.md)), then
  renders each entry with the shared `render_entry_tui_lines`, so the TUI and the
  dashboard show the same jargon-free story with no HTTP hop.
- Newest-first date list on the left (selected day highlighted); the entry on the
  right. **↑/↓** (or **j/k**) browse days, **/** starts a full-text search whose query
  filters the list, **Esc** clears it, **r** reloads. An empty store renders an honest
  "no entries yet" message.

## Configuration

| Variable | Default | Effect |
| --- | --- | --- |
| `SIMARD_JOURNAL_ENABLED` | `1` (on) | Set to a falsey value (`0`/`false`/`no`/`off`) to stop generating new entries. Existing entries stay browseable. |
| `SIMARD_JOURNAL_INTERVAL_SECS` | `3600` | Regeneration cadence for today's entry; clamped to a `60`s floor. |

## Security

- **XSS:** all operator-visible free text is HTML-escaped at render time; the render
  route returns inert markup.
- **Fail-loud corruption:** a corrupt `journal:`-keyed record surfaces as
  `SimardError::InvalidJournalRecord` on exact lookup rather than silently vanishing;
  broad enumeration stays lenient so one bad record never breaks browsing.
- **No new write surface:** every route and TUI path is read-only; entries are written
  only by the in-process thread through the existing caller-key dedup contract.

## Telemetry

The store and thread emit structured [tracing](./telemetry-metrics.md) on the
`simard::journal` target (entry saved; tick generated), and the daemon logs each
tick's outcome with the `[simard] journal:` prefix. No `println!`/`eprintln!` beyond
the `[simard] …` daemon-log convention.

## Related

- [The Simard Journal](../concepts/simard-journal.md) — concept & rationale.
- [Browse the Simard Journal](../howto/browse-the-simard-journal.md) — operator how-to.
- [Read Simard's daily journal](../tutorials/read-simards-daily-journal.md) — guided first read.
- [Episodic recall](./cognitive-memory-episodic-recall.md),
  [fact recall / caller-key dedup](./cognitive-memory-fact-recall.md),
  [cognitive-thread scheduling](./cognitive-thread-scheduling.md).
