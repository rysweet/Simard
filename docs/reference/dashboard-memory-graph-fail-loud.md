---
title: Dashboard — Memory graph fail-loud data-load contract
description: Reference for the fail-LOUD behaviour of the dashboard Memory tab's cognitive-memory graph (GET /api/memory/graph). The graph never silently blanks: a data-load failure (cognitive reader unreachable, a per-type enumeration/statistics error, or a stats-vs-nodes discrepancy) is surfaced as a sanitized, path-free `error` field and painted as a visible on-canvas error overlay (#mem-graph-error / mgError) rather than an empty canvas. A genuinely empty store is a distinct, non-error state (six type hubs, no item nodes, available:true) with a neutral "empty" message. This closes the regression where the restored Memory tab (issue #2627, PR #2895) fetched the graph but rendered a blank canvas with no signal on any load error.
last_updated: 2026-07-07
owner: simard
doc_type: reference
related:
  - ./dashboard-memory-tab.md
  - ../dashboard.md
  - ../memory.md
  - ./cognitive-memory-client-helpers.md
  - ./dashboard-overview-health-and-live-memory.md
  - ./dashboard-e2e-tests.md
---

# Dashboard — Memory graph fail-loud data-load contract

The dashboard **Memory** tab renders Simard's live cognitive-memory graph
(facts, events, procedures, plans, plus working-memory and sensory-buffer hubs)
from `GET /api/memory/graph` — see
[dedicated Memory tab](./dashboard-memory-tab.md) for the graph itself.

This page documents one specific guarantee layered on top of that graph: **it
fails LOUD.** When the graph data cannot be loaded, the operator sees a visible
error — never a blank canvas that is indistinguishable from "memory is empty".

