---
title: Activity tab — Cycle Reports (live cycle number, accurate tree status, shared detail)
description: Reference for the Activity tab's Logs sub-section "Cycle Reports" card. Documents the finished behaviour shipped for issue #26 — the card now shows the real live OODA cycle index (never a frozen #1), the actual per-cycle working-tree status (clean vs. dirty, not a constant "uncommitted changes"), per-cycle Observe/Orient/Decide/Act detail, and consecutive-cycle collapse with a ×N repeat-count. The card reads through the single shared cycle-report reader and strict collapse pass that the Thinking tab's Agent Internal Reasoning view uses, and renders through the same shared client entry-renderer, refreshing live every 15 s — so the two views agree on the same data instead of one rendering a stale copy.
last_updated: 2026-07-06
owner: simard
doc_type: reference
related:
  - ../dashboard.md
  - ./dashboard-thinking-cycle-history.md
  - ./dashboard-action-detail-humanization.md
  - ./state-root-resolution.md
  - ./ooda-brain-decision-protocol.md
---

# Activity tab — Cycle Reports

Reference documentation for the **Activity** tab's **Logs** sub-section
**Cycle Reports** card. This page describes the finished behaviour shipped for
issue #26.

The Cycle Reports card sits in the **Logs** sub-section of the **Activity** tab,
below the Background Service Log and the Cost Ledger:

| Surface | Location | Endpoint | Renderer |
|---------|----------|----------|----------|
| **Cycle Reports** | Activity → Logs (`#cycle-reports` card in `index_html/part_00.rs`) | `GET /api/logs` → `cycle_reports` | `fetchLogs` (`index_html/part_02.rs`) via the shared `renderCycleEntry` helper |

