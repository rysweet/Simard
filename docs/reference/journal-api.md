---
title: "Journal API"
description: >
  Reference for the src/journal module: the JournalEntry / PrSummary / DayContext data model,
  the journal:YYYY-MM-DD key scheme over cognitive memory, the two-pass generation pipeline
  (injectable clock, episode & PR sources, drafter, mandatory de-jargon reviewer) in both its
  prompt-first (recipe-runner) and deterministic-fallback forms, the shared report renderer
  (report headings + markdown/HTML PR table for dashboard AND TUI), the JournalStore query API
  (date range + full text, newest-first), the daily journal thread, the dashboard HTTP routes
  and Journal tab, the simard-tui Journal pane, configuration, security, and telemetry.
last_updated: 2026-07-06
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
  - ../architecture/episode-distillation.md
  - ./recipe-context-file-transport.md
  - ./journal-narrative-result-channel.md
  - ../concepts/journal-recipe-spawn-e2big.md
  - ../howto/diagnose-journal-e2big-spawn-failures.md
---

# Journal API

The **journal** module (`src/journal/`) turns Simard's day into a durable, searchable,
layperson-readable **narrative engineering & research report** stored in cognitive memory.
This page is the interface contract: types, traits, the storage-key scheme, the query API,
the two-pass generation pipeline (prompt-first plus deterministic fallback), the generation
recipes, the shared renderer, the operator surfaces, configuration, and the security and
telemetry guarantees.

For the *why*, read [The Simard Journal](../concepts/simard-journal.md). For day-to-day
operator use, read [Browse the Simard Journal](../howto/browse-the-simard-journal.md).

!!! warning "Implementation status — target design under issue #2606"
    This page is the **implementation spec** for the journal module as it is being rebuilt
    under issue #2606. It describes the intended finished state — the third-person report
    structure, the prompt-first recipe pipeline, the verbose prepared-context material, the
    timestamped moments, and the unconditional secret-redaction post-pass. Several of these
    are **not yet reflected in `src/journal/`**, which still carries the pre-fix #2618
    behaviour (a first-person "Dear diary" template drafter, a 3-field `PrSummary`, and a
    glossary-only reviewer). Read this as the contract we are building toward, not as a
    description of already-shipped behaviour. Symbols marked "(#2606)" are additions this
    work introduces.

## Module layout

```text
src/journal/
├── mod.rs         // pub re-exports; `pub mod journal;` is declared in src/lib.rs
├── types.rs       // JournalEntry, PrSummary, MemoryGrowth, DayContext (+ serde)
├── jargon.rs      // JOURNAL_GLOSSARY + scrub_jargon + scrub_secrets (the default reviewer body)
├── generate.rs    // JournalDrafter / JournalReviewer traits; TemplateDrafter + GlossaryReviewer
│                  //   (offline fallback); JournalGenerator (draft -> ALWAYS review ->
│                  //   scrub_secrets); default_pipeline() + for_repo()
├── recipe.rs      // RecipeDrafter + RecipeReviewer (prompt-first, recipe-runner-backed;
│                  //   degrade per-call to the deterministic drafter / glossary reviewer)
├── providers.rs   // JournalClock / EpisodeSource / PrListSource seams, DayExtras,
│                  //   assemble_day_context, generate_and_store; episode_time_label
├── render.rs      // html_escape, render_entry_html, render_entry_tui_lines (shared renderer)
├── store.rs       // JournalStore + borrowed free fns over &dyn CognitiveMemoryOps
├── pr_source.rs   // GhPrListSource: gh pr list -> plain-language PrSummary rows
└── thread.rs      // run_journal_tick(_with_prs[_in_repo]) + enable/interval env helpers
```

The module is a self-contained brick. Its only hard dependency is the existing
[`CognitiveMemoryOps`](./cognitive-memory-fact-recall.md) trait; everything the generator
needs from the outside world is injected behind a small trait, which is what makes the whole
pipeline testable with no network, no clock, no live daemon, and no `recipe-runner-rs`.

## Data model

Defined in `src/journal/types.rs`; all records are `serde` (de)serializable. New fields are
added **additively** with `#[serde(default)]`, so entries written before this change still
deserialize.

