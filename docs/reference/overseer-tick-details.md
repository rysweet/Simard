---
title: Overseer tick details reference
description: The informative, human-readable per-tick detail lists (observed_details / action_details) that make the Overseer activity log say WHAT it observed and WHAT it did with concrete values — the OverseerTickReport detail model, the sanitize_detail safety contract, the typed describe_* renderers (Signal::describe / describe_action / describe_hold / describe_act_error), the humanize_tick_details / overseerTickDetails helpers, and how the dashboard Overseer tab, TUI Overseer pane, and simard status render the detail lines beneath the unchanged summary.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./overseer-activity-feed.md
  - ./verify-and-merge-escalation-idempotency.md
  - ../howto/watch-overseer-activity.md
  - ./dashboard-action-detail-humanization.md
  - ./status-snapshot-api.md
  - ../design/overseer.md
  - ./simard-tui.md
---

# Overseer tick details reference

The [Overseer activity feed](./overseer-activity-feed.md) records the last `N`
Overseer ticks and surfaces them on the dashboard **Overseer** tab, the TUI
**Overseer** pane, and `simard status`. Originally each tick rendered as a
single count-only summary line:

```
15:30  saw 3 problems  ·  filed 1 issue, launched 1 fix, held 1
```

That line tells an operator *that* the steward saw and did something — but not
**what**. Which three problems? Which issue, on which repo, at what URL? Which
workstream? Why was one intervention held? The counts were honest but not
**informative**.

This reference documents the **tick details** layer: two additive,
human-readable string lists on every `OverseerTickReport` —
`observed_details` (WHAT it observed, with concrete values) and
`action_details` (WHAT it did, and its outcome — or, when it did nothing, WHY).
Both are rendered **beneath the unchanged summary** on all three surfaces, so
the same tick now reads:

```
15:30  saw 3 problems  ·  filed 1 issue, launched 1 fix, held 1
       observed: distill parse-failure rate 34% (threshold 20%)
       observed: CI failures rysweet/Simard: 3 failing across recent runs
       observed: blocked goal g-42: waiting on upstream review — needs human review
       did: filed issue https://github.com/rysweet/Simard/issues/2631
       did: launched workstream w-7 (fix distill parse failures)
       held: verify-and-merge PR rysweet/Simard#1299 — budget gate: $19.80 of $20.00 spent
```

> **This layer only *describes* what the tick already decided and did.** It adds
> no decision or intervention logic. The details are captured at the tick
> boundary from data the meta-OODA cycle already produced (`cycle.problems`,
> each `ActOutcome`, and the held plan) and rendered verbatim. It is
> **additive** and **backward-compatible**: an older reader that does not know
> the two fields ignores them; a newer reader given an older, detail-less file
> shows the summary exactly as before.

> **Modules:** report model `src/overseer/wiring.rs`
> (`OverseerTickReport`, `describe_action`, `describe_hold`,
> `describe_act_error`); signal renderer + safety primitive
> `src/overseer/signal.rs` (`Signal::describe`, `sanitize_detail`,
> `DETAIL_CAP`, `DETAIL_STR_CAP`); text helper
> `src/overseer/activity.rs` (`humanize_tick_details`); surfaces
> `src/operator_commands_dashboard/index_html/part_05.rs`
> (`overseerTickDetails`), `src/bin/simard_tui/tabs/overseer.rs`,
> `src/status/render.rs` (`render_overseer`).

## Data model

