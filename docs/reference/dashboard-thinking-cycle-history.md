---
title: Thinking tab — Cycle History (timestamps, collapse, duration trend)
description: Reference for the Thinking tab's first-half "Cycle History" surface — real per-cycle timestamps, consecutive-cycle collapse with a repeat-count and cycle range, difference-carrying row summaries, and a duration-trend chart that only renders when data exists. Documents the `/api/ooda-cycles` contract, the relaxed collapse mode in `thinking_collapse.rs`, and the producer-side timestamp/duration telemetry — while the second-half OODA Observe/Orient/Decide/Act reasoning breakdown is preserved unchanged (#21).
last_updated: 2026-07-06
owner: simard
doc_type: reference
related:
  - ../dashboard.md
  - ./dashboard-action-detail-humanization.md
  - ./ooda-brain-decision-protocol.md
  - ./telemetry-metrics.md
---

# Thinking tab — Cycle History

Reference documentation for the **Thinking** tab's *first half*, the **Cycle
History** surface. This page describes the finished behaviour shipped for
issue #21.

The Thinking tab has two halves and they are fed by two different endpoints:

| Half | Surface | Endpoint | Renderer |
|------|---------|----------|----------|
| **First** | **Cycle History** — a compact, collapsed table of recent OODA cycles plus a duration-trend chart | `GET /api/ooda-cycles` | `fetchOodaCycles` (`index_html/part_04.rs`) |
| **Second** | **Agent Internal Reasoning** — the per-cycle Observe / Orient / Decide / Act breakdown | `GET /api/ooda-thinking` | `fetchThinking` (`index_html/part_04.rs`) |

> **The second half is unchanged.** The OODA Observe/Orient/Decide/Act
> reasoning breakdown, its endpoint (`/api/ooda-thinking`), its collapse call
> (`collapse_reports`, strict mode), and its renderer (`fetchThinking`) behave
> exactly as before. Everything on this page concerns the **first half**
> (Cycle History) only.

## What the Cycle History shows

The Cycle History answers three operator questions at a glance:

1. **When did each cycle run?** Every row shows a real timestamp.
2. **Is the agent making progress, or looping?** Runs of equivalent cycles
   collapse into a single row with a `×N` repeat-count and the cycle range, and
   each row's summary describes *what actually happened* rather than boilerplate.
3. **Are cycles getting faster or slower?** A duration-trend chart renders when
   per-cycle duration data exists, and is hidden entirely when it does not.

### Row anatomy

The history table has the columns `# | Phase | Duration | Actions | Summary | Time`:

| Column | Content |
|--------|---------|
| **#** | Cycle number for a single cycle, or `×N (cycles #A–#B)` for a collapsed run, where `A` is the oldest and `B` the newest cycle number (ascending — e.g. `×20 (cycles #1021–#1040)`). |
| **Phase** | Final OODA phase reached in the cycle (`observe` / `orient` / `decide` / `act`). |
| **Duration** | Wall-clock cycle duration (e.g. `12.4s`), or `—` for legacy cycles with no recorded duration. |
| **Actions** | Count of actions taken in the cycle. |
| **Summary** | A **difference-carrying** description of what the cycle did — the decided action, `no-action: deferring to active engineer on <goal>`, or the meaningful decision clause. Never the old count-boilerplate. |
| **Time** | Relative timestamp (e.g. `4m ago`). Shows `—` only for legacy cycles that genuinely have no timestamp. |

