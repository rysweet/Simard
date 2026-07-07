---
title: Creative Ideas tab — live view and operator controls
description: >
  How the operator dashboard's Creative Ideas tab renders the live pool of
  persisted candidate ideas (content, status, created time, reviewer/synthesis
  signals) and the three operator controls layered on top of it — Run now
  (on-demand generation), and per-idea Promote (accept, optionally route to a
  goal) and Prune (reject) — including the HTTP API, the valid-transition
  gating rules, the JSON contracts, the DOM contract used by tests, and the
  end-to-end examples (#2805).
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented — additive, back-compatible
related:
  - ../dashboard.md
  - ../reference/creative-ideas-api.md
  - ../howto/configure-creative-ideas-thread.md
  - ../design/creative-ideas-thread.md
---

# Creative Ideas tab — live view and operator controls

The **Creative Ideas** tab of the operator dashboard shows the pool of
candidate self-improvement ideas Simard generates for herself, and lets an
operator act on them directly. It has two layers:

1. **A live view** of every idea the Creative Ideas thread has generated and
   persisted — its text, its lifecycle **status**, when it was created, and the
   reviewer/synthesis signals attached to it.
2. **Three operator controls** layered on top of that view:
   - **Run now** — generate a fresh batch of ideas on demand, instead of
     waiting for the 24-hour scheduler.
   - **Promote** (per idea) — accept an idea (`AcceptedForImplementation`) and,
     by default, route it onto the goal board.
   - **Prune** (per idea) — reject an idea (`Rejected`, terminal).

Everything on this tab is driven by **live data only** — the actual ideas
persisted in the running daemon's cognitive memory. Nothing is hard-coded,
cached, or placeholder. Reads and writes flow through the **same** cognitive
memory store the daemon itself uses, so an action taken here is reflected on the
next refresh with no split-brain.

This feature is **purely additive and back-compatible**: the existing read and
search endpoints, the `CreativeIdea` model, the `IdeaStatus` state machine, and
the generator thread are unchanged. Older clients that do not know about Run
now / Promote / Prune keep working.

> **Scope note.** This page documents the operator-facing surface. For the Rust
> types, the state machine, the generator thread, and the review pipeline see
> the [Creative Ideas subsystem API reference](../reference/creative-ideas-api.md).
> For turning the background thread on/off and tuning its cadence see
> [Configure and operate the Creative Ideas thread](../howto/configure-creative-ideas-thread.md).

## Open the tab

Start the dashboard and select **Creative Ideas** from the nav, or deep-link
straight to it with the `#creative-ideas` hash:

```bash
simard dashboard serve --port=8080
# then browse to http://localhost:8080/#creative-ideas
```

> The hash router resolves the tab from its slug (`creative-ideas` is a
> canonical slug). The web dashboard has no numeric tab hotkeys; the terminal
> UI (TUI) does bind `8` to this tab — see
> [Dashboard § TUI](../dashboard.md#terminal-ui-tui).

All Creative Ideas endpoints are behind the dashboard's session-cookie
authentication (`require_auth`), exactly like every other `/api/*` route — see
[Start the dashboard](../dashboard.md#start-the-dashboard) for the login flow.

## The live view

Each idea in the pool renders as a card with:

| Element | Source | Notes |
|---------|--------|-------|
| **Idea text** | `idea` | The candidate idea's content/title. |
| **Status pill** | `status` | One of the eight `IdeaStatus` values, colour-coded (see below). |
| **Created time** | `created_epoch` | Unix seconds, formatted client-side (`new Date(created_epoch * 1000)`). |
| **Rationale** | `rationale` | The "why" the generator recorded for the idea. |
| **Reviewer / synthesis signals** | `reviews`, `has_metric`, `metric` | How many reviews the idea accumulated and its success-metric name (the measurability reviewer's output). |
| **Links** | `links` | Count of typed links (e.g. to a routed goal/issue). |

Above the cards, a **status-count summary** shows how many ideas sit in each
status (`New`, `NeedsRevision`, `NeedsHumanReview`, `AcceptedForImplementation`,
`ImplementationStarted`, `ImplementationCompleted`, `Deferred`, `Rejected`).

### Status pill colours

| Status | Meaning |
|--------|---------|
| `New` | Freshly generated, not yet reviewed. |
| `NeedsRevision` | Synthesis asked for a rewrite before acceptance. |
| `NeedsHumanReview` | High-risk / flagged: a human must decide. |
| `AcceptedForImplementation` | Accepted; may be promoted to a goal. |
| `ImplementationStarted` | A goal/PR is in flight (set automatically when Promote routes to a goal). |
| `ImplementationCompleted` | Terminal; only when the success metric is met. |
| `Deferred` | Parked; may be reconsidered later. |
| `Rejected` | Terminal; pruned/rejected. |

### Empty state

When the store holds no ideas, the idea list (`#ci-list`) renders an explicit
**empty-state message** rather than a blank panel. This feature updates that
message to point the operator at **Run now** ("No creative ideas yet — use
**Run now** to generate a batch."); before Run now existed the copy read "No
ideas match. Simard fills this pool as the Creative Ideas thread runs." The view
always renders exactly what the store contains: `0..N` ideas, never a fabricated
count.

## Operator controls

### Run now

The **Run now** button (next to **Refresh** in the tab toolbar) triggers one
generation pass **immediately**, bypassing both the `enabled()` gate and the
24-hour interval. This is useful because:

- A **daemon restart resets the 24-hour timer**, so waiting for the next
  scheduled tick can mean waiting a full day.
- An operator may want a fresh batch of ideas right now, e.g. after seeding new
  goals or activity the generator should consider.

Clicking **Run now**:

1. Disables the button and shows a spinner (the run is synchronous from the
   operator's point of view).
2. Runs the generator's full pipeline once against the **live daemon store** —
   assemble inputs → generate → dedup/select → persist survivors → review →
   optionally route accepted ideas to goals/issues.
3. On success, re-loads the tab so the newly persisted ideas appear.
4. On any error, renders a **visible error banner** with the failure message.

**Guarded against double-runs.** A process-level re-entrancy guard ensures at
most one generation run is in flight at a time. A second **Run now** while one
is already running returns immediately with `{"running": true}` and the UI shows
"a creative-ideas generation run is already in progress" — it never starts a
concurrent run.

**Errors are loud, never silent.** If the idea source fails or is unavailable
(for example a `ReviewUnavailable` from an offline model), the failure is
surfaced verbatim in the banner. Run now never no-ops silently.

### Promote (accept, optionally route to a goal)

Every non-terminal idea card carries a **Promote** button *when the transition
is valid* (see [When each control is offered](#when-each-control-is-offered)).
Promote:

1. Transitions the idea to **`AcceptedForImplementation`** and persists it.
2. By default, **routes the accepted idea onto the goal board** — creating a
   `Proposed` goal tagged with the originating idea and advancing the idea to
   **`ImplementationStarted`**. The created goal (or any routing error) is
   surfaced in the UI.

The acceptance is persisted **before** routing, so an idea is durably accepted
even if goal routing fails; a routing failure never rolls back the acceptance —
it is shown as a non-fatal `goal_error` and the idea remains
`AcceptedForImplementation`.

> Accepted ideas that route to a goal are stamped with the `source:creative-ideas`
> provenance tag on the goal board — see
> [How to label, categorize, and filter goals](../howto/label-and-filter-goals.md).

### Prune (reject)

Every non-terminal idea card carries a **Prune** button. Prune transitions the
idea to **`Rejected`** (a terminal status) and persists it. A pruned idea stays
in the pool for the historical record with a `Rejected` pill; it is never
deleted.

### When each control is offered

The buttons respect the `IdeaStatus` state machine. The UI only offers a control
when the underlying transition is **valid from the idea's current status**, and
the **server re-validates** every write (defence in depth) — an invalid edge is
rejected with a clear error, never applied silently.

| Current status | Promote (→ `AcceptedForImplementation`) | Prune (→ `Rejected`) |
|----------------|:---:|:---:|
| `New` | ✅ | ✅ |
| `NeedsRevision` | — | ✅ |
| `NeedsHumanReview` | ✅ | ✅ |
| `Deferred` | — | ✅ |
| `AcceptedForImplementation` | — | ✅ |
| `ImplementationStarted` | — | ✅ |
| `Rejected` *(terminal)* | — | — |
| `ImplementationCompleted` *(terminal)* | — | — |

- **Promote** targets `AcceptedForImplementation`, which the state machine allows
  only from `New` and `NeedsHumanReview`.
- **Prune** targets `Rejected`, which is allowed from every non-terminal status.
- On the two terminal statuses (`Rejected`, `ImplementationCompleted`) neither
  control is shown.

For the full transition table see
[`IdeaStatus` state machine](../reference/creative-ideas-api.md#ideastatus-state-machine).

## HTTP API

All routes are under `/api/creative-ideas`, return `application/json`, and sit
behind `require_auth`. Read endpoints degrade to `{"error": …}` at HTTP 200;
write endpoints return an explicit success or an `{"error": …}` (also HTTP 200)
that the UI renders as a visible banner. Ideas are addressed by their stable
**`idea_id`**.

| Method & path | Purpose |
|---------------|---------|
| `GET /api/creative-ideas` | List the live idea pool + status counts. |
| `POST /api/creative-ideas/search` | Filter the pool by status and/or free text. |
| `POST /api/creative-ideas/run` | **Run now** — generate a batch on demand. |
| `POST /api/creative-ideas/{id}/promote` | Promote (accept, optionally route to goal). |
| `POST /api/creative-ideas/{id}/prune` | Prune (reject). |

### `GET /api/creative-ideas`

Returns the live pool (latest revision per idea, newest first) and per-status
counts.

```json
{
  "counts": {
    "New": 4,
    "NeedsRevision": 1,
    "NeedsHumanReview": 2,
    "AcceptedForImplementation": 1,
    "ImplementationStarted": 1,
    "ImplementationCompleted": 0,
    "Deferred": 0,
    "Rejected": 1
  },
  "ideas": [
    {
      "idea_id": "idea-7f3c…",
      "idea": "Add a recall-precision regression harness to the ranking evaluator",
      "status": "New",
      "rationale": "Ranking quality has no automated guardrail; regressions slip in silently.",
      "links": 0,
      "reviews": 3,
      "has_metric": true,
      "metric": "recall_precision_at_k",
      "created_epoch": 1751856238
    }
  ]
}
```

On a read failure the shape degrades gracefully:

```json
{ "error": "…", "ideas": [], "counts": {} }
```

### `POST /api/creative-ideas/search`

Body: `{ "status"?: string, "query"?: string }`. `status` filters to one
`IdeaStatus` (an unknown value is an error); `query` is a case-insensitive
substring match over the idea text and rationale. Returns
`{ "results": [ …idea summaries… ] }`, or `{ "error": …, "results": [] }`.

### `POST /api/creative-ideas/run`

Triggers one un-gated generation pass against the live daemon store. Body is
ignored (send `{}`).

**Success:**

```json
{
  "ok": true,
  "report": {
    "generated": 10,
    "surviving": 8,
    "persisted": 8,
    "reviewed": 8,
    "routed_goal": 1,
    "routed_issue": 1,
    "review_errors": 0
  }
}
```

**Already running (re-entrancy guard):**

```json
{ "error": "a creative-ideas generation run is already in progress", "running": true }
```

**Generation/persist failure (surfaced loudly):**

```json
{ "error": "review unavailable: idea source returned no candidates" }
```

Report fields:

| Field | Meaning |
|-------|---------|
| `generated` | Raw ideas the source produced this pass. |
| `surviving` | Ideas remaining after dedup + portfolio balancing. |
| `persisted` | Survivors written to the store as `status = New`. |
| `reviewed` | Ideas that completed the reviewer/synthesis pipeline. |
| `routed_goal` | Accepted ideas routed onto the goal board. |
| `routed_issue` | Ideas routed to a GitHub issue (`NeedsHumanReview`). |
| `review_errors` | Non-fatal reviewer errors (folded in, not silently dropped). |

### `POST /api/creative-ideas/{id}/promote`

`{id}` is the idea's `idea_id`. Body (optional): `{ "route_to_goal"?: bool }`
(default **`true`**). Accepts the idea, persists it, then best-effort routes it
to a goal.

**Success (routed to a goal):**

```json
{
  "ok": true,
  "idea": {
    "idea_id": "idea-7f3c…",
    "idea": "Add a recall-precision regression harness…",
    "status": "ImplementationStarted",
    "…": "…"
  },
  "goal": {
    "id": "goal-91a…",
    "title": "Add a recall-precision regression harness…",
    "status": "Proposed"
  }
}
```

**Success (accepted, but goal routing failed — non-fatal):**

```json
{
  "ok": true,
  "idea": { "idea_id": "idea-7f3c…", "status": "AcceptedForImplementation", "…": "…" },
  "goal_error": "goal store unavailable: …"
}
```

**Invalid transition (surfaced, never silent):**

```json
{ "error": "invalid idea transition from Deferred to AcceptedForImplementation" }
```

Notes:

- With `route_to_goal = false` the idea ends at `AcceptedForImplementation` and
  no goal is created — useful for accepting without immediately scheduling work.
- With routing successful, the idea advances to `ImplementationStarted` and the
  response carries the created `goal`.
- `{ "error": "idea not found" }` if no idea matches `id`.

### `POST /api/creative-ideas/{id}/prune`

`{id}` is the idea's `idea_id`. No body. Rejects the idea.

**Success:**

```json
{ "ok": true, "idea": { "idea_id": "idea-7f3c…", "status": "Rejected", "…": "…" } }
```

**Invalid transition (e.g. already terminal):**

```json
{ "error": "invalid idea transition from Rejected to Rejected" }
```

## Examples

The examples below assume a logged-in session cookie in `cookies.txt` (obtained
by POSTing your `~/.simard/.dashkey` code to `/api/login`, as the Playwright
smoke test does — see [Dashboard § Python Playwright smoke test](../dashboard.md#python-playwright-smoke-test)).

```bash
BASE=http://localhost:8080

# 1. List the live pool
curl -s -b cookies.txt "$BASE/api/creative-ideas" | jq '.counts, (.ideas | length)'

# 2. Run a generation pass on demand
curl -s -b cookies.txt -X POST "$BASE/api/creative-ideas/run" \
     -H 'Content-Type: application/json' -d '{}' | jq '.report'

# 3. Grab the first New idea's id
ID=$(curl -s -b cookies.txt "$BASE/api/creative-ideas" \
     | jq -r '.ideas[] | select(.status=="New") | .idea_id' | head -n1)

# 4. Promote it and route to a goal
curl -s -b cookies.txt -X POST "$BASE/api/creative-ideas/$ID/promote" \
     -H 'Content-Type: application/json' -d '{"route_to_goal": true}' \
     | jq '{status: .idea.status, goal: .goal.id, goal_error}'

# 5. Prune a different idea
curl -s -b cookies.txt -X POST "$BASE/api/creative-ideas/$OTHER_ID/prune" \
     | jq '.idea.status'   # -> "Rejected"
```

### Tutorial: from a fresh restart to a scheduled goal

1. Restart the daemon (`simard daemon restart`). The 24-hour Creative Ideas
   timer is now reset — no ideas will generate automatically for a day.
2. Open **Creative Ideas** and click **Run now**. Watch the status-count summary
   populate as the batch is generated, reviewed, and persisted.
3. Find a `New` idea you like and click **Promote**. Its pill flips to
   `ImplementationStarted` and a `Proposed` goal appears on the **Goals** tab,
   tagged `source:creative-ideas`.
4. Find a `New` idea that is not worth pursuing and click **Prune**. Its pill
   flips to `Rejected` and both controls disappear (terminal status).
5. Refresh — every change is already reflected, because reads and writes share
   the daemon's live store.

## DOM contract (for tests / automation)

The tab is stable enough to assert against without brittle class-name coupling.

**Selectors that already exist in the shipped markup:**

- **Panel root:** `#tab-creative-ideas` — a `<div class="tab-content">`. The
  e2e specs assert panel visibility by the `#tab-{slug}` id (e.g.
  `locator('#tab-creative-ideas')`), **not** a `data-tab` panel selector:
  `data-tab` lives on the *nav button* (`.tab[data-tab="creative-ideas"]`), not
  on the panel. The panel holds a single `<h1 class="page-h1">` and a
  jargon-free `<p class="page-lede">` (enforced by `tests_tab_meta` — see the
  [Tab identity contract](../dashboard.md#tab-identity-contract)).
- **Counts / list containers:** `#ci-counts` (`data-testid="ci-counts"`) and
  `#ci-list` (`data-testid="ci-list"`); idea cards render inside `#ci-list`.
- **Load/search errors** already render as a `<span class="err">` inside
  `#ci-list`.

**Selectors this feature adds** (per design §3.4, following the tab's existing
`onclick`-handler convention — the same style as the shipped **Refresh**
(`loadCreativeIdeas()`) and **Search** (`searchCreativeIdeas()`) buttons):

- The **Run now** button sits next to **Refresh** in the toolbar and invokes
  `runCreativeIdeas()`
  (`<button class="btn" onclick="runCreativeIdeas()">Run now</button>`).
- Per-idea **Promote** / **Prune** buttons are emitted inside each idea card in
  `#ci-list` by `renderIdeas`, carry `data-id="{idea_id}"`, and invoke
  `promoteIdea(id)` / `pruneIdea(id)`. Each button is rendered **only** when the
  corresponding transition is valid for that card's status.
- Action errors reuse the existing `<span class="err">` convention rendered
  inside `#ci-list`.

Client-side gating is UX only; the server re-validates every transition via
`CreativeIdea::try_transition`, so tests can safely POST an invalid edge and
assert a surfaced `{"error": …}`.

## Design guarantees

- **Live data only.** The view and all counts read the actual persisted ideas
  through a single shared reader (`CreativeIdeaStore::list`, backed in production
  by `ProspectiveCreativeIdeaStore`); nothing is hard-coded, and the empty pool
  renders an explicit empty state.
- **One shared store.** Run now / Promote / Prune write through the same
  `CognitiveMemoryOps` the daemon holds (the tier-0 in-process writer), so there
  is no split-brain between what the operator does and what the daemon sees.
- **Valid transitions only.** Promote and Prune go strictly through the
  `IdeaStatus` state machine; invalid edges surface a clear error and never
  corrupt status.
- **No silent failures.** Every failure — a stalled generation run, an offline
  idea source, an invalid transition, a goal-routing error — is surfaced to the
  operator, never swallowed.
- **Additive & back-compatible.** No new required fields, no schema migration,
  no changes to the model or generator; older clients keep working.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| Tab is empty right after a restart | The 24-hour timer reset and no batch has run yet | Click **Run now** to generate on demand. |
| **Run now** shows "already in progress" | A run is in flight (re-entrancy guard) | Wait for the current run to finish, then retry. |
| **Run now** shows an error banner | Idea source unavailable / generation failed | The message is the verbatim cause (e.g. model offline); resolve it and retry — it is intentionally not a silent no-op. |
| **Promote** returns an invalid-transition error | The idea is not in `New` / `NeedsHumanReview` | Only those statuses can be accepted; the button is hidden for others — a stale page may have shown it. Refresh. |
| **Prune** returns an invalid-transition error | The idea is already terminal (`Rejected` / `ImplementationCompleted`) | Terminal ideas cannot be pruned; refresh to drop the stale button. |
| Promote succeeded but no goal appeared | `goal_error` in the response (routing failed) | The idea is still durably `AcceptedForImplementation`; inspect `goal_error`, fix the goal store, and re-promote (or route later). |

## See also

- [Dashboard](../dashboard.md) — the full tab taxonomy and the tab identity contract.
- [Creative Ideas subsystem API reference](../reference/creative-ideas-api.md) — the Rust surface, `IdeaStatus` state machine, and the dashboard HTTP contract.
- [Configure and operate the Creative Ideas thread](../howto/configure-creative-ideas-thread.md) — turn the background thread on/off and tune its cadence.
- [Creative Ideas background thread — design](../design/creative-ideas-thread.md) — motivation, decision log, and roadmap.
- [How to label, categorize, and filter goals](../howto/label-and-filter-goals.md) — the `source:creative-ideas` provenance tag on promoted goals.