> **What this fixes.** The Memory tab was restored in
> [#2627](https://github.com/rysweet/Simard/issues/2627) /
> [PR #2895](https://github.com/rysweet/Simard/pull/2895) with a graph canvas and
> a background loader. But the loader treated every failure the same way it
> treated an empty store. The front-end had only a low-visibility fallback — a
> one-line `#mem-graph-stats` note (`Error: …` / `Load failed`) that `return`ed
> without touching the canvas — and, crucially, the backend **never emitted
> `error` at all**, so even that branch was effectively dead: on any data-load
> error the canvas stayed blank while a store holding thousands of facts appeared
> empty. The backend also swallowed read errors
> (`get_statistics().unwrap_or_default()`, `if let Ok(...)` per type) and the
> reader-unavailable branch returned a silent `available:false` payload with only
> a small `note`. This change removes those silent fallbacks end-to-end: the
> backend surfaces load failures as a structured `error`, and the front-end
> paints a visible overlay.

## Three outcomes, one signal

`GET /api/memory/graph` resolves to exactly one of three states. The client
renders each distinctly; **there is no fourth "silent blank" state.**

| Outcome | Backend response | What the operator sees |
|---------|------------------|------------------------|
| **Live data** | `error` absent; `nodes`/`edges` populated; `available:true`; live `stats`. | The force-directed graph, drawn from the live store. |
| **Genuinely empty store** | `error` absent; `nodes` = the six type hubs only, no item nodes; `edges` empty; `available:true`. The four enumerable-type `stats` (`semantic`/`episodic`/`procedural`/`prospective`) are `0`; the transient `working`/`sensory` counts may be non-zero — those two types are hub-only, so a transient-only store still renders as hubs with no item nodes. | The six hubs plus a neutral *"Memory graph is empty"* message. **Not** an error. |
| **Data-load failure** | `error` present (sanitized, path-free string). Reader-unavailable → `nodes:[]`, `available:false`; an in-build failure (per-type read `Err` or discrepancy) → the six hubs, `available:true`. See [Response shape by failure class](#response-shape-by-failure-class). | A visible on-canvas **error overlay** (`#mem-graph-error`) with the message; the (possibly partial) node set is suppressed; never a blank canvas. |

The single machine-readable signal is the top-level **`error`** field. Its
presence — and only its presence — puts the client into the error state.

## Fail-loud triggers (backend)

The `error` field is set whenever the graph cannot be assembled faithfully from
the live store. Three failure classes are surfaced, all in
`operator_commands_dashboard/memory.rs`:

1. **Reader unreachable.** `open_reader_client(state_root)` returns `Err` — the
   cognitive-memory reader (in-process, IPC/daemon socket, or direct library
   tier) could not be opened. The handler returns:

   ```jsonc
   {
     "error": "Cognitive memory reader is unavailable.",
     "nodes": [], "edges": [],
     "available": false,
     "stats": { "working": 0, "semantic": 0, "episodic": 0,
                "procedural": 0, "prospective": 0, "sensory": 0 }
   }
   ```

   This branch replaces the previous silent `available:false` + `note` payload.
   **Invariant:** on the reader-unavailable branch, `error` is present **iff**
   `nodes == []` **and** `available == false`.

2. **Per-type read error.** A statistics or enumeration read returns `Err`
   (`get_statistics()`, `search_facts`, `list_all_episodes`,
   `recall_procedure`, `list_all_prospective`). The builder no longer drops the
   error via `unwrap_or_default()` / `if let Ok(...)`; it sets `error`
   identifying which read failed. Any hubs already assembled may still be
   present, but the presence of `error` forces the client into the error state
   rather than presenting a partial graph as complete.

3. **Stats-vs-nodes discrepancy.** The reads *succeeded* but are internally
   inconsistent: an **enumerable** type reports `stats.<type> > 0` while it
   produced **zero** item nodes. That means the graph would misrepresent a
   populated store as hub-only, so the builder sets `error` describing the
   discrepancy instead of silently degrading. The guard is scoped **strictly to
   the four enumerable types** — `semantic` (facts), `episodic`, `procedural`,
   `prospective` — because working memory and the sensory buffer are transient
   and legitimately hub-only (they have no per-item enumerator), so counting
   them here would produce false positives.

Everything else (a truly empty store, working/sensory hubs with no items) is a
valid, non-error state.

### Response shape by failure class

The three triggers do **not** produce the same JSON. Only the reader-unavailable
branch is empty; the two in-build failures keep the already-assembled hubs so the
legend and six filters stay meaningful, and `available` stays `true` because the
reader *was* reachable:

| Trigger | `error` | `available` | `nodes` |
|---------|---------|-------------|---------|
| Reader unreachable | present | `false` | `[]` (empty) |
| Per-type read `Err` | present | `true` | six hubs (partial) |
| Stats-vs-nodes discrepancy | present | `true` | six hubs (partial) |

So the strict invariant *`error` present ⇔ `nodes == []` && `available == false`*
holds **only** on the reader-unavailable branch. For the two in-build failures
`error` coexists with the six hub nodes and `available: true`; the client keys off
`error` alone (see [front-end overlay](#front-end-error-overlay)), so it never
presents a partial graph as complete.

`error` is serialized as `Option<String>` with
`#[serde(skip_serializing_if = "Option::is_none")]` — it is **omitted** (not
`null`) on the live-data and genuinely-empty paths. The `"error": null` shown in
the `jq` examples below is `jq` filling a missing key in its projection, not a
literal field in the response.

### What is *not* an error

- **An empty store.** Zero facts/episodes/procedures/plans returns the six hubs,
  no item nodes, `available:true`, **no** `error`. The client shows the neutral
  empty message.
- **Working memory / sensory buffer being hub-only.** These two types are
  transient and expose no enumerator; their magnitude shows only via `stats`.
  They are exempt from the discrepancy guard above.

## Response shape

The successful/empty payload is unchanged from the
[Memory tab reference](./dashboard-memory-tab.md#response-shape) except that the
degraded path now carries `error` instead of `note`:

```jsonc
{
  "available": true,
  "nodes": [ /* six hubs + capped per-item nodes */ ],
  "edges": [ /* item -> hub edges, no dangling endpoints */ ],
  "stats": { "working": 3, "semantic": 42, "episodic": 17,
             "procedural": 9, "prospective": 4, "sensory": 1 },
  "timestamp": "2026-07-07T20:00:00Z"
}
```

### Field contract (fail-loud fields)

| Field | Type | Meaning |
|-------|------|---------|
| `error` | `string` (optional) | **Present only on a data-load failure.** Sanitized and **path-free** — never contains `state_root`, `$HOME`, or a backtrace. Its presence is the sole trigger for the client error overlay. Omitted (not `null`) on both the live-data and genuinely-empty paths. On an in-build failure it coexists with the six hub nodes — see [Response shape by failure class](#response-shape-by-failure-class). |
| `available` | `bool` | `true` when the reader was reachable (including an empty store). `false` only on the reader-unavailable branch, where `error` is present and `nodes`/`edges` are empty. |
| `nodes` / `edges` / `stats` / `timestamp` | — | Unchanged; see [dedicated Memory tab](./dashboard-memory-tab.md#field-contract). |

> **`note` is removed.** The pre-fix reader-unavailable payload carried a
> free-text `note`. That field is gone; failures are now reported through
> `error`. Clients that read `note` should read `error`.

### Examples

Reader reachable, populated store (happy path):

```bash
curl -s --cookie "simard_session=<token>" \
  http://localhost:8080/api/memory/graph \
  | jq '{error, available, nodes: (.nodes|length), edges: (.edges|length), stats}'
```

```json
{ "error": null, "available": true, "nodes": 78, "edges": 72,
  "stats": { "working": 3, "semantic": 42, "episodic": 17,
             "procedural": 9, "prospective": 4, "sensory": 1 } }
```

Reader unavailable (fail-loud):

```json
{ "error": "Cognitive memory reader is unavailable.",
  "available": false, "nodes": 0, "edges": 0,
  "stats": { "working": 0, "semantic": 0, "episodic": 0,
             "procedural": 0, "prospective": 0, "sensory": 0 } }
```

Empty store (valid, non-error):

```json
{ "error": null, "available": true, "nodes": 6, "edges": 0,
  "stats": { "working": 0, "semantic": 0, "episodic": 0,
             "procedural": 0, "prospective": 0, "sensory": 0 } }
```

## Front-end error overlay

The Memory tab panel (`index_html/part_00.rs`) gains a dedicated overlay element
positioned over the graph canvas:

```html
<div id="mem-graph-error" role="alert" style="display:none"></div>
```

The graph engine (`index_html/part_03.rs`) tracks a single error string,
`mgError` (`string | null`):

- **`fetchMemoryGraph()`** sets `mgError` when any of the following is true, and
  clears it (`null`) on a clean load:
  - the payload has a truthy `d.error`;
  - the `apiFetch('/api/memory/graph')` call throws (network / non-JSON / HTTP
    error);
  - a client-side discrepancy is detected (e.g. `d.stats` shows a populated
    enumerable type but `d.nodes` carries no item nodes for it) — a defence in
    depth mirroring the backend guard.

  On a clean load it also updates the `#mem-graph-stats` line and runs the
  normal `mgInitLayout()` → `mgApplyFilters()` → `mgSimulate()` render pipeline.

  This **replaces** the pre-fix handling in `fetchMemoryGraph`, which wrote
  `Error: …` / `Load failed` to the low-visibility `#mem-graph-stats` line and
  `return`ed, leaving the canvas untouched. That stats-line error branch is
  removed in favour of `mgError` + the `#mem-graph-error` overlay (a modify, not
  an add — the implementer must delete the old `d.error`/`catch` stats-line
  writes so there is a single error surface).

- **`mgRender()`** is the single paint path and it honours `mgError`:
  - when `mgError` is set, it shows `#mem-graph-error` with the message and does
    **not** draw the (possibly partial) node set beneath it — so a hub-only
    partial payload from an in-build failure is never shown as if it were the
    whole graph, and the canvas is never a silent blank;
  - when the graph is genuinely empty (no item nodes, no error) it shows the
    neutral *"Memory graph is empty"* message;
  - otherwise it draws the graph as before.

All overlay text is written with `textContent` / the shared `esc()` helper —
**never** unescaped `innerHTML` — so agent-authored memory content in an error
message cannot inject markup (stored-XSS safe).

The **"Refresh Graph"** button and the 120 s background loader
(`TAB_LOADERS['memory']` in `index_html/part_05.rs`, path
`/api/memory/graph`) are unchanged; a later refresh that succeeds clears the
overlay automatically.

## Configuration

No new configuration is introduced. The fail-loud path preserves the existing
graph bounds (a large store must not stream unbounded nodes into an error-free
render, and truncation must survive the switch to fail-loud):

| Constant (`memory.rs`) | Value | Purpose |
|------------------------|-------|---------|
| `GRAPH_MAX_PER_TYPE` | `200` | Max item nodes emitted per enumerable type. |
| `GRAPH_NODE_CONTENT_MAX` | `2048` | Max per-node `content` bytes; truncated on a UTF-8 char boundary. |

The Memory tab refreshes on the standard 120 s dashboard cadence and via the
manual **Refresh Graph** button.

## Security

| Ref | Guarantee |
|-----|-----------|
| SR-AUTH-1 | `GET /api/memory/graph` stays **above** the `require_auth` layer in `routes.rs` (route registered before `.layer(middleware::from_fn(require_auth))`), so it is auth-gated and returns `401` when unauthenticated. A scope-guard test asserts the route is inside the authenticated router. |
| SR-AUTH-3 | The dashkey (`~/.simard/.dashkey`) is never read, logged, or returned by this endpoint, and stays decoupled from `resolve_state_root()`. |
| SR-VAL-2 | The endpoint takes **no** request parameters; no request-controlled path is ever routed into `open_reader_client` (no path traversal). |
| SR-DATA-1 | The `error` string returned to the client is **path-free and sanitized** — no `state_root`, `$HOME`, or backtrace. Full failure detail is emitted only server-side via `tracing::warn!(target: "simard::dashboard", ...)`; there are **no** stray `println!` / `eprintln!`. |
| SR-DATA-2 | The error / empty overlay is rendered with `textContent` / `esc()`, never unescaped `innerHTML`. |
| SR-DATA-3 | `GRAPH_MAX_PER_TYPE` (200) and `GRAPH_NODE_CONTENT_MAX` (2048) caps are preserved on the fail-loud path (no DoS / payload-bloat regression). |

Net new attack surface: **none** — the endpoint stays read-only, parameter-free,
and auth-gated.

## Tests

The fail-loud contract is locked in at three layers:

1. **Backend unit tests** (`mod tests_memory_graph`,
   `operator_commands_dashboard/memory.rs`), using a fault-injecting
   `CognitiveMemoryOps` double plus the existing populated-store harness:
   - reader-open failure → response has `error`, empty `nodes`/`edges`,
     `available:false`;
   - a per-type enumeration / statistics `Err` → response has `error` (not a
     silently dropped result);
   - stats show a populated enumerable type but zero item nodes → response has
     `error` (discrepancy guard), scoped to the four enumerable types;
   - a genuinely empty store → **no** `error`, six hubs, `available:true`;
   - a populated store → non-empty structured `nodes`/`edges`, **no** `error`;
   - the returned `error` string is path-free (contains no `state_root` / home
     path).

2. **Rendered-HTML contract tests**
   (`index_html/tests_memory_tab.rs`): assert the emitted dashboard HTML/JS
   wires the error path — `fetchMemoryGraph` branches on `d.error` / a fetch
   exception, the `#mem-graph-error` overlay element exists, and `mgRender`
   references `mgError` so the overlay is actually painted.

3. **E2E behavioural tests** (`tests/e2e-dashboard/specs/memory-tab.spec.ts`):
   - **mocked error state:** route `/api/memory/graph` to `{ "error": "..." }`
     and assert the Memory tab shows the visible `#mem-graph-error` overlay (not
     a blank canvas);
   - **mocked empty state:** hubs-only, no `error` → neutral empty message, no
     error overlay;
   - **live authenticated path:** read `~/.simard/.dashkey` (or
     `SIMARD_DASHKEY`), `POST /api/login`, then `GET /api/memory/graph`; against
     a store with content assert the response has **non-empty** `nodes`/`edges`
     and no `error`, and the canvas renders non-blank. This live test gates on
     the presence of a dashkey / live store and skips cleanly in CI when neither
     is available (no flake). See
     [Dashboard E2E tests](./dashboard-e2e-tests.md) and the outside-in check
     `tests/gadugi/dashboard-memory-fidelity.sh` for the login → curl pattern.

## Live acceptance gate

The fix is accepted when, against a live daemon whose store holds content
(≈7,700 facts at time of writing):

```bash
KEY="$(cat ~/.simard/.dashkey)"
CJ="$(mktemp)"
curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
     -d "{\"code\":\"$KEY\"}" http://localhost:8080/api/login >/dev/null
curl -s -b "$CJ" http://localhost:8080/api/memory/graph \
  | jq '{error, nodes: (.nodes|length), edges: (.edges|length), stats}'
```

returns non-empty `nodes` and `edges` with `error: null`, **and** the served
Memory-tab HTML/JS renders that graph on the canvas — while an induced load
failure surfaces the visible `#mem-graph-error` overlay instead of a blank
canvas.

## Constraints honoured

- **Additive.** No endpoint, response field (beyond adding `error`), or route is
  removed; `note` is the only field dropped and it belonged to a
  never-user-facing degraded payload. `/api/memory/graph` keeps its shape on the
  success and empty paths.
- **Fail-loud.** No silent blank/empty fallback on a data-load error — every
  failure class surfaces a visible signal.
- **No `*Bridge` identifiers.** The reader is `open_reader_client` →
  `ReaderClient`; no new bridge-named symbol (any casing) is introduced.
- **No stray diagnostics.** Failure detail goes to `tracing::warn!` only; no new
  `println!` / `eprintln!` outside the `[simard]` / tracing convention.
- **Never `--admin` / `--no-verify`;** full CI green; PR against
  `rysweet/Simard` `main`.

## Related

- [Dashboard — dedicated Memory tab](./dashboard-memory-tab.md) — the graph, its
  live read path, and reader tiers this contract builds on.
- [Dashboard](../dashboard.md) — full tab catalogue.
- [Memory architecture](../memory.md) — the cognitive memory model behind the graph.
- [Cognitive-memory client helpers](./cognitive-memory-client-helpers.md) —
  `open_reader_client` and the live read path.
- [Overview Health & live memory-consolidation](./dashboard-overview-health-and-live-memory.md)
  — sibling live-read / fail-loud display work.
- [Dashboard E2E tests](./dashboard-e2e-tests.md) — the authenticated E2E harness.