Two fields are added to the existing
[`OverseerTickReport`](./overseer-activity-feed.md#data-model)
(`src/overseer/wiring.rs`). They sit alongside the count fields, which are
**unchanged**:

```rust
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverseerTickReport {
    // ── existing count fields (unchanged) ────────────────────────────────
    pub problems: usize,
    pub issues_filed: usize,
    pub recipes_launched: usize,
    pub prs_merged: usize,
    pub deploys: usize,
    pub escalations: usize,
    pub held: usize,
    pub whispers: usize,
    pub whispers_suppressed: usize,
    pub goals_unblocked: usize,
    pub goals_escalated: usize,
    pub goals_health_suppressed: usize,
    pub errors: usize,
    pub panicked: bool,

    // ── ADDITIVE (#4080): fatal-cycle signal, health-derivation input ─────
    /// `true` **only** when the tick's OODA cycle itself failed — either
    /// `run_cycle()` returned `Err` or the tick panicked (in which case
    /// `panicked` is also set). It is the fatal-failure signal that (with
    /// `panicked`) drives the `overseer` meta-thread's `"erroring"` health:
    /// `last_success = !panicked && !cycle_failed`.
    ///
    /// It is deliberately **separate** from `errors`: an isolated
    /// per-intervention `act()` failure increments `errors` (for visibility)
    /// but leaves `cycle_failed` **false**, so a transient capability error no
    /// longer pins the meta-thread in `"erroring"`. Covered by the struct-level
    /// `#[serde(default)]` (defaults to `false` for legacy JSON written before
    /// the field existed), so it needs no field-level attribute and does
    /// **not** bump `SCHEMA_VERSION`. Placed immediately after
    /// `panicked` so the two fatal-failure signals sit together.
    pub cycle_failed: bool,

    pub duration_ms: u64,

    // ── NEW: informative, human-readable detail lists ────────────────────
    /// The concrete evidence behind the `problems` count, in ranked
    /// (most-important-first) order: one line per **evidence `Signal`** of each
    /// ranked `Problem` (via `Signal::describe`), followed by each **benign**
    /// non-escalating signal. Its length is therefore generally **greater than**
    /// `problems` (a problem may cite several evidence signals, and benign
    /// signals add lines) — it is *not* one line per problem. Lines are stored
    /// **bare** (no `observed:` prefix); the prefix is added at print time. Each
    /// line is `sanitize_detail`-cleaned (see [Safety contract](#safety-contract))
    /// and capped.
    pub observed_details: Vec<String>,

    /// One line per action taken, held, or suppressed this tick, in
    /// plan-execution order — the concrete detail behind the "did / held"
    /// summary. Unlike `observed_details`, these lines are stored
    /// **self-prefixed** (`did: …` / `held: …` / `… suppressed …`; a failed
    /// action renders as a `did: … failed …` line) by their `describe_*`
    /// renderer, because the list mixes kinds and a flat `Vec<String>` cannot be
    /// reclassified downstream (see [Prefix ownership](#prefix-ownership)). Each
    /// line is sanitized and capped.
    pub action_details: Vec<String>,
}
```

### Why `Vec<String>` (structured strings), not free-form parsing

The detail lines are produced by **typed renderers** over the meta-OODA cycle's
own types (`Signal`, `Problem`, `Intervention`, `ActOutcome`) — never by
parsing a pre-formatted summary back apart. This follows guideline **G3**
(*prefer structured data + templated rendering over brittle string parsing*):
the renderers are pure functions, unit-tested per variant, and each surface is
a dumb display of the already-rendered strings. The list is `Vec<String>`
rather than a richer typed enum on purpose — the values are for humans, the
producing side owns all formatting, and every reader (Rust and the SPA's JS)
gets one flat, safe, escaped list to show.

### Prefix ownership

The two lists deliberately differ in **where the leading label comes from**,
and getting this right is what keeps a flat `Vec<String>` unambiguous:

| List | Stored as | Who adds the prefix | Why |
|---|---|---|---|
| `observed_details` | **bare** — `Signal::describe` output only, no leading label | the **print** side (`humanize_tick_details` / `overseerTickDetails`) prepends a uniform `observed: ` | the list is homogeneous (every line is an observation), so one prefix applies to all |
| `action_details` | **self-prefixed** — each line already begins with `did: ` / `held: ` / `… suppressed …` (a failed action renders as a `did: … failed …` line) | the **renderer** (`describe_action` / `describe_hold` / suppression / `describe_act_error`) | the list is heterogeneous (did vs held vs suppressed vs error); a downstream helper cannot reclassify a bare string, so each renderer must self-describe |

Consequently `humanize_tick_details` (and its JS twin) prepend `observed: ` to
`observed_details` but pass `action_details` through **verbatim** — never
re-prefixing an already-labelled action line. This is why the persisted
`action_details` in the [`GET /api/overseer`](#get-apioverseer-extended-response)
JSON below carry their `did:` / `held:` labels, while `observed_details` do not.

### Backward compatibility

- Both fields carry the struct-level `#[serde(default)]`, so an older
  `activity.json` that predates them deserializes with **empty** lists and every
  surface falls back to the summary line — no error, no blank panel.
