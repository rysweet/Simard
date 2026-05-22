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
| **Overview** | Daemon status (OODA loop active / stopped), current cycle number, top-priority goal, last cycle's actions, recent actions stream, system status (version, OODA daemon state, active processes, disk usage), open PRs, and open issues. |
| **Goals** | The full goal register: active top-N goals with priority, status, and current activity; the proposed backlog with promote/dismiss controls. |
| **Traces** | Live-tailed engineer subprocess traces and OODA cycle traces (xterm.js terminal). |
| **Logs** | Aggregated daemon and engineer logs. |
| **Processes** | Live process tree under the daemon — engineer subprocesses, LLM sessions, and their resource usage. |
| **Memory** | Cognitive memory graph (Working / Semantic / Episodic / Procedural / Prospective / Sensory) with per-type filters; full-text memory search; memory overview and per-type file listings. See [Memory architecture](memory.md). |
| **Costs** | Per-provider, per-model token spend across the active session. |
| **Chat** | Direct chat with Simard. |
| **Workboard** | Shared scratch canvas. (Renamed from "Whiteboard" — see [Tab identity contract](#tab-identity-contract).) |
| **Thinking** | Live thinking-cycle stream (planner output before action dispatch). |
| **Terminal** | Browser-attached PTY into the daemon host. |

## Screenshots

Overview — what the daemon did this cycle, top priority, recent actions, open PRs, system status, open issues:

![Dashboard overview](assets/dashboard-overview.png)

Goals — active priorities and backlog:

![Goals tab](assets/dashboard-goals.png)

Memory — six cognitive memory types with filters and search:

![Memory tab](assets/dashboard-memory.png)

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
cargo test -p simard_operator_commands_dashboard
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