```rust
/// A plain-language summary of one code-change proposal (pull request).
pub struct PrSummary {
    pub number: u64,           // rendered as `#123`
    pub plain_summary: String, // "what changed & why it matters", in plain language
    pub outcome: String,       // plain-language readiness phrase (see below)
}

/// A single day's report entry — the durable, persisted record.
pub struct JournalEntry {
    pub date: chrono::NaiveDate,             // UTC day; also the storage key
    pub generated_at: chrono::DateTime<chrono::Utc>, // latest (re)generation time
    pub narrative: String,                   // final, reviewed, jargon-free REPORT
    pub draft: String,                       // pre-review draft (provenance; draft != narrative)
    pub prs: Vec<PrSummary>,                 // backs the plain-language PR table
    pub quiet_day: bool,                     // rendered honestly, never fabricated
}
```

`MemoryGrowth { facts_added, episodes_added }` carries the day's memory growth when measured.

`outcome` is a **plain-language readiness phrase** for an *open* proposal, derived from the
same objective gates the merge authority evaluates (`pr_readiness_outcome`) — one of
`"still open — ready to combine into the main code"`, `"still open — automated checks still
running"`, or `"still open — not ready yet"`. The journal reads the **open** PR readiness
view — the same `gh pr list` service the dashboard's Merge Readiness panel uses (#1880) — so
lifecycle states such as "merged"/"closed" are intentionally **not** surfaced by this seam;
the column reports readiness, not lifecycle.

The transient generation input is `DayContext`. Alongside the primary episode source it
carries the **verbose prepared-context** material — brief descriptions of the day's facts,
triggers, and procedures — so the report can summarise *what* they were, not just how many:

```rust
pub struct DayContext {
    pub date: chrono::NaiveDate,
    pub episodes: Vec<CognitiveEpisode>, // PRIMARY narrative source (carry timestamps)
    pub prs: Vec<PrSummary>,
    pub goals: Vec<String>,
    pub deploys: Vec<String>,
    pub overseer_events: Vec<String>,
    pub memory_growth: Option<MemoryGrowth>,
    pub notable: Vec<String>,
    // Verbose prepared-context (issue #2606) — brief readable descriptions, not counts:
    pub facts: Vec<String>,      // key facts prepared for the day
    pub triggers: Vec<String>,   // prospective triggers that fired
    pub procedures: Vec<String>, // procedures recalled/applied
}
```

`DayContext::is_quiet()` is `true` when nothing happened worth narrating; the drafter then
emits an honest "quiet day" paragraph rather than an empty or invented one. Episodes are the
primary source; every other field is a best-effort augmentation that is simply omitted when
absent (honest degradation).

### Episode timestamps

Each remembered moment is rendered **with its timestamp** and in **chronological order**.
`episode_time_label(temporal_index: i64) -> String` (in `providers.rs`) turns an episode's
`temporal_index` into a human-readable label:

- an epoch-second magnitude is formatted as a UTC time label (e.g. `2026-07-06 14:32 UTC`);
- a small monotonic counter (not a real epoch) degrades to a stable ordinal label
  (`moment 1`, `moment 2`, …), so a test fixture using counters still renders sensibly.

The drafter sorts episodes by `temporal_index` ascending before rendering, so the report
reads oldest-to-newest.

## The two-pass generation pipeline