- A newer file read by an older binary has the two unknown fields **ignored**
  by `serde`; the summary still renders.
- `OverseerTickReport` **no longer derives `Copy`** (`Vec` is not `Copy`). It
  keeps `Clone`, `Debug`, `Default`, `PartialEq`, `Eq`, `Serialize`,
  `Deserialize`. Every call site consumes the report by reference or by a single
  move, so removing `Copy` is source-compatible.
- The struct-level `#[serde(default)]` also covers the additive
  `cycle_failed: bool` field (#4080): an older `activity.json` that predates it
  deserializes with `cycle_failed = false`, and a newer file read by an older
  binary ignores the unknown field. The derived health value the reader consumes
  is unaffected either way.
- The feed's [`SCHEMA_VERSION`](./overseer-activity-feed.md#data-model) stays at
  **`1`**. This is an *additive* change, not an incompatible one, so a
  rolling deploy (old daemon writing, new reader, or vice-versa) is safe in
  both directions.

### Bounds

Details are bounded **at capture time** so the feed can never grow unbounded:

| Constant | Value | Meaning |
|---|---|---|
| `DETAIL_CAP` | `24` | Max **detail** lines per list. Overflow is dropped and a final `"(+N more)"` marker is appended *after* the cap — the marker does **not** count toward the 24, so a saturated list holds at most 24 detail lines plus the one marker (25 entries). |
| `DETAIL_STR_CAP` | `512` | Max characters per line. A longer line is truncated with a trailing `…`. |

These sit inside the feed's existing `N = 100` retained-ticks cap and the 8 MiB
read guard, so total feed size stays bounded regardless of how noisy a single
tick is.

## Safety contract

Detail lines are **operator-visible, persisted to disk, and rendered into a
web page**, so every line — from every `describe_*` renderer — is passed
through one choke point, **`sanitize_detail(&str) -> String`**
(`src/overseer/signal.rs`), before it is stored:

1. **Strip terminal-escape / control characters.** ANSI escape sequences
   (`\x1b…`) and C0 control bytes are removed so a value that ever contained a
   terminal escape cannot repaint or corrupt the TUI / `simard status` output.
2. **Collapse whitespace.** Newlines, tabs, and runs of spaces collapse to a
   single space, keeping every detail to exactly one line.
3. **Redact secret-shaped substrings.** GitHub token prefixes (`ghp_…`,
   `gho_…`, `ghu_…`, `ghs_…`, `ghr_…`, `github_pat_…`) and long,
   high-entropy alphanumeric blobs (bearer tokens / opaque credentials) are
   replaced with `<redacted-secret>` so a leaked credential in an upstream
   error or URL is never written to the feed. Repo slugs, ids (`g-9`,
   `ws-77`), and URLs (whose alnum runs are broken by `/` and `.`) are left
   untouched.
4. **Truncate** to `DETAIL_STR_CAP` with a trailing `…`.

On top of that:

- **The SPA escapes every line again at render** with `esc()` and writes it as
  **element text content** (never `innerHTML` of raw markup, never an
  attribute/URL sink), so a hostile payload such as
  `</div><script>…` or `<img onerror=…>` renders inert. This is covered by an
  XSS regression test.
- **Errors are classified, never raw.** `describe_act_error` emits a
  `did: … failed` line keyed by the failing capability / gate (e.g.
  `"did: verify-and-merge rysweet/Simard#5 failed — merge: … (isolated)"`),
  with the whole line run through `sanitize_detail`, so an error string
  cannot leak a token, an internal path, or a stack detail into the feed.
- **Format-string safety.** Untrusted fields (repo names, goal reasons, URLs)
  are only ever interpolated as `{}` arguments, never used *as* a format string.

## Renderers

### `Signal::describe` — observed detail

`Signal::describe(&self) -> String` (`src/overseer/signal.rs`) renders each
`Signal` variant to a concrete, plain-language line with its **actual values**.
`observed_details` is assembled at the tick boundary from the meta-OODA
[`CycleReport`](../design/overseer.md) — which carries both `problems` (ranked,
deduplicated) and the raw `signals` set — in two passes:

1. **Problems first, ranked.** For each `Problem` in `cycle.problems` order
   (most-important-first), emit one line per **evidence `Signal`** via
   `Signal::describe`. Most problems cite a single primary evidence signal, so
   they contribute one line; a multi-evidence problem contributes several. A
   problem with no evidence signals falls back to its `summary`. (The problem
   `summary` is *not* emitted separately alongside its evidence — that would
   duplicate the line — so `observed_details.len()` is generally greater than
   `problems`.)
2. **Benign context last.** Each `Signal` in `cycle.signals` that is **not**
   cited as evidence by any problem — i.e. it never escalated — is appended as a
   low-priority observed line, tagged benign. This is what lets a quiet tick
   truthfully say *what it looked at*, not just how many problems it found.

| `Signal` variant | Rendered line (example) |
|---|---|
| `DistillFailureRate { pct }` | `distill parse-failure rate 34% (threshold 20%)` |
| `RestartChurn { restarts }` | `daemon restart churn: 4 restarts in window` |
| `LadderExhausted { count }` | `reasoner decide-ladder exhausted 3× this window` |
| `BudgetPressure { spent_usd, budget_usd }` | `LLM budget pressure — $18.40 of $20.00 spent today` |
| `EngineerSpawnRate { live }` | `engineer spawn elevated — 9 engineers live` |
| `MemoryGrowth { nodes_total }` | `cognitive-memory growth — 120,344 nodes` |
| `GymSkipped` | `gym self-eval skipped` |
| `CiFailureCluster { repo, failing }` | `CI failures rysweet/Simard: 3 failing across recent runs` |
| `PrReadyToMerge { repo, pr }` | `PR rysweet/Simard#1299 green and merge-ready` |
| `StaleGoal { goal_id }` | `goal g-42 re-litigated / stale-complete repeatedly` |
| `Anomaly { detail }` | `anomaly: <detail>` |
| `LoopDetected { goal_id, consecutive_no_action }` | `goal g-42 looping — 2 cycles with no progress` |
| `DriftCorrection { goal_id, detail }` | `goal g-42 drifting from intent: <detail>` |
| `GoalBlocked { goal_id, reason, needs_review, … }` | `blocked goal g-42: <reason> — needs human review` |

> `Signal::describe` covers **all 14** variants for completeness and forward
> compatibility, but `signals_from` derives only 12 today — `MemoryGrowth` and
> `StaleGoal` are defined but **not currently emitted**, so their rows above are
> illustrative of the renderer, not of a line you will see in the feed yet.

Because the benign pass (step 2 above) reads `cycle.signals` directly, a raw
signal that never escalated to a `Problem` (e.g. a lone `GymSkipped`, or an
`Anomaly` that Orient did not fold into a problem) still appears — rendered by
the same `Signal::describe` and marked benign at print:

```
observed: gym self-eval skipped — benign, no action needed
```

### `describe_action` — action detail

`describe_action(&Intervention, &ActOutcome) -> String`
(`src/overseer/wiring.rs`) renders one action line, **self-prefixed with
`did: `**, borrowing the concrete identifiers (repo, PR number, goal id, URL)
from the **paired intervention** when the outcome variant is a unit (e.g.
`ActOutcome::Merged` carries no PR number itself — it is read from the
`VerifyAndMergePr { repo, pr }` intervention it resolved). The example column
below shows the stored line **including** its `did: ` prefix (see
[Prefix ownership](#prefix-ownership)):

| `ActOutcome` | Rendered line (example, as stored) |
|---|---|
| `Launched(WorkstreamHandle { id })` | `did: launched workstream w-7 (fix distill parse failures)` |
| `Merged` | `did: merged PR rysweet/Simard#1299` |
| `ConflictResolved` | `did: resolved merge conflict on rysweet/Simard#1287` |
| `Deployed(report)` | `did: ran guarded deploy — canary healthy` |
| `IssueFiled(FiledNew { url })` | `did: filed issue https://github.com/rysweet/Simard/issues/2631` |
| `IssueFiled(MatchedExisting { url })` | `did: matched existing issue https://github.com/rysweet/Simard/issues/2588` |
| `GoalTransferred` | `did: transferred goal g-42 to a fresh workstream` |
| `Escalated` | `did: escalated to operator: distill failures over threshold 3 ticks running` |
| `Whispered { signature }` | `did: whispered steer note to Simard (loop-correction g-42)` |
| `GoalUnblocked { goal_id }` | `did: self-healed blocked goal g-42 — unblocked + reactivated` |
| `GoalEscalated { goal_id }` | `did: escalated blocked goal g-42 for human review` |
| `Audited` | `did: ran self-quality audit` |
| `Reported` | `did: recorded stewardship report` |

> **`IssueOutcome` here is the Overseer capabilities-layer projection**
> (`src/overseer/capabilities.rs`), whose `FiledNew` / `MatchedExisting`
> variants carry only a `url`. It is intentionally distinct from the richer
> `stewardship::StewardshipOutcome` (repo / issue_number / url / signature);
> only the canonical issue `url` is surfaced in the feed.

Only canonical `github.com/owner/repo#N` and issue URLs are emitted — no query
strings, no tokens (and `sanitize_detail` redacts any that slip through).

### `describe_hold` — "did nothing, and why"

When a planned intervention is **not admitted** (`planned.admitted == false`),
`describe_hold(&PlannedIntervention) -> String` renders *why*, using the gate
reason the plan already carries in `planned.note`:

```
held: verify-and-merge PR rysweet/Simard#1299 — budget gate: $19.80 of $20.00 spent
held: guarded deploy — autonomy gate: HIGH-RISK requires operator approval
```

Suppressed outcomes carry their own reason from the `ActOutcome`:

| `ActOutcome` | Rendered line (example) |
|---|---|
| `WhisperSuppressed { reason }` | `whisper suppressed — duplicate within 15-min window` |
| `GoalHealthSuppressed { reason }` | `goal-health action suppressed — per-hour cap reached` |

Like `describe_hold`'s `held: ` lines, these suppression lines are stored
**self-contained** (the word *suppressed* is their own classifier); the print
helpers pass them through verbatim rather than adding a `did: ` prefix.

### `describe_act_error` — classified failure

An `act` error is rendered by `describe_act_error(&Intervention, &OverseerError)
-> String` as a **classified** line — the intervention label plus a safe
context word — never the raw error body:

```
did: intervention verify-and-merge failed — merge blocked (isolated, tick continued)
```

Such an isolated failure increments `errors` (for feed / totals visibility) and
appears here in `action_details`, but it leaves `cycle_failed` **false** and so
does **not** pin the `overseer` meta-thread in `"erroring"` (#4080) — only a
failed `run_cycle()` or a panic does. See
[`health` derivation](./overseer-activity-feed.md#what-sets-the-meta-threads-consecutive_errors-4080).

A genuinely empty tick (no problems, no actions, no holds, nothing suppressed)
leaves both lists empty and keeps only the existing
`observing, no action needed` summary — there is nothing to detail.

## Text helper — `humanize_tick_details`

`humanize_tick_details(&OverseerTickReport) -> Vec<String>`
(`src/overseer/activity.rs`) is the shared, surface-agnostic helper that turns
the two lists into ready-to-print lines. It is deliberately asymmetric, matching
[Prefix ownership](#prefix-ownership): it **prepends `observed: `** to each
(bare) `observed_details` line, and passes each (already self-prefixed)
`action_details` line through **verbatim**:

```rust
// observed_details are stored bare  → helper prepends "observed: "
// action_details are stored prefixed → helper passes through unchanged
//   ("did: …" / "held: …" / "… suppressed …"; a failed action is a "did: … failed …" line)
let lines = humanize_tick_details(&record.report);
```

It is the companion of the existing
[`humanize_tick`](./overseer-activity-feed.md#data-model), which produces the
one-line summary. **`humanize_tick` is unchanged (byte-for-byte)** so the
existing summary needles and tests keep passing; `humanize_tick_details` is
strictly additive and used by the terminal (`simard status`) and TUI surfaces.
The dashboard mirrors the same logic in JS (`overseerTickDetails`, below).

## Rendering per surface

All three surfaces render **the same** data — the summary line first, then the
detail lines indented beneath it — and cap the number of detail lines shown per
tick so a burst can never dominate the panel.

### `simard status` — `OVERSEER` section

`render_overseer` (`src/status/render.rs`) prints each tick's `humanize_tick`
summary, then the `humanize_tick_details` lines indented under it:

```console
OVERSEER
  recent
    15:30  saw 3 problems  ·  filed 1 issue, launched 1 fix, held 1
             observed: distill parse-failure rate 34% (threshold 20%)
             observed: CI failures rysweet/Simard: 3 failing across recent runs
             observed: blocked goal g-42: waiting on upstream review — needs human review
             did: filed issue https://github.com/rysweet/Simard/issues/2631
             did: launched workstream w-7 (fix distill parse failures)
             held: verify-and-merge PR rysweet/Simard#1299 — budget gate: $19.80 of $20.00 spent
    15:15  saw 1 problem  ·  merged 1 PR
             observed: PR rysweet/Simard#1287 green and merge-ready
             did: merged PR rysweet/Simard#1287
    15:00  observing, no action needed
```

### TUI **Overseer** pane

`render_lines` (`src/bin/simard_tui/tabs/overseer.rs`) emits the summary line
then the indented detail lines, capped at `DETAIL_ROWS = 12` per tick with a
trailing `… N more` when a tick has more:

```
Recent activity:
  15:30  saw 3 problems  ·  filed 1 issue, launched 1 fix, held 1
           observed: distill parse-failure rate 34% (threshold 20%)
           observed: CI failures rysweet/Simard: 3 failing across recent runs
           observed: blocked goal g-42: waiting on upstream review — needs human review
           did: filed issue https://github.com/rysweet/Simard/issues/2631
           did: launched workstream w-7 (fix distill parse failures)
           held: verify-and-merge PR rysweet/Simard#1299 — budget gate: $19.80 of $20.00 spent
```

### Dashboard **Overseer** tab

`overseerTickDetails(r)` (`src/operator_commands_dashboard/index_html/part_05.rs`)
reads `r.observed_details` / `r.action_details` (each guarded with `?? []`),
`esc()`s **every** string, and appends one sub-`<div>` (`data-testid=
"overseer-detail"`) per line beneath the existing `overseerTickHuman(r)` summary
row. Mirroring `humanize_tick_details`, it **prepends `observed: `** to each
`observed_details` line and renders each `action_details` line **verbatim**
(already `did:` / `held:` / `… suppressed …`-prefixed). `overseerTickHuman`
itself is **unchanged**, so the existing summary and its HTML-contains test are
preserved; the details are additive DOM beneath it.

```
15:30 — saw 3 problems · filed 1 issue, launched 1 fix, held 1
          observed: distill parse-failure rate 34% (threshold 20%)
          observed: CI failures rysweet/Simard: 3 failing across recent runs
          observed: blocked goal g-42: waiting on upstream review — needs human review
          did: filed issue https://github.com/rysweet/Simard/issues/2631
          did: launched workstream w-7 (fix distill parse failures)
          held: verify-and-merge PR rysweet/Simard#1299 — budget gate: $19.80 of $20.00 spent
```

## `GET /api/overseer` — extended response

The two fields ride the **existing** `GET /api/overseer` envelope — no new
route, verb, or auth path (it stays behind `require_auth`; see the
[activity feed reference](./overseer-activity-feed.md#dashboard-endpoint-get-apioverseer)).
Each `recent[].report` now carries the detail lists:

```jsonc
{
  "section": {
    "availability": "ok",
    "freshness": "live",
    "data": {
      "recent": [
        {
          "timestamp": "2026-07-05T15:30:00Z",
          "enabled": true,
          "report": {
            "problems": 3, "issues_filed": 1, "recipes_launched": 1,
            "prs_merged": 0, "held": 1, "errors": 0,
            "panicked": false, "duration_ms": 843,

            "observed_details": [
              "distill parse-failure rate 34% (threshold 20%)",
              "CI failures rysweet/Simard: 3 failing across recent runs",
              "blocked goal g-42: waiting on upstream review — needs human review"
            ],
            "action_details": [
              "did: filed issue https://github.com/rysweet/Simard/issues/2631",
              "did: launched workstream w-7 (fix distill parse failures)",
              "held: verify-and-merge PR rysweet/Simard#1299 — budget gate: $19.80 of $20.00 spent"
            ]
          }
        }
      ]
    }
  }
}
```

Both lists are optional (a pre-details tick, or an older reader, sees them
absent/empty) and newest-first alongside their tick. As stored, `observed_details`
are **bare** (the `observed: ` prefix is a render-time affordance) while
`action_details` are **self-prefixed** (`did:` / `held:` / `… suppressed …`) —
so a `jq` consumer sees exactly what is persisted. Script against them the same
way as any other field:

```bash
# The observed problems of the most recent tick, one per line.
curl -fsS -H "Authorization: ******" \
  http://localhost:8080/api/overseer \
  | jq -r '.section.data.recent[0].report.observed_details[]'

# Everything the steward actually did in the last tick.
curl -fsS -H "Authorization: ******" \
  http://localhost:8080/api/overseer \
  | jq -r '.section.data.recent[0].report.action_details[]'
```

## Configuration

The detail layer has **no configuration of its own** — it describes whatever the
Overseer already observes and does, so it is governed entirely by the existing
[Overseer settings](./overseer-activity-feed.md#configuration)
(`SIMARD_OVERSEER_ENABLED`, `SIMARD_OVERSEER_INTERVAL_SECS`,
`SIMARD_OVERSEER_AUTHOR_LOGIN`). The `DETAIL_CAP` / `DETAIL_STR_CAP` /
`DETAIL_ROWS` bounds are compile-time constants, not env knobs.

## Guarantees

- **Informative, not just counted.** Every non-empty tick names the concrete
  problems observed (with values) and the concrete actions taken (with
  ids/URLs) — or, when it acted on nothing, *why* (gate/suppression reason).
- **Additive & backward-compatible.** Two `#[serde(default)]` fields; summary
  counts, `humanize_tick`, `overseerTickHuman`, `totals`, and `SCHEMA_VERSION`
  are all unchanged. Old ↔ new readers/writers interoperate.
- **Structured over parsed (G3).** Details come from typed `describe_*`
  renderers over the cycle's own types, unit-tested per variant — never from
  re-parsing a summary string.
- **Safe.** Every line is `sanitize_detail`-cleaned at capture (control-strip,
  whitespace-collapse, secret-redact, truncate) and `esc()`-escaped again in the
  SPA as element text; errors are classified, never raw.
- **Bounded & deterministic.** `DETAIL_CAP = 24` lines (`(+N more)`),
  `DETAIL_STR_CAP = 512` chars, ranked/execution ordering — stable for hermetic
  tests.
- **Read-only surfacing.** No Overseer decision or intervention logic changes.

## See also

- [Overseer activity feed reference](./overseer-activity-feed.md) — the feed
  these details ride in: data model, file contract, honest states, and the
  `GET /api/overseer` endpoint.
- [How to watch what the Overseer is doing](../howto/watch-overseer-activity.md)
  — the operator walkthrough, updated with the detail lines.
- [Overview action-detail humanization](./dashboard-action-detail-humanization.md)
  — the sibling render-layer humanizer for the OODA **Overview** tab's action
  detail strings.
- [Overseer design](../design/overseer.md) — the meta-OODA loop, `Signal` /
  `Problem` / `Intervention` vocabulary, and guardrails these renderers describe.
