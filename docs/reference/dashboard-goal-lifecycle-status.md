---
title: Goals tab lifecycle-status badges
description: Reference for the Goals tab Status column — the per-goal lifecycle badge that renders each active goal's real, live status (Not started, In progress, Blocked + reason, Completed, Proposed, Paused) distinctly, driven by the additive status_progress field on /api/goals.
last_updated: 2026-07-16
owner: simard
doc_type: reference
related:
  - ../dashboard.md
  - ./goal-board-api.md
  - ./dashboard-action-detail-humanization.md
  - ../howto/unblock-stuck-ooda-goals.md
---

# Goals tab lifecycle-status badges

Reference documentation for the **Status** column of the dashboard **Goals**
tab. Each active goal now renders an authoritative, distinctly-coloured
**lifecycle badge** that reflects that goal's *real, live* status —
**Not started**, **In progress**, **Blocked** (with its block reason),
**Completed**, **Proposed**, or **Paused** — instead of collapsing every goal
into a single uniform "failed/blocked" appearance
([#20](https://github.com/rysweet/Simard/issues/20)).

## Why this exists

The 20-slot active goal register is almost always in **mixed** states: some
goals are `blocked` (for example the OODA-safeguard hold —
`🔒 [OODA-SAFEGUARD] … 3 consecutive no-action cycles; needs human review`),
many are `not-started`, several are `in-progress`, and several are `completed`.
`simard goal list` reports those mixed states correctly.

Previously the **Goals** tab did **not** reflect them: the prominent coloured
signal in each row was the **Current Activity** *activity chip* (`Working`,
`Skipped`, `Failed`, `Waiting`, `Spawned engineer`), and the Status column
merely printed the raw `status` string. Operators read the activity chip — and
its red `Failed` state — as the goal's lifecycle status, so the whole board
looked as though every goal had failed or was blocked. That conflated two
different axes:

- **Lifecycle status** — where a goal sits in its life (not-started → in-progress
  → completed, or blocked / paused). This is `GoalProgress`.
- **Current activity** — what the daemon did on its *most recent* touch of the
  goal (worked, skipped, spawned an engineer, …). This is the activity chip.

The fix separates the two. The **Status** column is now the authoritative
lifecycle indicator; the **Current Activity** column keeps the activity chip
unchanged. A goal that is simply *not started* no longer looks *failed*, and a
genuinely *blocked* goal shows **why**.

## What the operator sees

Active-goals table, **Status** column:

| Goal lifecycle | Badge label | Badge colour |
|----------------|-------------|--------------|
| `NotStarted` | **Not started** | neutral grey `#8b949e` |
| `Proposed` | **Proposed** | neutral grey `#8b949e` |
| `InProgress { percent }` | **In progress — 42%** | accent `var(--accent)` |
| `Blocked(reason)` | **Blocked — &lt;reason&gt;** | amber `#d29922` |
| `Paused` | **Paused** | muted `#6e7681` |
| `Completed` | **Completed** | green `#2ea043` |

Key points:

- **Blocked is amber `#d29922`, deliberately different from the activity chip's
  `Failed` red `#f85149`.** A lifecycle *block* (needs human review) must not be
  mistaken for an activity *failure*.
- The **block reason is shown inline** — e.g.
  `Blocked — 🔒 [OODA-SAFEGUARD] … 3 consecutive no-action cycles; needs human
  review` — so the operator knows what to do without leaving the tab.
- Every lifecycle status is **visually distinct**, so a board with mixed states
  reads as mixed, matching `simard goal list`.

### Before / after

| Was (Status column) | Now (Status column) |
|---------------------|---------------------|
| `blocked: 🔒 [OODA-SAFEGUARD] … needs human review` (raw string, looked like every other row) | **Blocked — 🔒 [OODA-SAFEGUARD] … needs human review** (amber badge) |
| `not-started` (raw string; row looked "failed" because the activity chip was red) | **Not started** (grey badge) |
| `in-progress(42%)` (raw string) | **In progress — 42%** (accent badge) |
| `completed` (raw string) | **Completed** (green badge) |

## Live, not a stale snapshot

The Goals tab reflects the **live** goal board. `/api/goals` rebuilds its view
from the goal store on **every request** via
`dashboard_goal_board_snapshot(state_root)` in
[`goals.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/goals.rs)
— there is no cached one-time snapshot. Because both the dashboard and
`simard goal list` read the same authoritative goal store, the two **agree by
construction**: a goal that `simard goal list` reports as `blocked` renders as
an amber **Blocked** badge in the tab, and so on.

## API: the `status_progress` field

The fix is **additive**. Each active-goal object returned by `/api/goals`
carries a new `status_progress` field alongside the existing fields. It is the
**serialized `GoalProgress` enum** (the same closed, trusted internal type used
across the goal board), which is what carries the structured block reason and
progress percent.

```jsonc
{
  "active": [
    {
      "id": "audit-test-coverage",
      "description": "Audit test coverage across crates",
      "priority": 3,
      "status": "not-started",                 // existing: GoalProgress Display string
      "status_progress": "NotStarted",          // NEW: serialized GoalProgress enum
      "assigned_to": null,
      "repo": "rysweet/Simard",
      "current_activity": null,
      "status_chip": "Waiting",                 // existing: activity chip (unchanged)
      "detail": "",
      "detail_full": "",
      "wip_refs": []
    },
    {
      "id": "agent-kgpacks-rs-ws24",
      "description": "…",
      "priority": 1,
      "status": "blocked: 🔒 [OODA-SAFEGUARD] … 3 consecutive no-action cycles; needs human review",
      "status_progress": {                      // NEW: object form carries the reason
        "Blocked": "🔒 [OODA-SAFEGUARD] … 3 consecutive no-action cycles; needs human review"
      },
      "status_chip": "Waiting",
      "wip_refs": []
    }
  ],
  "backlog": [ /* … unchanged … */ ],
  "active_count": 20
}
```

### `status_progress` serialization forms

`status_progress` mirrors the `GoalProgress` enum exactly (see the
[`GoalProgress` variants](goal-board-api.md#goalprogress-variants) table):

| `GoalProgress` variant | `status_progress` JSON |
|------------------------|------------------------|
| `NotStarted` | `"NotStarted"` |
| `Proposed` | `"Proposed"` |
| `InProgress { percent }` | `{"InProgress":{"percent":42}}` |
| `Blocked(reason)` | `{"Blocked":"<reason>"}` |
| `Paused` | `"Paused"` |
| `Completed` | `"Completed"` |

### Compatibility

- **Additive only.** The existing `status` (Display string), `status_chip`,
  `detail`, `detail_full`, `current_activity`, and `wip_refs` fields are
  **unchanged**. Existing consumers keep working; `status_progress` is new.
- **No route, auth, or schema change.** `status_progress` rides the same
  authenticated `GET /api/goals` request behind `require_auth`. Nothing new is
  exposed — the block reason was already present in the `status` string.
- **No persistence change.** `status_progress` is a **response-only
  projection** of the in-memory `GoalProgress`; the on-disk goal store format,
  the `GoalProgress` variants, and any migration path are untouched.

## Rendering pipeline (front end)

The Status column is rendered client-side in
[`index_html`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/index_html/)
from three cooperating pieces:

1. **`humanizeGoalProgress(status_progress)`** *(existing, reused)* — turns the
   serialized enum into a plain-text label: `"NotStarted"` → `Not started`,
   `{"InProgress":{"percent":42}}` → `In progress — 42%`,
   `{"Blocked":"<reason>"}` → `Blocked — <reason>`, `"Completed"` →
   `Completed`, and so on. Returns **plain text only** (escape-last invariant).

2. **`goalLifecycleKey(status_progress)`** *(the classifier)* — maps the
   serialized enum to one canonical colour key —
   `blocked` · `completed` · `in-progress` · `not-started` · `proposed` ·
   `paused` — **by the enum variant name only**. It dispatches on the two serde
   shapes that `GoalProgress` produces:

   - **String forms** (`"NotStarted"`, `"Proposed"`, `"Paused"`, `"Completed"`,
     and the legacy `"Done"`) are the variant name itself, so the classifier
     keys **directly off the string**.
   - **Object forms** (`{"Blocked":"<reason>"}`,
     `{"InProgress":{"percent":42}}`) wrap the variant name as their sole key,
     so the classifier reads **`Object.keys(status_progress)[0]`** — the variant
     name — and never the wrapped payload.

   In both cases the variant name is lower-cased/kebab-mapped to the colour key
   (`Blocked`→`blocked`, `InProgress`→`in-progress`, `NotStarted`→`not-started`,
   …). Any unrecognized value falls through to `not-started`. Because it keys on
   the variant name only, it never inspects the block-reason text or the percent
   value, so a hostile or unusual reason string cannot change which colour is
   chosen (guideline **G3**: classify structured data by its enum, not by
   parsing a Display string).

3. **`GOAL_STATUS_COLORS`** *(the allowlist)* — a hard-coded map from colour key
   to colour:

   | Key | Colour |
   |-----|--------|
   | `blocked` | `#d29922` (amber) |
   | `completed` | `#2ea043` (green) |
   | `in-progress` | `var(--accent)` |
   | `not-started` | `#8b949e` (grey) |
   | `proposed` | `#8b949e` (grey) |
   | `paused` | `#6e7681` (muted) |

The Status cell renders a coloured badge whose **label** is
`esc(humanizeGoalProgress(g.status_progress))` and whose **colour** is
`GOAL_STATUS_COLORS[goalLifecycleKey(g.status_progress)]`. The fallback to the
legacy `esc(g.status)` string triggers **only when `g.status_progress == null`**
(field absent or explicitly null in older payloads) — a `== null` test, not a
falsiness test. This mirrors `humanizeGoalProgress`, whose own guard is
`if (status == null) return ''`: any present `status_progress` value (including
an object form) is truthy and is always rendered through the humanizer +
classifier path rather than falling back.

The **Current Activity** column is unchanged: it still shows the activity chip
(`Working` / `Skipped` / `Failed` / `Waiting` / `Spawned engineer`) plus any
WIP references. The activity chip is **not** the page's status indicator.

### Security invariants

- **Output encoding, escape-last.** The badge label is
  `esc(humanizeGoalProgress(...))` with `esc()` applied **last**, so an attacker
  who controls a block reason cannot inject markup.
- **Colour from the allowlist only.** Badge colour comes exclusively from the
  hard-coded `GOAL_STATUS_COLORS` map keyed by `goalLifecycleKey`. Goal data is
  **never** interpolated into a `style=` sink, so a reason string containing
  `"` or `;` cannot break out of the badge's style or recolour it.
- **Structured over brittle (G3).** Colour and layout decisions read the closed
  `GoalProgress` enum key, not the free-form `status` Display string. The
  percent is used for the label text only, never for badge geometry.

## Tests

Hermetic tests pin the behaviour end to end:

- **API (`tests_goals_crud.rs`)** — given active goals with lifecycle
  `{NotStarted, InProgress{percent}, Blocked(reason), Completed}`, `/api/goals`
  exposes each goal's distinct `status_progress`, and the `Blocked` entry
  carries its reason string. Existing `status_chip == "Working"` assertions stay
  green (additive change).
- **Rendering (`index_html/tests_tab_meta.rs`)** — `INDEX_HTML` contains
  `humanizeGoalProgress(g.status_progress)`, defines `goalLifecycleKey(`, and
  includes the amber blocked colour `#d29922` **distinct from** the activity
  `Failed` red `#f85149` — proving the Status column renders per-status,
  non-uniform lifecycle badges rather than one blanket "failed/blocked" look.
- **Reconciliation** — a test asserts the rendered lifecycle statuses match the
  underlying `GoalProgress` values, i.e. the dashboard view and the goal store
  agree.

## Work Board sub-section (kanban) — blocked ≠ failed

The retired standalone *Work Board* view now renders as a **Work Board**
sub-section inside the Goals tab, showing a small kanban (`Queued`,
`In progress`, `Blocked`, `Done`) fed by `GET /api/workboard`. Its cards had
the *same* blocked-vs-failed confusion the Status column fixed, in a different
place ([#4178](https://github.com/rysweet/Simard/issues/4178)):

- The **Blocked** column card coloured a blocked goal's progress bar with the
  activity-failure red `var(--red)` (`#f85149`) — so a blocked goal on the
  kanban read as *failed*, contradicting the amber decision above.
- The card never surfaced **why** the goal was blocked; the reason was only
  reachable by opening the Status column.

The fix brings the Work Board into line with the Status column:

- The blocked card's progress bar now uses amber `var(--yellow)` (`#d29922`),
  the same colour as the **Blocked** lifecycle badge and deliberately distinct
  from the activity-`Failed` red.
- The card renders an inline **`Blocked — <reason>`** row (escape-last:
  `esc(reason)`), so the operator sees the block reason without leaving the
  kanban.

### API: the `block_reason` field on `/api/workboard`

The fix is **additive**. Each blocked goal object returned by `/api/workboard`
now carries a clean, prefix-free `block_reason` string alongside the existing
fields. The legacy `status` field is **unchanged** — it keeps its
`"blocked: <reason>"` shape, which the client-side kanban classifier
(`g.status.startsWith('blocked')`) still keys off — so existing consumers are
unaffected.

```jsonc
{
  "goals": [
    {
      "name": "agent-kgpacks-rs-ws24",
      "description": "…",
      "status": "blocked: 🔒 [OODA-SAFEGUARD] … needs human review", // existing, unchanged
      "block_reason": "🔒 [OODA-SAFEGUARD] … needs human review",     // NEW: clean reason
      "progress_pct": 0,
      "priority": 1,
      "assigned_to": null
    },
    {
      "name": "audit-test-coverage",
      "status": "in_progress",
      "progress_pct": 42
      // no block_reason — the field is OMITTED for every non-blocked goal
    }
  ]
}
```

`block_reason` is a **response-only projection** of the in-memory
`GoalProgress::Blocked(reason)`; it is omitted for every other lifecycle state.
The Work Board card prefers it and falls back to stripping the legacy
`blocked: ` prefix from `status` so older payloads still render a reason.

## Related

- [Dashboard](../dashboard.md) — the Goals tab in context
- [Goal board API](goal-board-api.md) — `GoalProgress` variants and the goal store
- [Overview action-detail humanization](dashboard-action-detail-humanization.md) — the sibling render-layer humanizer
- [Unblock stuck OODA goals](../howto/unblock-stuck-ooda-goals.md) — clearing an OODA-safeguard block