Generation is deliberately **two-pass** so the jargon-free guarantee is structural, not
incidental (see the concept's
[two-pass rewrite](../concepts/simard-journal.md#two-passes-draft-then-a-mandatory-de-jargon-rewrite-that-has-teeth)):

```rust
pub trait JournalDrafter: Send + Sync { fn draft(&self, day: &DayContext) -> String; }
pub trait JournalReviewer: Send + Sync { fn review(&self, draft: &str) -> String; }

pub struct JournalGenerator { /* boxed drafter + reviewer */ }
impl JournalGenerator {
    pub fn new(drafter: Box<dyn JournalDrafter>, reviewer: Box<dyn JournalReviewer>) -> Self;

    /// Deterministic, offline pipeline: TemplateDrafter + GlossaryReviewer.
    pub fn default_pipeline() -> Self;

    /// Prompt-first for a deployment, with an HONEST fallback to `default_pipeline()`
    /// when `recipe-runner-rs` or a recipe asset is unavailable (issue #2606).
    pub fn for_repo(repo_root: &std::path::Path) -> Self;

    /// draft -> ALWAYS review -> ALWAYS scrub secrets. No path returns an unreviewed
    /// narrative, and no path skips secret redaction (see `generate` below).
    pub fn generate(&self, day: &DayContext) -> JournalEntry;
}
```

`generate` runs the drafter, then **always** runs the reviewer over the draft, then applies
`scrub_secrets` to the reviewed text as an **unconditional post-pass** — so secret redaction
covers the prompt-first reviewer and the offline reviewer alike — and stores both `draft`
(provenance) and the final `narrative`.

### The drafter — a structured, third-person report

Both drafters emit the **same report shape**: an **Overview** paragraph, `##`-headed
**sections** (Engineering work / Research / Key observations / Decisions / Outcomes, each
omitted when empty), the **timestamped chronological** remembered moments, the **verbose
prepared-context** summary (substance of facts/triggers/procedures/moments), and a one-line
lead-in to the pull-request table. Neither drafter uses a diary voice.

- **`RecipeJournalDrafter`** (prompt-first, primary) shells out to `recipe-runner-rs`
  running the [`journal-narrative`](#the-generation-recipes) recipe with the day's context
  passed as delimited context variables. The prompt owns the report shaping and voice.
- **`TemplateDrafter`** (deterministic, offline fallback) assembles the identical structure
  from a template — third person, overview + sections + timestamped episodes + verbose
  context — with **no** "Dear diary" phrasing anywhere.

### The de-jargon rewrite pass

The mandatory second pass is what makes the entry a layperson-readable report — and, unlike
the earlier no-op, it **materially changes** the text:

- **`RecipeJournalReviewer`** (prompt-first, primary) shells out to `recipe-runner-rs`
  running the [`journal-plain-language`](#the-generation-recipes) recipe: it expands
  acronyms, removes raw code identifiers / internal code names / insider jargon, and
  explains any unavoidable technical term in plain words.
- **`GlossaryReviewer`** (deterministic, offline fallback) delegates to `scrub_jargon`, a
  whole-word, case-insensitive glossary substitution (`JOURNAL_GLOSSARY`) — **strengthened**
  in #2606 so raw identifiers, internal code names, and unexpanded acronyms are removed or
  expanded (e.g. `PR` → "pull request", `CI` → "the automated checks", `episodic` →
  "moment-by-moment", `OODA` → "decision cycle").

Regardless of which reviewer runs, `JournalGenerator::generate` then applies `scrub_secrets`
to the reviewed narrative as a **mandatory, unconditional post-pass**, so token/key/PEM-shaped
substrings are redacted on **both** the prompt-first and the offline paths — the LLM reviewer's
output is never trusted to be secret-free on its own.

Both reviewers guarantee the same testable property: for a representative banned-jargon
token list, **none** of those tokens survive into `narrative`, and `narrative != draft`.

```rust
/// The strengthened glossary (whole-word, case-insensitive, longest-phrase-first).
pub const JOURNAL_GLOSSARY: &[(&str, &str)];
/// Rewrite input, replacing every whole-word glossary term with plain language.
pub fn scrub_jargon(input: &str) -> String;
/// Redact secret-shaped substrings (tokens, keys, PEM blocks) — offline, pure.
pub fn scrub_secrets(input: &str) -> String;
```

### The generation recipes

Prompt assets live under `prompt_assets/simard/recipes/`, hot-reloading from
`~/.simard/prompt_assets/simard/recipes/` at runtime with an **in-tree fallback** (the same
`resolve_recipe_path(repo_root)` pattern used by
[episode distillation](../architecture/episode-distillation.md) and the OODA brains). A fix
lands in-repo so a deploy syncs it.

| Recipe file | Role | Key context vars (via `-c`) |
| --- | --- | --- |
| `journal-narrative.yaml` | Draft the structured, third-person report from the day's context. | `day_context` (JSON: `date`, `episodes` with time labels, `prs`, `goals`, `deploys`, `overseer_events`, `facts`, `triggers`, `procedures`, `memory_growth`, `notable`) |
| `journal-plain-language.yaml` | Rewrite a draft into genuinely jargon-free language. | `draft` (delimited) |

The recipes are **prose-only**: the day's data is passed as delimited context variables the
prompt treats as untrusted input, never as instructions. If a recipe fails to parse or the
runner is unavailable, generation **fails closed** to the deterministic fallback rather than
emitting an unreviewed entry.

### Injectable seams (`providers.rs`)

```rust
pub trait JournalClock: Send + Sync { fn today(&self) -> NaiveDate; }     // SystemClock = UTC
pub trait EpisodeSource: Send + Sync { fn episodes_for_date(&self, d: NaiveDate) -> SimardResult<Vec<CognitiveEpisode>>; }
pub trait PrListSource: Send + Sync { fn prs_for_date(&self, d: NaiveDate) -> SimardResult<Vec<PrSummary>>; }

pub struct DayExtras {
    pub goals: Vec<String>,
    pub deploys: Vec<String>,
    pub overseer_events: Vec<String>,
    pub notable: Vec<String>,
    pub memory_growth: Option<MemoryGrowth>,
    // Verbose prepared-context (issue #2606):
    pub facts: Vec<String>,
    pub triggers: Vec<String>,
    pub procedures: Vec<String>,
}

pub fn assemble_day_context(date, episodes: &dyn EpisodeSource, prs: &dyn PrListSource, extras: DayExtras) -> SimardResult<DayContext>;
pub fn generate_and_store(date, episodes, prs, extras, generator: &JournalGenerator, mem: &dyn CognitiveMemoryOps) -> SimardResult<JournalEntry>;
pub fn episode_time_label(temporal_index: i64) -> String;   // UTC label or "moment N"
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
| Read-path dedup | Defensive: should the backend ever surface **more than one** `journal:YYYY-MM-DD` fact for a day, every read collapses them to the single **newest-generated** entry (by `generated_at`) — the same "latest supersedes" semantics as the write path |

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
// the dashboard reader and the daily thread). JournalStore delegates to these.
pub fn save_entry(mem: &dyn CognitiveMemoryOps, entry: &JournalEntry) -> SimardResult<String>;
pub fn get_entry_by_date(mem: &dyn CognitiveMemoryOps, date: NaiveDate) -> SimardResult<Option<JournalEntry>>;
pub fn all_entries(mem: &dyn CognitiveMemoryOps) -> SimardResult<Vec<JournalEntry>>;
pub fn query_entries(mem: &dyn CognitiveMemoryOps, range, text) -> SimardResult<Vec<JournalEntry>>;
pub fn entry_matches(entry: &JournalEntry, query: &str) -> bool;    // same rule the TUI filters with
```

`query` filters by an inclusive date range and/or a case-insensitive substring over the
narrative, the date, and each proposal's summary/outcome/number — newest day first.
This single path backs both the dashboard and the TUI, so they never diverge. This
searchable, browse-by-date behaviour is **preserved unchanged** from #2618.

Enumeration is lenient (a non-journal or unparseable candidate fact is skipped), but a
`journal:`-keyed fact whose JSON is corrupt fails **loud** as
`SimardError::InvalidJournalRecord` on an exact `get_by_date`.

Duplicate-day facts are collapsed at **read** time, not merely on write. The
caller-key write contract is supposed to keep exactly one live fact per day, but
the live store was observed holding two `journal:2026-07-15` facts, which made
the dashboard date picker and search list the same day **twice** with
conflicting PR counts while `GET /api/journal/entry/{date}` returned only one.
The reader now keeps the **newest `generated_at`** entry per day across
`all_entries`, `dates`, `query`, and `get_by_date`, so all four surfaces agree
and each day appears exactly once regardless of how a duplicate arose.


## Rendering — the shared renderer

`render.rs` holds pure functions that turn one `JournalEntry` into each surface, so the
dashboard and TUI render **the same report** from one source of truth. The renderer turns
the report's headings and PR table into proper structure on **both** surfaces (rather than
leaking raw `##` / `|table|` markup):

```rust
pub fn html_escape(s: &str) -> String;                       // & < > " '
pub fn render_entry_html(entry: &JournalEntry) -> String;    // dashboard: <h_>, <p>, <table>
pub fn render_entry_tui_lines(entry: &JournalEntry) -> Vec<String>; // TUI: heading lines + aligned text table
```

- **Dashboard (`render_entry_html`)** emits an `<h2>` date, the overview and each section as
  heading + paragraph blocks, and the pull-request table as a real HTML `<table>` with three
  columns — **PR # / What changed &amp; why it matters / Outcome**. Every piece of untrusted
  text (narrative, and each PR summary/outcome) is passed through `html_escape`, so a
  narrative or PR field containing `<script>` renders as inert text.
- **TUI (`render_entry_tui_lines`)** emits a date header, heading lines for the overview and
  sections, and an **aligned text table** for the same three PR columns. Control bytes in
  untrusted text are neutralised so a malicious field cannot corrupt the terminal.

A quiet or absent day renders an honest note and **no** PR table — never a fabricated one.

## The daily journal thread (`thread.rs`)

The daemon regenerates *today's* entry on a slow cadence — a rolling update that keeps the
day's entry current as remembered moments accumulate.

```rust
pub fn run_journal_tick(mem: &dyn CognitiveMemoryOps, clock: &dyn JournalClock)
    -> SimardResult<JournalEntry>;                       // offline PR source (empty table)
pub fn run_journal_tick_with_prs(mem: &dyn CognitiveMemoryOps, clock: &dyn JournalClock,
    prs: &dyn PrListSource) -> SimardResult<JournalEntry>; // inject the day's real proposals
pub fn journal_enabled() -> bool;        // default true (opt-out)
pub fn journal_interval_secs() -> u64;   // default 3600, floor 60
```

Both ticks read the day's episodics from the store (primary source, carrying timestamps),
fold in the active goals and the verbose prepared-context material (best-effort
augmentation), generate the reviewed report, and persist it via `save_entry`. They differ
only in where the day's code-change proposals come from:

- `run_journal_tick` uses the offline `NoNetworkPrs` source (empty proposal table). Pure and
  network-free — used by tests and as a fallback.
- `run_journal_tick_with_prs` takes an injected `PrListSource`. In production the daemon
  passes a **`GhPrListSource`**, which wraps the `gh pr list` PR-readiness service (the same
  external view the dashboard's Merge Readiness panel uses) and maps each open PR into a
  layperson row: the **what-changed summary** has its Conventional-Commits prefix stripped,
  any Copilot CLI launch-log banner (`ℹ NODE_OPTIONS=… (saved preference)`) dropped via the
  shared `strip_recipe_noise` filter (issue #1093), and its jargon scrubbed
  (`plainify_pr_title`), and the **outcome** is a plain-language
  readiness phrase ("still open — ready to combine into the main code", "…automated checks
  still running", "…not ready yet"), derived from the same objective gates the merge
  authority evaluates. A `gh` failure **degrades honestly** to an empty table (logged) so
  the report is still written.

Wiring: the OODA daemon runs `run_journal_tick_with_prs` default-on, interval-gated, and
panic-isolated, **after** the authoritative OODA cycle so it can never stall or crash the
loop. Because it fetches PRs over the network it runs on a background thread (never inline),
overlap-guarded so a slow tick never stacks. It fires on the first iteration so a fresh
daemon writes the day's entry promptly.

## Dashboard: HTTP routes and the Journal tab

The [operator dashboard](../dashboard.md) has a **Journal** tab — a distinct, additive tab
whose `journal` slug and `/api/journal/*` route namespace do not collide with any other tab.

### HTTP routes

All routes are read-only, registered before `require_auth`, and reuse the existing session
auth. There are no write/delete endpoints — entries are produced only in-process by the
daily thread.

| Route | Response | Notes |
| --- | --- | --- |
| `GET /api/journal/dates` | `{ "dates": [ {date, quiet_day, pr_count}, … ] }` | Newest day first, each day exactly once (duplicate-day facts collapse to the newest generation), for the date picker. |
| `POST /api/journal/search` | `{ "results": [ {date, quiet_day, pr_count, snippet}, … ] }` | Body `{query?, from?, to?}` (dates `YYYY-MM-DD`); newest first, one entry per day. |
| `GET /api/journal/entry/{date}` | `JournalEntry` JSON, or `{status:"error", error}` | `{date}` strictly parsed `%Y-%m-%d`. |
| `GET /api/journal/render/{date}` | `text/html` (server-rendered fragment) | Report headings + 3-column PR table, fully HTML-escaped — safe to inject into the panel. |

## simard-tui: the Journal pane

The [`simard-tui`](./simard-tui.md) terminal dashboard has a **Journal** pane:

- A `Tab::Journal` variant reachable via **Alt+8** / **Ctrl+8** (bare digits are ignored) or
  **Tab** / **Shift+Tab** / **←** / **→**.
- The pane reads journal facts directly from the cognitive-memory database (read-only, via
  the same `lbug` path the goal board uses — see the
  [operator read state-root contract](./operator-read-state-root-contract.md)), then renders
  each entry with the shared `render_entry_tui_lines`, so the TUI and the dashboard show the
  same jargon-free report (headings + aligned PR table) with no HTTP hop.
- Newest-first date list on the left (selected day highlighted); the entry on the right.
  **↑/↓** (or **j/k**) browse days, **/** starts a full-text search whose query filters the
  list, **Esc** clears it, **r** reloads. An empty store renders an honest "no entries yet"
  message.

## Configuration

| Variable | Default | Effect |
| --- | --- | --- |
| `SIMARD_JOURNAL_ENABLED` | `1` (on) | Set to a falsey value (`0`/`false`/`no`/`off`) to stop generating new entries. Existing entries stay browseable. |
| `SIMARD_JOURNAL_INTERVAL_SECS` | `3600` | Regeneration cadence for today's entry; clamped to a `60`s floor. |

The prompt-first pipeline additionally requires `recipe-runner-rs` on `PATH` and the
`journal-narrative` / `journal-plain-language` recipe assets to be resolvable (hot-reload dir
or in-tree). When either is missing, generation falls back to the deterministic pipeline —
no configuration change is needed, and the report is still structured and jargon-free.

## Security

- **XSS:** all operator-visible free text (narrative and each PR summary/outcome) is
  HTML-escaped at render time; the render route returns inert markup.
- **Secret redaction:** `JournalGenerator::generate` runs `scrub_secrets` over the reviewed
  narrative as an **unconditional post-pass** on **both** the prompt-first and offline
  reviewer paths (the LLM reviewer's output is never trusted to be secret-free on its own),
  so token/key/PEM-shaped substrings never reach a stored entry or a surface.
- **Recipe input isolation:** the day's data is passed to recipes as delimited context
  variables treated as untrusted input, never as instructions; a parse failure fails closed
  to the deterministic fallback.
- **Fail-loud corruption:** a corrupt `journal:`-keyed record surfaces as
  `SimardError::InvalidJournalRecord` on exact lookup rather than silently vanishing; broad
  enumeration stays lenient so one bad record never breaks browsing.
- **No new write surface:** every route and TUI path is read-only; entries are written only
  by the in-process thread through the existing caller-key dedup contract.

## Telemetry

The store and thread emit structured [tracing](./telemetry-metrics.md) on the
`simard::journal` target (entry saved; tick generated), and the daemon logs each tick's
outcome with the `[simard] journal:` prefix. No new `println!`/`eprintln!` is added in
production code beyond the existing `[simard] …` daemon-log convention.

## Testing guarantees

The hermetic (network-free, LLM-free) tests exercise the deterministic fallback and assert
the finished-state contract:

- The generated `narrative` contains **report-style headings** (an overview plus sections)
  and a **pull-request table** section.
- A representative **banned-jargon token list** (e.g. raw identifiers, internal code names,
  unexpanded acronyms, and any "Dear diary" phrasing) does **not** appear in `narrative`.
- The de-jargon pass **materially changed** the text: `draft != narrative` (and banned
  tokens permitted in `draft` are absent from `narrative`).
- Each remembered moment renders **with a timestamp**, in chronological order.
- The prepared-context summary describes **substance** (facts/triggers/procedures/moments),
  not bare counts.
- `JournalEntry` / `DayContext` deserialization is **backward-compatible** (older records
  without the verbose prepared-context fields still load via `#[serde(default)]`).

## Related

- [The Simard Journal](../concepts/simard-journal.md) — concept & rationale.
- [Browse the Simard Journal](../howto/browse-the-simard-journal.md) — operator how-to.
- [Read Simard's daily journal](../tutorials/read-simards-daily-journal.md) — guided first read.
- [Episode distillation](../architecture/episode-distillation.md) — the recipe-runner pattern
  the journal recipes follow.
- [Episodic recall](./cognitive-memory-episodic-recall.md),
  [fact recall / caller-key dedup](./cognitive-memory-fact-recall.md),
  [cognitive-thread scheduling](./cognitive-thread-scheduling.md).
