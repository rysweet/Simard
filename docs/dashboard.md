---
title: Dashboard
description: Read-only web dashboard for inspecting the autonomous OODA daemon, goal register, memory layers, processes, costs, and live traces.
last_updated: 2026-05-22
owner: simard
doc_type: howto
---

# Dashboard

Simard ships a read-only web dashboard that surfaces what the autonomous OODA daemon is doing right now: the active goal register, recent cycle actions, open PRs and issues, the cognitive memory graph, live traces, costs, and per-process resource usage. It is the primary operator-visible surface when Simard is running in daemon mode.

## Start the dashboard

```bash
simard dashboard serve --port=8080
```

A login code is generated on first start and printed to stdout. It is also persisted to `~/.simard/.dashkey` for re-use. Subsequent visits to `http://localhost:8080/` redirect to a login page that accepts the code and sets a session cookie.

## Tabs

The dashboard is a single-page app with the following tabs:

| Tab | Shows |
|-----|-------|
| **Overview** | Daemon status (OODA loop active / stopped), current cycle number, top-priority goal, last cycle's actions, recent actions stream, system status (version, OODA daemon state, active processes, disk usage), open PRs, open issues, and a **Machines & Memory Sharing** card (whether Simard runs on one machine or a group, and how they share what they've learned). |
| **Goals** | The full goal register: active top-N goals with priority, status, and current activity; the proposed backlog with promote/dismiss controls. |
| **Traces** | Recent agent traces collected from the cost ledger, journald, and in-process spans, plus OTEL status. Each cost row reads as plain language: **when** (relative time, absolute on hover), **what** (call type, model, estimated tokens, and dollar cost), and **who** (call context and session id) — so an operator can see which calls were most expensive without decoding raw `[cost]` lines. |
| **Logs** | The **Background Service Log** (live activity from Simard's always-on background process), the cost ledger, and per-cycle reports. The level menu (All / Errors / Warnings / Info) filters the log to a single severity, and a free-text box searches within it. |
| **Processes** | Live process tree under the daemon — engineer subprocesses, LLM sessions, and their resource usage. |
| **Memory** | Cognitive memory graph (Working / Semantic / Episodic / Procedural / Prospective / Sensory) with per-type filters; full-text memory search; a **Memory Overview** with the live **Memory Store** counts; and a **Memory Files** panel showing the goals snapshot plus any non-empty legacy snapshot files. See [Memory architecture](memory.md). |
| **Costs** | Per-provider, per-model token spend across the active session. |
| **Chat** | Direct chat with Simard. Conversations are saved as durable, resumable **sessions**: a sidebar lists every saved chat, the panel fills the page, and assistant replies stream in incrementally. See [Chat tab: durable, resumable sessions](#chat-tab-durable-resumable-sessions). |
| **Workboard** | Shared scratch canvas. (Renamed from "Whiteboard" — see [Tab identity contract](#tab-identity-contract).) |
| **Thinking** | Live thinking-cycle stream (planner output before action dispatch). |
| **Terminal** | Browser-attached PTY into the daemon host. |

### Logs tab: filtering by severity (#1687)

The **Background Service Log** panel classifies every line into a severity —
**error**, **warning**, or **info** — and the **level menu** filters the view to
just that severity. Picking **Errors** surfaces failures (parse failures, brain
fallbacks, "did not emit a recognised action") and hides routine info lines;
**All levels** shows everything. The text box composes with the level menu to
search within the currently selected severity.

Classification is served by `/api/logs`, which returns a `daemon_log_levels`
array parallel to `daemon_log_lines`, with an identical client-side fallback
classifier. This is required because the daemon emits human-readable lines with
no level token of their own — without classification, selecting any level
(even *Info*) matched nothing and the control appeared inert.

### Chat tab: durable, resumable sessions

The **Chat** tab is a full conversational surface over Simard's meeting
backend. Every conversation is a **durable, resumable session** — the complete
turn history is written to disk and survives both page reloads and daemon
restarts.

**Session sidebar.** The tab lists every saved chat session (newest first),
each showing its title — derived from the first message you sent — and when it
was last active. Click a session to reopen it: the panel loads the entire prior
conversation, and the connection resumes with full context on both the UI and
the agent side (the agent is re-seeded with the history, so replies stay
coherent). A **New chat** control starts a fresh session; the session record is
created the moment you send your first message, so empty windows never clutter
the list.

**Full-height layout.** The chat panel fills the available vertical and
horizontal space. The transcript scrolls inside a flex-grown message area while
the input row stays anchored at the bottom, and the panel grows with the browser
window — no fixed small box.

**Streaming with graceful fallback.** Assistant replies appear **incrementally**,
word-by-word, rather than all at once. A client that only understands the legacy
single-message shape still works — it simply receives the reply as one complete
message — so the conversation renders cleanly either way and is persisted
identically.

**Real agent, real memory.** Chat turns flow through the same
`SessionBuilder` / `MeetingBackend` path as the CLI meeting REPL (governed by
`SIMARD_LLM_PROVIDER`), so anything you say can become a goal and slash-commands
like `/status`, `/goal`, and `/close` work exactly as they do on the command
line. See [How to start a meeting](howto/start-a-meeting.md).

Sessions are stored under `<state_root>/chat_sessions/` (keyed by a stable
session id, honoring `SIMARD_STATE_ROOT`). Two REST endpoints back the sidebar —
`GET /api/chat/sessions` (list) and `GET /api/chat/sessions/{id}` (full
history) — and the live conversation runs over `GET /ws/chat` (optionally
`?session_id=<id>` to resume). Full storage layout, the REST contract, and the
WebSocket wire protocol (handshake, `restore`, `chunk`/`done`, and the
non-streaming fallback frame) are documented in the
[Dashboard Chat reference](reference/dashboard-chat.md).

### Overview tab: plain-English action details (#2358)

The Overview tab's **Last Cycle Actions** and **Recent actions** lists show the
daemon's raw `outcome.detail` strings, which are written for machines
(`brain: continue_skipping (brain-error fallback: no decision keyword found…)`).
Both lists now route those strings through the client-side
`humanizeActionDetail` helper, which strips the `brain:` / `advance-goal:` /
`<x>-brain:` prefixes, maps known decision tokens to plain phrases
(`continue_skipping` → *continued without acting*), drops brain-fallback
boilerplate, and applies the shared `BANNED_JARGON` strip — while preserving any
`agent='engineer-…'` reference verbatim so the inline **Attach →** button still
works. The transform is render-layer only: the canonical `brain` / `ooda_brain`
strings, logs, and API responses are unchanged. See
[Overview action-detail humanization](reference/dashboard-action-detail-humanization.md).

### Overview tab: plain-English "Machines & Memory Sharing" card

The Overview card that reports whether Simard is running on one machine or a
cluster (formerly headed **Cluster Topology**) used to render machine-internal
vocabulary directly: `Topology`, `Memory Sync: DHT+bloom gossip (peer-to-peer)`,
`Hive Status: standalone`, an `Event Bus` panel with `Subscribers` / `Events/min`
counters, and one row per **raw event-topic enum name** (`fact_imported`,
`fact_promoted`, `memory_sync_requested`, `node_joined`, `node_left`). A live
Playwright audit flagged this as the densest jargon cluster on the landing page.

The card is now titled **Machines & Memory Sharing** with a one-line
plain-English description, and every label/value is humanized at the render layer:

| Was | Now |
|-----|-----|
| `Cluster Topology` (card header) | **Machines & Memory Sharing** |
| `Topology: distributed` | **Multi-machine mode: Supported (can run across machines)** |
| `Local Host` | **This machine** |
| `Memory Sync: DHT+bloom gossip (peer-to-peer)` | **How memory is shared: Peer-to-peer (machines share facts directly)** |
| `Hive Status: standalone` | **Sharing status: Standalone (this machine only)** |
| `Peers` | **Other machines connected** |
| `Event Bus` | **Live internal signals** |
| `Subscribers` / `Events/min` / `Last event` | **Parts listening** / **Signals per minute** / **Most recent signal** |
| `fact_imported: 0 subs, …` (raw enum) | **Facts received from other machines: 0 listening, …** |

The transform is render-layer only via the client-side helpers
`humanizeTopology`, `humanizeSyncProtocol`, `humanizeHiveStatus`, and
`humanizeEventTopic`. The `/api/distributed` payload, the underlying protocol
strings, and the stable `data-testid` selectors are unchanged, and each raw
machine id survives as a `title=` hover tooltip so power users lose nothing.
Verified outside-in by `tests/gadugi/dashboard-cluster-clarity.sh`.

Before (machine jargon) and after (plain English):

| Before | After |
|--------|-------|
| ![Cluster card before](assets/dashboard-cluster-card-before.png) | ![Machines & Memory Sharing card after](assets/dashboard-cluster-card-after.png) |

### Workboard tab: plain-English Task Memory & Recent Actions

A live Playwright audit of the **Workboard** tab flagged two machine-jargon
offenders that leaked raw internal representations onto the page:

- **Task Memory** rendered raw goal-board **JSON blobs** directly — e.g.
  `{"active":[{"id":…,"status":{"InProgress":{"percent":8}}}]}` — exposing the
  serialized `GoalProgress` enum (`InProgress`) to the operator.
- **Recent Actions** showed the raw daemon result string — e.g.
  `brain: continue_skipping (recipe-engineer-lifecycle-brain: no decision keyword
  found in recipe output; defaulting to continue_skipping)`.

Both are now humanized at the render layer:

| Was | Now |
|-----|-----|
| `{"active":[{"id":"enhance-simard-meeting-experience","description":"Improve …","status":{"InProgress":{"percent":8}}}]}` | **Improve the interactive meeting facilitator … — In progress — 8%** |
| `status:{"InProgress":{"percent":5}}` (raw enum) | **In progress — 5%** |
| `status:{"Blocked":"waiting on CI"}` (raw enum) | **Blocked — waiting on CI** |
| `brain: continue_skipping (recipe-engineer-lifecycle-brain: no decision keyword found…)` | **continued without acting** |
| `no-action: I'll triage the adopt-tdd goal…` | **I'll triage the adopt-tdd goal…** |

The transform is render-layer only. Task Memory routes each fact's content
through the new client-side `humanizeTaskMemory` helper (which parses a
goal-board snapshot and renders each active goal as a plain line via
`humanizeGoalProgress`), and Recent Actions routes the daemon result through the
existing, tested `humanizeActionDetail` brain-decision humanizer (the same one
the Overview tab uses) before `renderActionDetail` performs its single escape —
so the inline **Attach →** button still works. The `/api/workboard` payload, the
`GoalProgress` serialization, and the stable `#wb-actions` / `#wb-facts-list`
render slots are unchanged, and the raw JSON / raw daemon string each survives
as an `escAttr()`-hardened `title=` hover tooltip so power users lose nothing.
Verified outside-in by `tests/gadugi/dashboard-workboard-clarity.sh`.

Task Memory — before (raw goal-board JSON) and after (plain English):

| Before | After |
|--------|-------|
| ![Task Memory before](assets/dashboard-workboard-task-memory-before.png) | ![Task Memory after](assets/dashboard-workboard-task-memory-after.png) |

Recent Actions — before (raw brain enum) and after (plain English):

| Before | After |
|--------|-------|
| ![Recent Actions before](assets/dashboard-workboard-recent-actions-before.png) | ![Recent Actions after](assets/dashboard-workboard-recent-actions-after.png) |

## Screenshots

Overview — what the daemon did this cycle, top priority, recent actions, open PRs, system status, open issues:

![Dashboard overview](assets/dashboard-overview.png)

Goals — active priorities and backlog:

![Goals tab](assets/dashboard-goals.png)

Memory — six cognitive memory types with filters and search:

![Memory tab](assets/dashboard-memory.png)

### Memory tab: live store vs. legacy snapshots

The **Memory Overview** card is the source of truth. Its **Memory Store**
section reports the live cognitive-memory counts straight from the native
graph store: recent observations, what Simard is currently thinking about,
events remembered, facts learned, known procedures, and planned actions.

The **Memory Files** panel sits beside it and is intentionally minimal:

- **Goals (snapshot)** — a point-in-time count of active and backlog goals,
  sourced from cognitive memory (not a disk file). It links to the **Goals**
  tab for the full board.
- **Legacy snapshots** — a single collapsed disclosure ("Legacy snapshots
  (superseded by the Memory Store)") that lists the retired JSON snapshot
  files (`memory_records.json`, `evidence_records.json`, `latest_handoff.json`)
  **only when a file actually has content**. When none of them qualify, the
  panel shows a one-line note instead.

This matters because those JSON files were superseded by the native Memory
Store. Rendering them as permanent "0 records / 0 B" tiles next to a store
holding thousands of facts told operators that memory was empty when it was
rich. The panel now hides empty legacy tiles so the displayed numbers always
match Simard's actual remembered state.

## Read-only

The dashboard does not let operators force shell commands or edit code through the browser. Goal promotion, status changes, and refresh are the only state-changing operations. All other panels are observational.

## Tab identity contract

Every tab in the dashboard satisfies four invariants. They exist so an operator who lands on any single page (deep link, browser-history entry, screenshot in a bug report) can immediately answer *"what page am I on?"* and *"what is this page for?"* without learning Simard's internal vocabulary.

The four invariants:

1. **Unique, non-empty browser `<title>`.** Each tab sets `document.title` to `"{PageName} · Simard"` — including Overview, which uses `"Overview · Simard"`. The format is mechanical and uniform; there are no per-tab exceptions. No two tabs share a title.
2. **Unique, non-empty visible `<h1>`.** Each tab panel renders exactly one `<h1 class="page-h1">` immediately under the global brand bar. No two tabs share an H1.
3. **Non-empty plain-English lede.** Each tab panel renders exactly one `<p class="page-lede">` immediately under its H1. The lede is a single sentence that explains what the page is for in language a non-expert can understand.
4. **No banned jargon in any lede.** The strings `OODA`, `Observe-Orient-Decide-Act`, `synergize`, `leverage`, and `ideate` are forbidden anywhere in lede text — the goal is to ban consultant-speak that an operator without Simard context cannot decode. The blocklist is enforced at build time by a unit test and again at runtime by the Playwright smoke test. Simard-internal domain vocabulary (`facilitator`, `consolidation`, `episodic`, …) is *allowed* — those are legitimate terms a memory or goals page may need to use; the bar is "no corporate jargon", not "no jargon at all".

The global header (`🌲 Simard Dashboard`) is intentionally demoted from `<h1>` to `<div class="brand">` so that every page has exactly one semantic `<h1>` — the page-specific one — when active.

### Where the strings live: `TabMeta` single source of truth

All five user-visible strings per tab (`label`, `title`, `h1`, `lede`, `tooltip`) plus the routing `slug` are defined in **one** Rust module:

```
src/operator_commands_dashboard/index_html/tab_meta.rs
```

```rust
pub struct TabMeta {
    pub slug: &'static str,     // e.g. "workboard"
    pub label: &'static str,    // nav button text, e.g. "Workboard"
    pub title: &'static str,    // browser <title>, always "{Label} · Simard"
    pub h1: &'static str,       // page <h1>, e.g. "Workboard"
    pub lede: &'static str,     // plain-English sentence shown under the H1
    pub tooltip: &'static str,  // rendered as the nav button's HTML `title=`
                                // attribute (browser-native hover tooltip)
}

pub const TAB_METADATA: &[TabMeta] = &[ /* one entry per tab, in nav order */ ];
```

The HTML template is rendered by substituting markers from `TAB_METADATA` in `index_html_string()`:

| Marker              | Resolves to                                                                 |
|---------------------|-----------------------------------------------------------------------------|
| `{{DEFAULT_TITLE}}` | Initial `<title>` of the page (matches the default-active tab).             |
| `{{TAB_NAV}}`       | Full `<div class="tabs">…</div>` nav, one button per tab (label + tooltip + `data-tab`). |
| `{{TAB_META_JS}}`   | `<script>window.__TAB_META = { … };</script>` map of `slug → {title, h1, label}`. |

The per-tab `<h1 class="page-h1">` and `<p class="page-lede">` blocks are inlined directly in each `<div class="tab-content">` in `part_00.rs` / `part_01.rs` rather than via a marker — so an editor can `grep` for a heading and find it in the markup. The cross-check tests in `tests_tab_meta.rs` (`rendered_html_contains_every_h1`, `rendered_html_contains_every_lede`, `rendered_html_contains_every_tooltip_from_sot`) enforce that every value in `TAB_METADATA` appears verbatim in the rendered HTML, so a typo or a forgotten panel header fails CI rather than shipping a tab with the wrong text.

The `__TAB_META` JS object is serialized with `serde_json::to_string` and then `<` is replaced with `\u003c` so that a future lede or title containing `</script>` cannot terminate the inline `<script>` tag. A `debug_assert!` at the end of `index_html_string()` rejects any unresolved `{{MARKER}}` left in the rendered output.

On the client, the existing tab-click handler in `part_01.rs` sets `document.title = window.__TAB_META[slug].title` when a tab is activated. The H1 and lede do not need to be re-injected at click time — every panel is pre-rendered with its own header block, and the handler toggles `class="active"` on `.tab-content` so that exactly one panel is on-screen at any moment.

### Adding a new tab

Adding a tab is a single-file edit followed by writing the panel content:

1. Append a new `TabMeta { … }` entry to `TAB_METADATA` in `tab_meta.rs`. Pick a `slug` matching `^[a-z][a-z0-9_]*$`, a one-word `label`, a `title` of the form `"{H1} · Simard"`, an `h1` (usually equal to `label`), a `lede` that passes the jargon blocklist, and a `tooltip`.
2. Add the panel to the appropriate `part_NN.rs`: a `<div class="tab-content" id="tab-{slug}">` whose first two children are `<h1 class="page-h1">{h1}</h1>` and `<p class="page-lede">{lede}</p>` with text matching the SoT entry exactly.
3. Run `cargo test` — the unit tests in `tests_tab_meta.rs` verify uniqueness of `slug`, `label`, `title`, `h1`, non-emptiness of `lede`, absence of banned jargon, and that the rendered HTML contains every label / H1 / lede / tooltip from the SoT. The smoke test will pick the new tab up automatically (it discovers tabs from the rendered DOM, not from a hardcoded list).

No other file needs to change. There is no second place to update a string.

### The Whiteboard → Workboard rename (#1993 / #1994 / #1995)

Historically the rightmost-but-one tab carried the visible label `"Whiteboard"`, while the underlying route, API endpoint (`/api/workboard`), and Playwright spec (`workboard.spec.ts`) all used `workboard`. The Tab Identity Contract requires one label per route, so the visible label was renamed to match the existing route: **`Whiteboard` → `Workboard`**. No URL, API, or storage path changed; only the user-facing string. Bookmarks to the `#workboard` deep link continue to work.

## Tests

Two complementary test layers enforce the Tab Identity Contract:

### Rust unit tests

`src/operator_commands_dashboard/index_html/tests_tab_meta.rs` covers:

- `tab_meta_slugs_unique`
- `tab_meta_labels_unique`
- `tab_meta_titles_unique`
- `tab_meta_h1s_unique`
- `tab_meta_titles_follow_label_dot_simard_format` (every `title` equals `"{label} · Simard"`)
- `tab_meta_ledes_non_empty`
- `tab_meta_ledes_no_banned_jargon` (rejects `OODA`, `Observe-Orient-Decide-Act`, `synergize`, `leverage`, `ideate`)
- `tab_meta_every_slug_has_header_marker` (template contains `{{HEADER:slug}}` for every entry in `TAB_METADATA`)
- `html_escape_handles_metachars` (`<`, `>`, `&`, `"`, `'`)
- `tab_meta_js_resists_script_breakout` (e.g. `</script><script>alert(1)</script>` payload cannot escape the inline `<script>` block)
- `all_markers_resolved_in_rendered_html`

Run with:

```bash
cargo test -p simard operator_commands_dashboard
```

### Python Playwright smoke test

`tests/e2e-dashboard/smoke_python/` is a small pytest suite that exercises the running dashboard end-to-end. It:

1. Reads `~/.simard/.dashkey` (or `SIMARD_DASHKEY`) and POSTs it to `/api/login` as a JSON body (`Content-Type: application/json`, field name `code`) to obtain a session cookie. The encoding matches the existing route handler in `operator_commands_dashboard/auth.rs`.
2. Discovers every nav button by querying `data-tab` attributes — no hardcoded tab list.
3. Clicks each button in turn and uses Playwright's `expect(locator).to_be_visible()` on `.tab-panel[data-tab="{slug}"]`. This avoids hard-coding a `.active` class name and lets the contract survive future tab-handler refactors.
4. Captures `document.title`, the visible `.page-h1` text, and the visible `.page-lede` text.
5. Asserts: all titles unique and non-empty; all H1s unique and non-empty; every lede non-empty and free of banned jargon.
6. Prints a markdown table `slug | title | h1 | lede` to stdout. CI uploads this as build evidence and the PR template links it into the description.

The `BANNED_JARGON` constant lives in both `tab_meta.rs` and `test_tab_clarity.py`. They are intentionally duplicated (no shared format file) and contributors are responsible for keeping them in step — both files are referenced from the same line in the "Adding a new tab" checklist, and the two-line list is short enough that drift is unlikely.

Run locally:

```bash
pip install -r tests/e2e-dashboard/smoke_python/requirements.txt
python -m playwright install --with-deps chromium

# In another terminal, start the dashboard:
simard dashboard serve --port=8080

pytest tests/e2e-dashboard/smoke_python/ -v
```

The smoke test pins `playwright==1.59.0` to match the CI image and the TypeScript Playwright suite.

### CI

The smoke test runs in the existing `e2e-dashboard` job in `.github/workflows/verify.yml`, after the TypeScript Playwright suite has already started the dashboard server and provisioned `~/.simard/.dashkey`. Three steps are appended:

```yaml
- run: pip install -r tests/e2e-dashboard/smoke_python/requirements.txt
- run: python -m playwright install --with-deps chromium
- run: pytest tests/e2e-dashboard/smoke_python/ -v --tb=short
  env:
    SIMARD_DASHBOARD_URL: http://localhost:${{ env.PORT }}
```

The `SIMARD_DASHBOARD_URL` environment variable is honored by `conftest.py` (defaulting to `http://localhost:8080`) so the same suite runs unchanged in CI and locally on a custom port. A failed assertion fails the job. The evidence table is visible in the job's log.

## Related

- [Daemon mode (autonomous OODA loop)](daemon-mode.md)
- [Memory architecture](memory.md)
- [Run the OODA daemon](howto/run-ooda-daemon.md)
- [Dashboard E2E tests](reference/dashboard-e2e-tests.md)
- [Overview action-detail humanization](reference/dashboard-action-detail-humanization.md)