Collapsed runs (`repeat_count > 1`) render a `×N` badge. Only a repeated
**reasoning** run — a *non-deferral, non-progress* decision that repeats
`LOOP_REPEAT_THRESHOLD` times — additionally carries a subtle **⚠ possible loop**
affordance. A run of healthy **deferrals** is **never** flagged: it collapses
quietly with just its `×N` count. That distinction is the entire point of the
feature (issue #2580) — a correct no-action deferral to a live engineer must not
be dressed up as a stuck loop.

## Collapse / dedup semantics

The Cycle History collapses **consecutive equivalent cycles** into one row so
that a stuck loop of near-identical cycles does not drown out genuine forward
progress. This is the behaviour the tab's own description has always promised —
"repeated deferring-to-an-active-engineer notes are collapsed with a count so
genuine forward progress stands out from a stuck loop."

Collapse runs at the **display layer only** (`thinking_collapse.rs`); it never
touches the OODA reasoner or the persisted cycle reports.

### Rules

- Only **consecutive** cycles that share a grouping key merge. A single
  non-matching cycle in between breaks the run.
- A run of length **1** renders as an ordinary row (`#N`, no `×N` suffix).
- A run of length **N ≥ 2** renders as a single row labelled **`×N (cycles #A–#B)`**,
  where `A` is the oldest and `B` is the newest cycle number in the run
  (ascending: `A = min`, `B = max`). This matches the second half's existing
  `Cycles #A–#B` convention.
- The row's representative **Time** is the timestamp of the most-recent cycle in
  the run.
- **Progressing** cycles (those that launched work or produced an artifact —
  `pr #`, `commit`, `launched`, `dispatched`, or a live spawned engineer) are
  **never** collapsed together; each distinct forward step keeps its own row.
- **Deferring** cycles (a deliberate no-action deferral to an already-active,
  healthy engineer) collapse by the goal set they defer on.
- **Reasoning** cycles (anything else) collapse by their *normalized decision
  text* (see below). A non-progressing reasoning decision that repeats
  `LOOP_REPEAT_THRESHOLD` (3) or more times is additionally flagged
  `loop_suspected`.

### Collapse modes: `Strict` vs `Relaxed`

`thinking_collapse.rs` exposes two collapse modes:

| Mode | Used by | Grouping of `Reasoning` cycles |
|------|---------|--------------------------------|
| `Strict` | Second half (`/api/ooda-thinking`, `collapse_reports`) | Exact normalized decision text (byte-for-byte prior behaviour). |
| `Relaxed` | First half (`/api/ooda-cycles`, `collapse_reports_with(reports, Relaxed)`) | Decision text with every run of ASCII digits masked to a single `#`, so the cycle number and volatile counts (`Cycle #1040 — 3 priorities…`) stop distinguishing otherwise-identical decisions. |

`collapse_reports(reports)` is preserved as an exact alias for
`collapse_reports_with(reports, Strict)`, so the second half and all existing
`thinking_collapse` tests are unaffected.

> **Deferral summary phrasing is mode-specific.** The difference-carrying
> `no-action: deferring to active engineer on <goal>` phrasing described in
> [Meaningful row summaries](#meaningful-row-summaries) is emitted **only** in
> `Relaxed` mode (the first half). `Strict` mode retains the exact legacy
> deferral summary `Deferring to an active engineer on <goal>` (with the
> `(repeated N cycles)` suffix). An existing `thinking_collapse` test asserts
> that legacy string byte-for-byte, so the `Strict` path — and therefore the
> preserved second half — must keep it unchanged.

**Why `Relaxed` for the first half.** Production cycle summaries such as

```
Cycle #1039 — 3 priorities considered, 2 of 2 actions succeeded · 2 goals tracked · 20 open issues · working tree clean
Cycle #1040 — 3 priorities considered, 2 of 2 actions succeeded · 2 goals tracked · 20 open issues · working tree clean
```

differ only in the cycle number and volatile counts. Under `Strict` they never
match, so dozens of near-identical rows are listed individually. `Relaxed`
normalizes those cosmetic digit-runs to `#`, so the two lines above share a key
and collapse to a single `×2 (cycles #1039–#1040)` row — while a cycle whose decision
*text* genuinely differs (a different action, a different goal) still keeps its
own row. Digit masking only merges cosmetic numeric variance; it never merges
two cycles that decided different things.

### Meaningful row summaries

The old boilerplate summary ("*N priorities considered, M of M actions
succeeded · … · working tree clean*") is replaced, in the first-half (`Relaxed`)
path, by a **difference-carrying** `collapsed_summary` chosen by disposition:

| Disposition | Summary conveys |
|-------------|-----------------|
| **Progressing** | The concrete decided action — the outcome `action_description`, else its `detail`, else a planned-action `description` (e.g. `opened PR #204`). |
| **Deferring** | `no-action: deferring to active engineer on <goal>` (lists the goals). |
| **Reasoning** | The concrete decision text from the cycle's outcome (`action_description`, else `detail`, else a planned-action `description`); falls back to `reasoning cycle (no action selected)` when the cycle carries no action text. |

`collapsed_summary` is **always non-empty** — the blank / boilerplate cell is
gone. The transform is server-side. The first-half renderer displays
`collapsed_summary` **directly** (falling back to the humanized `summary` only
when a row predates the field); the shared client helper `humanizeCycleSummary`
is itself **unchanged** and still serves other surfaces.

## Duration trend chart

The Cycle History renders a small inline SVG bar-and-line chart of per-cycle
duration, plus a text trend (`↓ Improving` / `↑ Degrading` / `→ Stable`).

- The trend and chart are computed from the **uncollapsed** per-cycle duration
  series, so collapsing rows never starves the chart of data points.
- **The duration widget (trend verdict + chart) renders only once at least
  4 cycles in the window carry a numeric `duration_secs`.** Below that
  threshold the whole widget is **absent** — no chart and no trend arrow — and
  only a plain `N cycles recorded` line remains. The permanently-stuck
  "Not enough data / Need at least 4 cycles" placeholder no longer appears.
- Legacy cycles with no recorded duration (for example a fresh state root) are
  excluded from the series; once **4** duration-bearing cycles exist, both the
  chart and the `↓ Improving` / `↑ Degrading` / `→ Stable` verdict appear.

Per-cycle duration is populated by the producer (see
[Producer telemetry](#producer-telemetry-timestamp-duration)); it is recorded
in seconds as an `f64` (via `Duration::as_secs_f64`), so sub-second cycles keep
their fractional value rather than truncating to `0`.

## API: `GET /api/ooda-cycles`

Returns the most recent cycles (up to `MAX_CYCLES = 50`), newest-first,
**after** relaxed collapse. Auth-gated like every other dashboard API.

### Response shape

```jsonc
{
  "cycles": [
    {
      "cycle_number": 1040,          // representative (newest cycle in the run)
      "cycle_number_first": 1040,    // newest in the run
      "cycle_number_last": 1039,     // oldest in the run
      "repeat_count": 2,             // number of cycles collapsed into this row
      "disposition": "deferring",    // "progressing" | "deferring" | "reasoning"
      "collapsed_summary": "no-action: deferring to active engineer on adopt-tdd",
      // "loop_suspected": true      // emitted ONLY on a reasoning run that repeats
      //                             // >= LOOP_REPEAT_THRESHOLD; absent otherwise
      //                             // (deferrals are never flagged)
      "phase": "act",                // final OODA phase: observe|orient|decide|act
      "duration_secs": 12.4,         // f64 seconds, or null when unavailable
      "action_count": 0,
      "actions_taken": [],
      "timestamp": "2026-07-06T04:12:33Z"   // RFC3339; empty/absent only for legacy cycles
    }
  ],
  "total_cycles": 1,
  "duration_trend": {
    "direction": "improving",        // improving | degrading | stable | insufficient_data
    "recent_avg_secs": 10.2,
    "older_avg_secs": 18.7,
    "change_pct": -45.5
  },
  "timestamp": "2026-07-06T04:31:28Z"
}
```

### Field notes

- `cycles` is **collapsed** (relaxed). A single cycle has `repeat_count: 1` and
  `cycle_number_first == cycle_number_last == cycle_number`.
- `total_cycles` is the number of **raw** cycles read *before* collapse, so the
  renderer's "N cycles recorded" line reflects real activity rather than the
  (smaller) collapsed-row count.
- `duration_trend.direction` is one of `improving | degrading | stable |
  insufficient_data`; on `insufficient_data` (fewer than 4 duration-bearing
  cycles) the object also carries a `detail` string explaining why. The
  renderer hides the **entire** duration widget (chart + verdict) while the
  direction is `insufficient_data`, leaving only a plain `N cycles recorded`
  line — never a broken placeholder.
- `duration_secs` is `null` for legacy cycles; such cycles are excluded from the
  duration series used to compute the trend. The endpoint reads the producer's
  `duration_secs` field first, falling back to a legacy `cycle_duration_secs`
  key when present, so cycles persisted under the older key still populate the
  chart and trend.
- `timestamp` is `""`/absent only for legacy cycle reports written before
  timestamp telemetry existed; the renderer shows `—` for those. This is the
  **only** case in which the Time column shows `—`.

## Producer telemetry: timestamp + duration

Cycle timestamps and durations originate at the **producer**, not in the
`CycleReport` decision struct. The persisted `cycle_reports/cycle_<N>.json`
carries two top-level fields alongside the existing OODA report:

| Field | Type | Meaning |
|-------|------|---------|
| `timestamp` | RFC3339 string | When the cycle report was persisted (end of cycle). Always written for new cycles. |
| `duration_secs` | `f64` or `null` | Wall-clock duration of the cycle in seconds. Populated for daemon-produced cycles; `null` when the caller did not supply an elapsed time. |

New cycles are written with `duration_secs`. The `/api/ooda-cycles` endpoint
also accepts a legacy `cycle_duration_secs` key as a fallback, so reports
persisted by older builds under that name still surface a duration.

These fields live in the persisted JSON only — the `CycleReport` struct and its
serialization are unchanged, so the OODA decision contract is untouched and
legacy on-disk reports (which lack the fields) still deserialize and render.

The daemon cycle loop measures the cycle's elapsed time and passes it through
when persisting the report, so the dashboard's duration chart is fed by real
per-cycle wall-clock data. Callers that do not measure elapsed time (for
example non-daemon test paths) still get a `timestamp` and a `null`
`duration_secs`.

## Second half preserved: `GET /api/ooda-thinking`

The second-half OODA reasoning breakdown is **behaviourally and visually
unchanged** by the Cycle History fix:

- `/api/ooda-thinking` still returns `{ "reports": [...] }` from
  `collapse_reports` (**strict** mode).
- `fetchThinking` still renders the Observe / Orient / Decide / Act phase blocks,
  the `deferring ×N` / `progress` badges, the `⚠ possible loop` flag, the
  `spawn_engineer` block, and the per-outcome artifact/assessment badges exactly
  as before.
- The strict collapse path is guarded by a regression test asserting it is
  byte-identical to the pre-change `collapse_reports`.

## Examples

### A run of healthy deferrals collapses to one row (no loop warning)

Fifty consecutive cycles that all defer to the same healthy engineer render as a
single row instead of fifty near-identical lines. Because a deferral is a
*correct* no-action, the row carries **only** its `×50` count — **no** loop
warning:

```
#                        | Phase | Duration | Actions | Summary                                              | Time
×50 (cycles #1041–#1090) | act   | 11.9s    | 0       | no-action: deferring to active engineer on adopt-tdd | 2m ago
```

### A repeated reasoning decision is flagged as a possible loop

A *reasoning* decision (not a deferral, not progress) that repeats
`LOOP_REPEAT_THRESHOLD` (3) or more times is the genuine stuck-loop case, and it
alone gets the **⚠ possible loop** affordance:

```
#                       | Phase  | Duration | Actions | Summary                                      | Time
×3 (cycles #0205–#0207) | orient | 4.0s     | 0       | re-ranking the same backlog with no decision | 5m ago  ⚠ possible loop
```

### Genuine progress is never hidden

Two cycles that each launched a different engineer stay as two rows:

```
#1092 | act | 34.1s | 1 | launched engineer for improve-meeting-ux          | just now
#1091 | act | 28.7s | 1 | opened PR #204                                    | 1m ago
```

### Reading the JSON directly

```bash
# Requires an authenticated session cookie (see dashboard login).
curl -s --cookie "$COOKIE" http://localhost:8080/api/ooda-cycles | jq '.cycles[0]'
```

```json
{
  "cycle_number": 1090,
  "cycle_number_first": 1090,
  "cycle_number_last": 1041,
  "repeat_count": 50,
  "disposition": "deferring",
  "collapsed_summary": "no-action: deferring to active engineer on adopt-tdd",
  "phase": "act",
  "duration_secs": 11.9,
  "action_count": 0,
  "timestamp": "2026-07-06T04:29:02Z"
}
```

## Configuration

The Cycle History has no operator-facing configuration; it reads whatever the
daemon has persisted under the resolved state root. Two internal constants
govern behaviour:

| Constant | File | Value | Meaning |
|----------|------|-------|---------|
| `MAX_CYCLES` | `ooda_cycles.rs` | `50` | Most-recent cycles read before collapse. |
| `LOOP_REPEAT_THRESHOLD` | `thinking_collapse.rs` | `3` | Repeats of a non-progress reasoning decision before `loop_suspected` is set. |

## Tests

The behaviour is covered by hermetic tests (temp dirs / fixtures, no network,
no daemon):

- **Collapse (relaxed):** N consecutive equivalent cycles collapse to exactly
  one row with `repeat_count = N` and the `#A–#B` range; distinct/progressing
  cycles stay separate.
- **Collapse (strict) regression:** `collapse_reports_with(reports, Strict)` is
  byte-identical to the legacy `collapse_reports` on a fixture — the guard that
  proves the second half is preserved.
- **Normalization:** each run of ASCII digits masks to a single `#` (so the
  cycle number and volatile counts stop distinguishing otherwise-identical
  decisions); genuinely different decision text stays distinct.
- **Summaries:** `collapsed_summary` is non-empty for every disposition and
  carries the concrete action / no-action reason (no count boilerplate).
- **Timestamps:** rows render real timestamps when present; `—` only when a
  timestamp is genuinely absent (legacy).
- **Duration chart gate:** the whole duration widget (chart + trend verdict)
  renders only once **≥4** cycles carry a numeric `duration_secs`; below that
  the trend reads `insufficient_data`, the widget is absent from the DOM, and
  only a plain `N cycles recorded` line remains. The trend is computed from the
  **uncollapsed** duration series (collapsing rows never starves it), and rows
  with no recorded duration carry a `null` `duration_secs`.
- **Producer:** the persisted `cycle_<N>.json` contains `timestamp` (RFC3339)
  and `duration_secs` (`f64` or `null`).
- **Second half unchanged:** the `/api/ooda-thinking` path and `fetchThinking`
  rendering are asserted untouched.

## Related

- [Dashboard](../dashboard.md) — the Thinking tab in context.
- [Overview action-detail humanization](./dashboard-action-detail-humanization.md) — the sibling render-layer humanizer.
- [OODA brain decision protocol](./ooda-brain-decision-protocol.md) — how cycle decisions are produced.
- [Telemetry & metrics](./telemetry-metrics.md) — related dashboard telemetry surfaces.