> **Post-consolidation home.** Tab consolidation (#2641) merged the former
> standalone OODA/thinking surfaces into the **Activity** tab. The Cycle Reports
> card is the Logs-sub-section view of the same per-cycle OODA data that the
> tab's **Thinking** sub-section renders as **Agent Internal Reasoning**. Both
> now read through **one** shared reader and strict collapse pass and render
> through **one** shared entry-renderer; see
> [Shared data path](#shared-data-path-one-reader-one-collapse-one-renderer).

## What changed (issue #26)

Before this fix the Cycle Reports card rendered a **stale, deduped-nothing,
frozen** view. Operators saw the same line repeated verbatim, cycle after cycle:

```
Cycle #1 — 20 priorities considered, 6 of 6 actions succeeded · 19 goals tracked · 20 open issues · uncommitted changes
```

Two things were wrong, and both were *data* bugs, not cosmetics:

1. **The cycle number was frozen at `#1`.** The live daemon's OODA loop was
   advancing (`journalctl` showed `cycle=13` and climbing), yet the card showed
   `Cycle #1` every refresh.
2. **The tree status was a stale constant.** The working tree was clean
   (`git status --porcelain` empty), yet the card said `uncommitted changes`
   on every row.

The goals/issues counts were roughly live, so *some* fields updated while the
cycle-number and tree-status fields did not — the signature of a **stale data
path**, not a rendering-only glitch.

### Root cause

The Logs endpoint built its `cycle_reports` payload with a **third, divergent
reader** that did not match the reader the Thinking tab already used:

- it read **only** `state_root/cycle_reports/` and never the daemon's live
  `state_root/state/cycle_reports/` directory, so on a host where the live
  daemon writes to the `state/` sub-path it kept serving an ancient seed file
  (`cycle_1.json`) — hence the frozen `#1` and its stale `tree=dirty`;
- it sorted **oldest-first** and read the directory **unbounded** with **no
  newest-first limit**, so the oldest report led the list; and
- it applied **no dedup / collapse**, so a run of equivalent cycles rendered as
  a wall of identical lines.

Meanwhile the Thinking tab's Agent Internal Reasoning view (#2580) already read
the **live** reports correctly, collapsed them, and rendered real per-cycle
detail. The Activity Cycle Reports card simply had not been moved onto that same
path.

### After the fix

The Cycle Reports card now:

1. shows the **real, live cycle index** of each report (e.g. `#13`, `#14`, …),
   never a frozen `#1`;
2. shows each cycle's **actual working-tree status** — **working tree clean**
   when the tree was clean, **uncommitted changes** only when it truly was
   dirty;
3. renders **per-cycle detail** — the observed signals, the decided action(s),
   and the outcome — reusing the same Observe/Orient/Decide/Act render the
   Thinking tab uses, so an operator can see what changed cycle-to-cycle;
4. **collapses** runs of genuinely-equivalent cycles into a single row with a
   `×N` repeat-count and cycle range `#A–#B`, so forward progress stands out
   from a stuck loop; and
5. **refreshes live** every 15 s with the rest of the Logs sub-section, so new
   cycles appear as they happen instead of a one-time stale snapshot.

## Live cycle number

Each report carries and displays the **real** OODA cycle index taken from the
authoritative persisted filename `cycle_<N>.json`, newest-first. The shared
reader stamps each report's `cycle_number` with the **filename** index — the
persisted cumulative number — overwriting the in-body counter, which resets to
`1` on every daemon restart (#1680) and was the frozen `#1` the operator saw.
Because the shared reader scans **both** report directories and orders by
descending cycle number, the newest live cycle always leads the card.

> The filename `N` in `cycle_<N>.json` is the persisted source of truth for a
> cycle's number — the same value the Thinking tab and the Recent Actions feed
> display. Using it keeps every surface in agreement and is what fixes the
> frozen `#1` (see also
> [Cycle-number reconciliation](#relationship-to-the-authoritative-cycle-counter)).

## Accurate tree status

Each row reflects **that cycle's** working-tree state at the time the cycle ran,
read from the report's own `observation.environment.git_status`:

| Tree state at cycle time | Rendered as |
|--------------------------|-------------|
| clean (empty `git_status`) | **(clean)** in the Observe line |
| dirty (non-empty `git_status`) | the recent-commits/issues line without the `(clean)` marker |

The value is per-report, so it changes row to row exactly as the real tree did.
Because the card reads the **newest live** reports (not a frozen old file), a
clean cycle now renders as clean instead of a stale constant "uncommitted
changes". When a cycle carries the canonical one-line summary
(`… tree=clean` / `… tree=dirty`), the shared `humanizeCycleSummary` helper
rewrites `tree=clean → working tree clean` and `tree=dirty → uncommitted
changes` in the header summary, unchanged.

## Per-cycle detail

Each entry conveys **what actually happened that cycle**, not just aggregate
counts. The card reuses the Thinking tab's phase renderer so a report's detail
appears as:

- **Observe** — goals tracked, environment signals (open issues, recent
  commits, tree status).
- **Orient** — the prioritised goals with their urgency and reason.
- **Decide** — the planned action(s) (`kind`, target goal, description).
- **Act** — the outcomes, including the launched-sub-agent block, the
  produced-artifact marker (`🔗` for `PR #…` / `commit`), and success/failure.

For a collapsed run, the newest cycle in the run is the representative whose
detail is shown, and the row is labelled with its `×N` count and cycle range.

This is the same Observe/Orient/Decide/Act detail the Thinking sub-section's
Agent Internal Reasoning timeline renders; the Activity card renders it through
the **same shared client helper** (`renderCycleEntry`, extracted from that
timeline) rather than a second copy. See also
[Thinking tab — Cycle History](./dashboard-thinking-cycle-history.md).

## Dedup / collapse

Consecutive equivalent cycles collapse into one row, using the **same**
display-layer collapse the Agent Internal Reasoning view uses —
`thinking_collapse.rs`, `collapse_reports` (`CollapseMode::Strict`). The rules
are those of #2580:

- Only **consecutive** cycles that share a grouping key merge; one non-matching
  cycle breaks the run.
- A run of length **1** renders as an ordinary row (`#N`, no `×N`).
- A run of length **N ≥ 2** renders as one row labelled **`×N (cycles #A–#B)`**
  (`A` oldest, `B` newest).
- **Progressing** cycles (launched work / produced an artifact) are **never**
  collapsed together — each distinct step keeps its own row.
- **Deferring** cycles (a deliberate no-action deferral to an already-active,
  healthy engineer) collapse by the goal set they defer on and are **never**
  flagged as a loop — they carry only their `×N` count.
- **Reasoning** cycles collapse by their (verbatim) decision text; a
  non-progress reasoning decision that repeats `LOOP_REPEAT_THRESHOLD` (3) or
  more times additionally carries the **⚠ possible loop** affordance.

Collapse runs at the **display layer only**; it never touches the OODA reasoner
or the persisted reports. See
[Collapse / dedup semantics](./dashboard-thinking-cycle-history.md#collapse-dedup-semantics)
for the full rules — the Cycle Reports card reuses them verbatim.

## Live refresh

The Cycle Reports card refreshes with the rest of the **Logs** sub-section. When
the **Activity** tab is active the client polls `fetchLogs()` on a **15 s**
interval (`tabRefreshTimers.logs = setInterval(fetchLogs, 15000)`), and each
poll re-renders the card from the freshly-read reports. New cycles therefore
appear within ~15 s of being persisted — the card is never a one-time snapshot.
The **Refresh** button on the Background Service Log panel also triggers an
immediate `fetchLogs()`.

## Shared data path: one reader, one collapse, one renderer

The fix removes the divergent third reader. The Activity **Cycle Reports** card
and the Thinking **Agent Internal Reasoning** view now flow through the same
building blocks:

| Stage | Shared component | File |
|-------|------------------|------|
| **Read** persisted reports (both dirs, newest-first, bounded) then stamp the authoritative filename cycle number, then collapse | `cycle_source::read_cycle_reports_collapsed(state_root)` | `cycle_source.rs` |
| ↳ raw newest-first union of both persisted dirs | `read_recent_cycle_reports(state_root, n)` | `current_work.rs` |
| ↳ **Collapse** consecutive equivalent cycles | `collapse_reports(reports)` (strict) | `thinking_collapse.rs` |
| **Render** one collapsed entry (badge, `×N` + range, O/O/D/A detail) | shared client entry-renderer `renderCycleEntry`, **extracted** from the Agent Internal Reasoning timeline | `index_html/part_04.rs` → called by `fetchThinking` (part_04) and `fetchLogs` (part_02) |

- `read_cycle_reports_collapsed` is the **single** reader behind both endpoints.
  Internally it calls `read_recent_cycle_reports`, which scans **both**
  `state_root/cycle_reports/` and `state_root/state/cycle_reports/`, orders by
  **descending** cycle number, and reads only the newest `n` (the same union the
  Recent Actions feed uses). Reading both directories is a legitimate **union of
  the two real persistence locations**, not a silent fallback — a cycle written
  to either directory is live data. The shared reader then unwraps each report to
  the raw Thinking-tab shape and stamps `cycle_number` from the `cycle_<N>.json`
  filename before collapsing.
- `collapse_reports(reports)` is the strict collapse
  (`collapse_reports_with(reports, Strict)`) that the Agent Internal Reasoning
  half already used, so its output is byte-for-byte unchanged.
- The client renders a collapsed entry through **one** shared helper. Previously
  the Agent Internal Reasoning entry-renderer (badge, `×N` + range, O/O/D/A
  detail) lived **inline** in `fetchThinking` (`part_04.rs`), while `fetchLogs`
  (`part_02.rs`) rendered only a bare `Cycle #<n>` header plus a summary line —
  no badge, no collapse, no detail. The fix **extracts** that entry-renderer into
  the shared `renderCycleEntry` helper and has both `fetchThinking` and
  `fetchLogs` call it, so no duplicated or divergent cycle-report rendering
  remains.

Because `/api/logs` → `cycle_reports` and `/api/ooda-thinking` → `reports` are
**both** `read_cycle_reports_collapsed(state_root)`, the two views **agree on the
same data** by construction. Fixing the reader once fixes both.

### Build scope

The Thinking tab already read, collapsed, and rendered these reports correctly,
so this fix is a **consolidation**, not new behaviour. Two pieces that were
duplicated or private were made genuinely shared:

1. **Server — one shared reader.** A new `cycle_source::read_cycle_reports_collapsed`
   wraps the existing both-dir union reader `read_recent_cycle_reports`, stamps
   the authoritative filename cycle number, and applies the strict
   `collapse_reports`. The bound `MAX_CYCLE_REPORTS` (`50`) lives beside it in
   `cycle_source.rs`. Both `/api/logs` and `/api/ooda-thinking` now call this one
   function; the Logs endpoint's old divergent reader (top-level dir only, lexical
   path sort, unbounded, no collapse) and the `ooda_thinking` inline reader are
   **removed** in favour of it.
2. **Client — extract the entry-renderer.** The Agent Internal Reasoning
   entry-renderer (badge, `×N` + range, O/O/D/A detail) was written **inline** in
   `fetchThinking` (`part_04.rs`); `fetchLogs` (`part_02.rs`) previously emitted
   only a bare `Cycle #<n>` header + summary. The fix **extracts** that renderer
   into the shared `renderCycleEntry` helper and has both callers use it.

No OODA reasoner or persistence behaviour changes, and the strict-collapse output
(`collapse_reports_with(_, Strict)`) is untouched. The `/api/ooda-cycles` Cycle
History table (relaxed collapse + duration trend) is a **separate** sibling
surface and is not modified by this fix.

### Relationship to the authoritative cycle counter

The single "Cycle #N" **counter** rendered on Overview / Whiteboard / System
Status is computed by `cycle_source::authoritative_cycle_number` — the max of
the process-local health counter and the highest persisted cycle number (#1680).
The Cycle Reports card does **not** re-derive a single counter; it lists **each
report's own** cycle number straight from its `cycle_<N>.json` filename via the
shared reader. Both approaches trust the persisted filename as the source of
truth, so the per-report numbers in the card are always consistent with the
single authoritative counter shown elsewhere.

> **Now durable at the source.** The `health_cycle_number` input to that `max()`
> is itself the **brain-relative** durable counter — `daemon_health.json`'s
> `cycle_number` is written from the persisted `PersistentGoalState.cycle_count`,
> which continues across daemon restarts instead of resetting to `1`. So the two
> inputs to `authoritative_cycle_number` now **agree** rather than one
> contradicting the other; the `max()` remains as a defensive safety net. See the
> [Durable OODA cycle counter API reference](./durable-ooda-cycle-counter.md) and
> [Brain-relative OODA cycle counter](../concepts/brain-relative-ooda-cycle-counter.md).

## API: `GET /api/logs` → `cycle_reports`

The Logs endpoint returns the recent cycle reports under the `cycle_reports`
key, newest-first, **after** strict collapse — the same element shape
`/api/ooda-thinking` exposes under `reports`. Auth-gated like every other
dashboard API.

### Response shape (cycle_reports element)

```jsonc
{
  "daemon_log_lines": [ "…" ],
  "daemon_log_levels": [ "info" ],
  "ooda_transcripts": [ /* … */ ],
  "terminal_transcripts": [ /* … */ ],
  "cost_log_lines": [ "…" ],
  "cycle_reports": [
    {
      "cycle_number": 14,            // representative (newest cycle in the run),
                                     // stamped from the cycle_<N>.json filename
      "cycle_number_first": 14,      // newest in the run
      "cycle_number_last": 13,       // oldest in the run
      "repeat_count": 2,             // cycles collapsed into this row
      "disposition": "deferring",    // "progressing" | "deferring" | "reasoning"
      "deferring_to": [ "adopt-tdd" ],        // deferring rows only
      "collapsed_summary": "Deferring to an active engineer on adopt-tdd (repeated 2 cycles)",
                                     // deferring rows only (strict phrasing)
      // "loop_suspected": true      // ONLY on a reasoning run repeating
      //                             // >= LOOP_REPEAT_THRESHOLD; absent otherwise
      //                             // (deferrals are never flagged)
      "summary": "OODA cycle #14: …, tree=clean",  // raw report summary (humanized on render)
      "observation": { /* goals, environment (git_status, open_issues, …) */ },
      "priorities": [ /* prioritised goals with urgency + reason */ ],
      "planned_actions": [ /* decided actions */ ],
      "outcomes": [ /* per-action results incl. spawn_engineer block */ ]
    }
  ],
  "timestamp": "2026-07-06T14:31:28Z"
}
```

### Field notes

- `cycle_reports` is **collapsed** (strict) and **newest-first**. A single
  cycle has `repeat_count: 1` and
  `cycle_number_first == cycle_number_last == cycle_number`.
- `cycle_number*` come from the persisted `cycle_<N>.json` filename — the live
  index. There is no frozen `#1`.
- `disposition`, `repeat_count`, and `cycle_number_first`/`_last` are added to
  **every** row by the shared collapse pass. `deferring_to` and
  `collapsed_summary` are added only on **deferring** rows; `loop_suspected` only
  on a flagged **reasoning** run. Progressing rows carry no `collapsed_summary`
  and render their humanized `summary` plus the O/O/D/A phase detail.
- The tree status for a cycle is carried inside `observation.environment`
  (`git_status`); the renderer shows `(clean)` when it is empty.
- Any `summary`, `timestamp`, or `duration_secs` on a row are the **raw
  report's** own persisted fields, passed through unchanged — the collapse pass
  neither adds nor requires them.
- The top-level `timestamp` in the payload is the endpoint's render time (for
  the live-refresh contract); each report's own `timestamp`, when present, is
  when that cycle was persisted.
- Legacy plain-text `cycle_<N>.json` files (which are not JSON objects) are
  preserved: they surface with a `cycle_number`, a `summary` string, and a
  `legacy: true` flag, and are humanized for display. This is a documented legacy
  shape branch, not a silent fallback.

## Rendering

The card markup is a single `#cycle-reports` container
(`<h2>Cycle Reports</h2>` in `index_html/part_00.rs`). `fetchLogs`
(`index_html/part_02.rs`) renders each `cycle_reports` element through the
shared `renderCycleEntry` helper:

- **Header** — `Cycle #N` for a single cycle, or `Cycles #A–#B` for a collapsed
  run; a `deferring ×N` / `progress` disposition badge; and a
  `⚠ possible loop ×N` badge only on a flagged reasoning run.
- **Summary line** — for a **deferring** row, the difference-carrying
  `collapsed_summary` (`Deferring to an active engineer on <goal>`); for a
  **progressing** or **reasoning** row, the humanized `summary`, never the old
  count-boilerplate.
- **Detail** — the Observe/Orient/Decide/Act phase blocks described in
  [Per-cycle detail](#per-cycle-detail) (skipped for collapsed deferral rows,
  which show only the one-line deferral summary).

The visible text carries **no** insider jargon: the shared `humanizeCycleSummary`
helper strips the `BANNED_JARGON` terms (e.g. `OODA`,
`Observe-Orient-Decide-Act`, `spawn_engineer`) and rewrites `key=value`
shorthand into plain English, so the card reads in operator language and uses
the friendly phase labels.

## Configuration

The Cycle Reports card has no operator-facing configuration; it reads whatever
the daemon has persisted under the resolved state root (see
[State-root resolution](./state-root-resolution.md)). Behaviour is governed by
these internal constants:

| Constant | File | Value | Meaning |
|----------|------|-------|---------|
| `MAX_CYCLE_REPORTS` | `cycle_source.rs` | `50` | Most-recent cycles the shared `read_cycle_reports_collapsed` reads before collapse, for **both** `/api/logs` and `/api/ooda-thinking`. |
| `LOOP_REPEAT_THRESHOLD` | `thinking_collapse.rs` | `3` | Repeats of a non-progress reasoning decision before `loop_suspected` is set. |
| Logs auto-refresh interval | `index_html/part_01.rs` | `15000` ms | How often the active Activity tab re-polls `/api/logs`. |

## Examples

### A healthy run collapses to one live row (no loop warning)

Twelve consecutive cycles that all defer to the same healthy engineer render as
a single row carrying the **live** cycle range and a `×12` count — not twelve
identical `Cycle #1` lines, and with **no** loop warning (a deferral is a
correct no-action):

```
Cycles #2–#13   deferring ×12
Deferring to an active engineer on adopt-tdd (repeated 12 cycles)
```

### Genuine progress is never hidden

Two cycles that each launched a different engineer stay as two rows, each with
its real cycle number and its Observe/Orient/Decide/Act detail (the outcome —
`opened PR #204` — appears in the **Act** block):

```
Cycle #14   progress
Cycle #14 — 3 priorities considered, 1 of 1 actions succeeded · … · working tree clean
  ⚡ Act  ✅ AdvanceGoal — opened PR #204

Cycle #13   progress
Cycle #13 — … · working tree clean
  ⚡ Act  ✅ AdvanceGoal — launched engineer for improve-meeting-ux
```

### Reading the JSON directly

```bash
# Requires an authenticated session cookie (see dashboard login).
curl -s --cookie "$COOKIE" http://localhost:8080/api/logs | jq '.cycle_reports[0]'
```

The same reports, read from the Agent Internal Reasoning endpoint, are
identical — the two views share one reader and one collapse:

```bash
curl -s --cookie "$COOKIE" http://localhost:8080/api/ooda-thinking | jq '.reports[0]'
```

## Tests

The behaviour is covered by hermetic tests
(`operator_commands_dashboard::tests_cycle_reports_activity`, temp state root via
`HermeticState`, no network, no daemon):

- **Live cycle number.** A report renders its **real** cycle index (from the
  `cycle_<N>.json` filename), not a hard-coded `1`; when the newest live report
  is written to `state/cycle_reports/`, the card leads with that cycle rather
  than an older top-level `cycle_1.json`, and a report whose in-body counter has
  reset to `1` still displays the filename index.
- **Accurate tree status.** The rendered tree status reflects the report's
  actual `git_status` input — clean for a clean tree, dirty for a dirty one —
  never a constant, and a clean cycle carries an empty `git_status`.
- **Dedup / collapse.** N identical/no-progress cycles collapse to exactly one
  row with `repeat_count = N` and the `#A–#B` range; distinct progressing cycles
  stay separate.
- **Per-cycle detail.** The observed signals, decided action(s), and outcome
  render for a cycle (top-level `observation` / `priorities` / `planned_actions`
  / `outcomes` plus the shared `disposition`), not just aggregate counts.
- **Views agree.** The `/api/logs` `cycle_reports` array and the
  `/api/ooda-thinking` `reports` array produced from the same on-disk reports are
  **equal** — the Activity Cycle Reports card and the Thinking Agent Internal
  Reasoning view agree on the same data.
- **Strict path preserved.** `collapse_reports` remains the strict collapse, so
  the Agent Internal Reasoning summaries are unchanged
  (`tests_ooda_cycles_history::ooda_thinking_preserves_legacy_strict_deferral_summary`).
- **Tab-identity contract.** The `humanizeCycleSummary` cross-check assertions
  in `tests_tab_meta.rs` are kept in lockstep with the shared renderer.

## Related

- [Dashboard](../dashboard.md) — the Activity tab in context.
- [Thinking tab — Cycle History](./dashboard-thinking-cycle-history.md) — the
  sibling Thinking sub-section surfaces; the Agent Internal Reasoning view shares
  this card's reader, strict collapse pass, and `renderCycleEntry` detail
  renderer, while the Cycle History table is a separate relaxed view (#21).
- [Overview action-detail humanization](./dashboard-action-detail-humanization.md)
  — the sibling render-layer humanizer.
- [State-root resolution](./state-root-resolution.md) — how the reader locates
  the persisted `cycle_reports/` directories.
- [OODA brain decision protocol](./ooda-brain-decision-protocol.md) — how the
  per-cycle decisions the card renders are produced.
