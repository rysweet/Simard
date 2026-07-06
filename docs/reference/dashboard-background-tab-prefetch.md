---
title: Background tab prefetch and refresh (instant tab switches)
description: Reference for the dashboard's background prefetch/refresh scheduler — every tab's data loads on page open and stays on its own persistent, per-tab refresh interval regardless of which tab is visible, so switching tabs renders already-fetched, continuously-refreshed data instead of a slow on-activate fetch. Documents the TAB_LOADERS registry, the bounded-concurrency staggered scheduler, apiFetch GET de-duplication, per-tab "Updated <relative>" freshness indicators, and the global document.visibilityState back-off gate (#2649).
last_updated: 2026-07-06
owner: simard
doc_type: reference
related:
  - ../dashboard.md
  - ./dashboard-e2e-tests.md
  - ./dashboard-thinking-cycle-history.md
---

# Background tab prefetch and refresh

Reference documentation for the dashboard's **background prefetch and refresh**
behaviour. This page describes the finished state shipped for issue #2649.

Before this feature, the dashboard loaded and refreshed a tab's data **only
when that tab became active**. Switching tabs triggered a fresh on-activate
fetch, and because some of those fetches are slow, tab switches felt laggy.
Every switch also *wiped* the previous tab's refresh timers, so a tab you were
not looking at went stale the moment you left it.

Now the dashboard **prefetches every tab on page load** and keeps **each tab on
its own persistent background refresh** regardless of which tab is visible.
Switching to a tab renders immediately from the already-fetched, continuously
refreshed data. A manual **Refresh** still works, and a subtle per-tab
"Updated *&lt;relative&gt;*" indicator lets the operator see exactly how fresh
each panel is — there is no silent staleness.

## What changed at a glance

| Before (#2627 and earlier) | After (#2649) |
|----------------------------|---------------|
| Data fetched only when a tab is activated | Every tab prefetched on page load |
| Refresh timers armed on activate, **wiped on the next tab click** (`activateTab` → `runTabFetches` → `clearTabTimers()`) | Each tab keeps its **own persistent interval**; nothing is wiped on switch |
| Switching tabs blocks on a fresh fetch | Switching tabs renders from cache immediately (non-blocking) |
| A non-visible tab goes stale | Every tab stays as fresh as its cadence, visible or not |
| No visibility awareness — timers keep firing on a hidden browser tab | Global back-off: refresh suspends when `document.visibilityState === 'hidden'`, resumes on visible |
| No fetch de-duplication — manual + auto could double-hit an endpoint | `apiFetch` de-dupes concurrent GETs for the same endpoint |
| No per-panel freshness signal | Per-tab `Updated <relative>` / `refreshing…` indicator |

The change is **additive and client-side**. No backend endpoints changed; every
loader reuses the same routes it always hit. The only source files touched are
the inline dashboard JS (`src/operator_commands_dashboard/index_html/part_01.rs`
and `part_05.rs`) plus tests.

## Behaviour

### 1. Prefetch every tab on load

On page load, the scheduler enqueues a background load for **every canonical
tab that has a registered loader** (see [TAB_LOADERS registry](#the-tab_loaders-registry)).
The wave is drained with **bounded concurrency and a stagger** so the daemon
never sees all ~10 tabs' endpoints fire at once (see
[Efficiency and the daemon budget](#efficiency-and-the-daemon-budget)).

Loaders are derived from the runtime `CANONICAL_TABS` list, so the prefetch set
tracks whatever tab set ships — a slug with no registered loader is **silently
skipped** (it is a not-yet-wired tab, not an error). This survives tab-set
churn from parallel consolidation work without hardcoding.

### 2. Persistent per-tab refresh

After the initial wave, each fetcher is armed on **its own persistent
interval** at a per-tab cadence (see [Refresh schedule](#refresh-schedule)).
These intervals are **never cleared on tab switch** — every tab keeps refreshing
in the background whether or not it is visible. Timer ids live in a single
table so arming and suspending are idempotent and cannot leak.

### 3. Activate = render from cache

Clicking a tab (or following a deep link / `hashchange`) now **renders from the
already-fetched cache** — it swaps the visible panel, updates
`document.title`, and returns. It does **not** block on a fetch and it does
**not** wipe any timers.

Today `activateTab(slug)` calls `runTabFetches(slug)`, whose **first line is
`clearTabTimers()`** — so each switch tears down every other tab's interval and
re-arms only the active tab's. The feature **removes that on-activate
`runTabFetches` call**: `activateTab` no longer clears or re-arms any timers,
because the background scheduler owns them for the page's lifetime.
(`activateTab` never calls `clearTabTimers()` directly — the wipe has always
lived inside `runTabFetches`.) An optional immediate, non-blocking refresh of
the newly-active tab may still run, but it never gates the render — the panel is
visible instantly with the last-fetched data.
The one interactive exception is **Workers → Terminal**: because a live PTY
cannot be sensibly prefetched, `initAgentLogTerminal` is still initialised
lazily on the first activation of the Workers tab (from `activateTab`), never
by the background scheduler.

### 4. Manual Refresh is preserved

Every existing manual **Refresh** button still works exactly as before. A
manual refresh and a background refresh of the same endpoint **share one
in-flight request** (see [Fetch de-duplication](#fetch-de-duplication-apifetch)),
so pressing Refresh while a background tick is in flight does not double-hit the
daemon.

## The TAB_LOADERS registry

`TAB_LOADERS` is the single source of truth mapping each tab slug to the set of
background fetchers it owns. It replaces the previous `runTabFetches(slug)`
slug-branch chain. Each entry is a list of `{ fn, intervalMs, paths }`
descriptors:

| Field | Meaning |
|-------|---------|
| `fn` | The existing loader function to call (e.g. `fetchStatusSnapshot`). Reused as-is; no loader was rewritten. |
| `intervalMs` | This fetcher's persistent background refresh cadence, in milliseconds. |
| `paths` | The endpoint pathname(s) this loader reads, used to compute the tab's freshness indicator (the minimum `lastOk` across its paths). |

Registry rules:

- **Only list/summary loaders are registered.** Interactive, streaming, or
  attach-style surfaces are **never** auto-opened in the background — see
  [What is deliberately excluded](#what-is-deliberately-excluded).
- **Unknown slugs are skipped.** The scheduler iterates `CANONICAL_TABS`; a slug
  absent from `TAB_LOADERS` is a no-op, so adding or renaming a tab never throws.
- **Constants are grouped at the top** of the scheduler block for easy tuning.

### Refresh schedule

Per-tab cadences preserve every previous interval as a **floor** — a tab is
never refreshed *slower* than it was before, and several now refresh in the
background where they previously only refreshed while visible. Fast-changing
data refreshes more often than slow data:

| Tab | Background loader(s) | Interval | Rationale |
|-----|----------------------|----------|-----------|
| **overview** | `fetchStatusSnapshot` | 30 s | Daemon status board |
| **goals** | `fetchGoals`, `fetchWorkboard` | 30 s | Goal register + work board |
| **activity** | `fetchLogs` | 15 s | Live service log (fast) |
| | `fetchTraces`, `fetchThinking`, `fetchOodaCycles`, `fetchBrainFailures` | 30 s | Traces, thinking, cycles, failures |
| **workers** | `fetchSubagentSessions` | 5 s | Sub-agent sessions (fastest — very live) |
| | `fetchTmuxSessions` | 10 s | tmux sessions |
| | `fetchProcessTree` | 15 s | Process tree |
| **pull-requests** | `fetchMergeJudge`, `fetchPrReadiness` | 30 s | Merge decisions + readiness |
| **resources** | `fetchRecentMemories`, `fetchMemoryHistory`, `fetchMemoryGraph`, `fetchMemory`, `fetchCosts` | 120 s | Memory graph + cost ledger (slow-changing) |
| **chat** | `loadChatSessions` | 120 s | Session **list** only (never a WS attach) |
| **overseer** | `fetchOverseer` | 30 s | Steward health view |
| **journal** | `loadJournal` | 120 s | Narrative journal (slow-changing) |
| **creative-ideas** | `loadCreativeIdeas` | 120 s | Idea pool (slow-changing) |

The three cadence bands are: **fast** (5–15 s), **medium** (30 s), and **slow**
(120 s). If a future edit needs a different floor, change the `intervalMs` in
that tab's `TAB_LOADERS` entry — it is the only place the value lives.

> **Steady-state note.** Tabs that previously refreshed on a timer keep that
> cadence as a floor. Several loaders that previously ran **once on activate**
> now hold a persistent background interval too — `fetchGoals`, `fetchTraces`,
> the five **resources** loaders, `loadChatSessions`, `loadJournal`, and
> `loadCreativeIdeas`. This is the intended effect of #2649 (every tab loads
> *and* stays fresh), but it does raise the steady-state request count, which is
> exactly why the [daemon-budget controls](#efficiency-and-the-daemon-budget)
> (concurrency cap, stagger, jitter, GET de-dupe, visibility back-off) are
> mandatory rather than optional.

### What is deliberately excluded

Background prefetch applies to **list and summary loaders only**. The scheduler
**never** opens a live stream, WebSocket attach, or PTY on its own:

- **Workers → Terminal** (`initAgentLogTerminal`) — a live PTY attach. It is
  **not** in `TAB_LOADERS` and is never opened by the background scheduler.
  Instead it is initialised lazily the first time the operator opens the
  Workers tab (from `activateTab`), so a hidden or never-visited Workers tab
  holds no PTY connection.
- **Chat** live attach / message stream — only the session **list**
  (`loadChatSessions`) is prefetched. Opening a conversation and streaming
  replies remains an explicit user action.

These interactive surfaces are out of scope precisely because auto-opening them
in the background would consume a live connection with no operator watching it.

## Efficiency and the daemon budget

This is a **local operator dashboard**, so the scheduler is deliberately
lightweight and never stampedes the daemon. Four mechanisms bound the load:

### Bounded concurrency + stagger

The initial prefetch wave is drained as a queue in nav order with a small,
tunable set of constants at the top of the scheduler:

| Constant | Value | Purpose |
|----------|-------|---------|
| `MAX_CONCURRENCY` | `3` | At most three background loaders in flight at once during the initial wave. |
| `STAGGER_MS` | `150` | Minimum delay between dispatching queued loaders, so requests spread out instead of firing simultaneously. |
| interval jitter | small per-fetcher offset | Persistent intervals are jittered so they do not all realign onto the same tick and re-create a thundering herd. |

Treat these constants as a **rate-safety control**, not just a perf tweak —
they exist to protect the daemon from a self-inflicted request storm. Keep them
when editing the scheduler.

### Fetch de-duplication (`apiFetch`)

`apiFetch` de-dupes **concurrent GET requests for the same endpoint**. This is
what lets a manual Refresh, a tab switch, and a background tick coincide without
triple-hitting the daemon.

- **Key:** `method + pathname + search`, computed from a parsed URL. **GET only** —
  mutations and any `*/search` POSTs are never collapsed.
- **Shared promise:** a `Map` holds the pending promise; concurrent callers for
  the same key receive the *same* promise.
- **Cleared on settle:** the map entry is deleted when the request settles —
  **on success *and* on failure** — so a failed request can never poison the key
  and lock out future fetches.
- **No result caching beyond the DOM.** De-duplication shares an *in-flight*
  request only; there is no response cache. Rendered state lives in the DOM and
  in-memory maps — nothing is written to `localStorage`/`sessionStorage`.
- The `401 → /login` redirect guard is preserved: a background GET that returns
  `401` still redirects to the login page and stops, rather than retrying in a
  loop.

### Visibility back-off

The scheduler installs a **single global `visibilitychange` handler** (one gate,
not per-tab):

- On `document.visibilityState === 'hidden'` → **suspend all** background
  intervals. A backgrounded browser tab does no refresh work.
- On return to `visible` → **re-arm all** intervals **and immediately refresh the
  currently-active tab** so what the operator looks at first is fresh, then the
  rest resume on their normal cadence.

This is cheaper and simpler than per-tab back-off and guarantees a hidden
dashboard is not silently burning daemon requests.

## Freshness indicators

Because tabs now refresh in the background, the operator needs to know how old
each panel's data is. Each tab panel renders a subtle freshness element:

```html
<span data-testid="{slug}-updated">Updated 12s ago</span>
```

- The element is **injected by JS** into `#tab-{slug}` (via `textContent`,
  never `innerHTML`) — no HTML template markers change, so the
  [Tab Identity Contract](../dashboard.md#tab-identity-contract) and its
  `tests_tab_meta` cross-checks are untouched.
- The text reuses the existing **`timeAgo(ts)`** helper and reads
  `Updated <relative>` where `<relative>` is the **minimum `lastOk`** across all
  of that tab's registered `paths` (i.e. the age of its *stalest* underlying
  endpoint).
- While a fetch for that tab is in flight, the indicator shows a transient
  **`refreshing…`** state.
- `lastOk[pathname]` is stamped with the **client's** `Date.now()` on every `2xx`
  response — never a server-supplied timestamp — so the age reflects when the
  browser actually received data.

`data-testid="{slug}-updated"` is a stable hook for Playwright assertions.

## Rendering flow (end to end)

```
page load
  └─ startBackgroundScheduler()
       ├─ enqueue every CANONICAL_TABS slug that has a TAB_LOADERS entry
       ├─ drain queue: ≤ MAX_CONCURRENCY in flight, ≥ STAGGER_MS apart
       │     └─ each loader → apiFetch(GET) → de-dupe → render panel → stamp lastOk
       ├─ arm one persistent (jittered) interval per fetcher  ← never cleared on switch
       └─ install global visibilitychange gate

user clicks a tab
  └─ activateTab(slug)
       ├─ swap visible panel + update document.title      ← renders from cache, instant
       ├─ (optional) non-blocking refresh of this tab      ← never gates render
       └─ does NOT wipe timers, does NOT block on fetch

browser tab hidden  → suspend all intervals
browser tab visible → re-arm all intervals + refresh active tab
```

## Configuration

There is **no new user-facing configuration surface** — the feature is on by
default and requires no flags. Tuning is done in-source via the scheduler
constants and the per-tab registry:

| What to change | Where |
|----------------|-------|
| Max simultaneous background loaders | `MAX_CONCURRENCY` (scheduler block, `part_05.rs`) |
| Stagger between dispatches | `STAGGER_MS` (scheduler block, `part_05.rs`) |
| A tab's refresh cadence | that tab's `intervalMs` in `TAB_LOADERS` |
| Which loaders a tab prefetches | that tab's entry in `TAB_LOADERS` |
| Add prefetch for a new tab | add a `TAB_LOADERS` entry keyed by the new slug (an unregistered slug is simply skipped) |

Visibility back-off follows the browser's own `document.visibilityState`; there
is no setting to disable it (a hidden dashboard should not do work).

## Implementation pointers

The feature is a client-side change to the inline dashboard JS. These are the
concrete call sites it touches (line numbers current as of the #2649 baseline):

| Symbol | Current location | Change |
|--------|------------------|--------|
| `runTabFetches(slug)` | `part_01.rs:655` (a slug-branch chain) | Retired — replaced by the `TAB_LOADERS` registry driven by the background scheduler. |
| `clearTabTimers()` | defined `part_01.rs:636`; **called as the first line of `runTabFetches()`** at `part_01.rs:656` | This is the on-switch wipe being removed. It is *not* called by `activateTab` directly. |
| `activateTab(slug, section)` | `part_01.rs:672`; invokes `runTabFetches(slug)` at `part_01.rs:683` | **Drop the `runTabFetches(slug)` call** so activate only swaps the panel and updates the title (renders from cache). Because the wipe lives inside `runTabFetches`, removing this call removes the wipe — there is no `clearTabTimers()` line to strip from `activateTab` itself. |
| `initAgentLogTerminal()` | auto-invoked inside `runTabFetches('workers')` at `part_01.rs:660` | Move to **lazy init on the first Workers activation** (from `activateTab`); it is excluded from `TAB_LOADERS` so the background scheduler never opens the PTY. |
| `timeAgo(ts)` | `part_01.rs:331` | Reused as-is to render the per-tab `Updated <relative>` freshness text. |
| `TAB_LOADERS`, `startBackgroundScheduler`, `MAX_CONCURRENCY`, `STAGGER_MS`, `visibilitychange` gate | new scheduler block (`part_05.rs`) | New code — the single source of truth for prefetch + persistent refresh. |

> **Pointer correction.** An earlier design note said to "strip
> `clearTabTimers()` from `activateTab` (`part_01.rs:672`)". That is misplaced:
> `activateTab` never calls `clearTabTimers()` directly. The wipe is the first
> statement of `runTabFetches()` (`part_01.rs:656`), which `activateTab`
> invokes at `part_01.rs:683`. The correct edit is to **drop the
> `runTabFetches(slug)` call from `activateTab`**, not to search for a
> `clearTabTimers()` line inside `activateTab`.

## Security considerations

- **All background loaders route through `apiFetch`** (never raw `fetch`), so the
  `401 → /login` guard fires for background requests too; a backgrounded `401`
  stops the scheduler rather than retry-looping.
- **No cached or replayed credentials.** Every tick is a fresh,
  cookie-authenticated GET. No tokens are stored.
- **De-dupe is GET-only.** Keying on `method + pathname + search` from a parsed
  URL guarantees state-changing requests and `*/search` POSTs are never
  collapsed onto one another.
- **Freshness stamps use the client clock** (`Date.now()`), never a
  server-supplied timestamp — responses stay untrusted for age computation.
- **Indicators use `textContent`/escaping**, never `innerHTML`, so an injected
  `{slug}-updated` element cannot become a markup sink.
- **In-memory only.** The in-flight `Map`, the `lastOk` timestamps, and the
  timer table live in browser memory for the page's lifetime — nothing is
  persisted, and no secrets, tokens, PII, or response bodies appear in the
  de-dupe key, the timer table, or the console. Background failures log
  status + pathname only.
- **CSRF: N/A.** Auto-issued requests are all idempotent GETs; the scheduler
  never auto-issues a state-changing request.
- **Rate-safety is a security control.** The concurrency cap, stagger, jitter,
  and de-dupe together prevent the dashboard from DoS-ing its own daemon — do
  not remove them.

## Tests

Two test layers assert the finished behaviour.

### Rust registry/scheduler contract test

Added to `src/operator_commands_dashboard/index_html/tests_tab_meta.rs`
(alongside the existing Tab Identity Contract tests, which stay green):

- **Every canonical tab has a `TAB_LOADERS` entry** — the rendered JS registers a
  loader set for each slug in `CANONICAL_TABS` that is expected to prefetch (the
  interactive-only Terminal/Chat-attach surfaces are asserted *absent* from the
  background registry).
- **Scheduler markers are present** — the rendered HTML contains the
  `startBackgroundScheduler`, the `MAX_CONCURRENCY`/`STAGGER_MS` constants, and
  the `visibilitychange` handler, so a refactor that drops the scheduler fails
  CI.
- The pre-existing `tests_tab_meta` suite (`tab_meta_slugs_unique`,
  rendered-HTML cross-checks, etc.) continues to pass unchanged — freshness
  indicators are JS-injected and touch no `{{…}}` template markers.

Run with:

```bash
cargo test -p simard operator_commands_dashboard
```

### Playwright: `tab-prefetch.spec.ts`

`tests/e2e-dashboard/specs/tab-prefetch.spec.ts` uses the route-mock pattern
from `tabs.spec.ts` (mock every `/api/*` endpoint so tabs render without a live
backend) plus **per-route request counters** and Playwright's **`page.clock`**
to drive timers deterministically. It asserts:

1. **Background load for all tabs (not just active).** After load — *without
   clicking any tab* — every registered endpoint has been requested at least
   once. A tab that is not the initially-active one is proven to have prefetched.
2. **Switch renders pre-loaded data with no blocking fetch.** Clicking a tab
   makes its panel visible **immediately**, rendering the already-fetched data;
   the switch does not depend on a new in-flight request completing first.
3. **Hidden-tab back-off and resume.** Dispatching a `visibilitychange` to
   `hidden` and advancing `page.clock` past several intervals produces **no new
   requests**; returning to `visible` resumes refreshes and immediately
   refreshes the active tab.
4. **Concurrent duplicate fetches are de-duped.** Triggering a manual Refresh
   while a background tick for the same endpoint is in flight results in **one**
   network request, not two (asserted via the route counter).
5. **Freshness indicator updates.** The `[data-testid="{slug}-updated"]` element
   shows a relative `Updated …` string after a successful fetch and a transient
   `refreshing…` while in flight.

Run with:

```bash
cd tests/e2e-dashboard
npx playwright test specs/tab-prefetch.spec.ts
```

## Related

- [Dashboard](../dashboard.md) — overview, tabs, and the Tab Identity Contract
- [Dashboard E2E tests](./dashboard-e2e-tests.md)
- [Thinking tab — Cycle History](./dashboard-thinking-cycle-history.md)
