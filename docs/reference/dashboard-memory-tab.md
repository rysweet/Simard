---
title: Dashboard — dedicated Memory tab (cognitive-memory graph)
description: Reference for the dedicated dashboard **Memory** tab, which restores the full interactive cognitive-memory graph visualization (facts, events, procedures, plans, working memory, and sensory buffer as nodes, with item→type edges, per-type filters, live counts, pan/zoom, and node details) as its own top-level tab wired to LIVE memory data through GET /api/memory/graph. The graph reader is a single shared live reader (open_reader_client → &dyn CognitiveMemoryOps); the endpoint returns real {nodes, edges, available:true, stats} built by build_live_memory_graph, bounded by GRAPH_MAX_PER_TYPE and GRAPH_NODE_CONTENT_MAX. Episode and prospective enumeration is forwarded across every reader tier — including the IPC/daemon-socket backend — via additive ListAllEpisodes/ListAllProspective memory-IPC request variants, so per-item nodes never silently collapse to hub-only in the normal daemon topology.
last_updated: 2026-07-07
owner: simard
doc_type: reference
related:
  - ../dashboard.md
  - ../memory.md
  - ./dashboard-background-tab-prefetch.md
  - ./dashboard-overview-health-and-live-memory.md
  - ./cognitive-memory-client-helpers.md
---

# Dashboard — dedicated Memory tab

The **Memory** tab is a first-class, top-level dashboard tab that renders the
full interactive visualization of Simard's cognitive memory. It restores the
memory-graph component that the earlier tab consolidation had folded away into
an *"advanced memory view"* `<details>` toggle inside the **Resources** tab
([#2627](https://github.com/rysweet/Simard/issues/2627)). The visualization is
back as its own dedicated tab — one click from the nav bar, deep-linkable at
`#memory`, and wired to **live** memory data rather than a stale snapshot.

> **What changed.** The graph itself is unchanged in look and feel — it is the
> same force-directed canvas the dashboard has always shipped. Three things are
> new: (1) it lives on its **own tab** instead of behind a collapsed toggle
> under Resources; (2) its backend, `GET /api/memory/graph`, now returns
> **real nodes and edges** read live from the cognitive store instead of the
> stats-only response the de-fork left behind (empty `nodes`/`edges`,
> `available:false`, plus live counts and a `note` —
> [#2307](https://github.com/rysweet/Simard/issues/2307)); and (3) episode and
> prospective enumeration is now forwarded across **every** reader tier —
> including the IPC/daemon-socket backend that previously could not carry it, so
> those types no longer silently collapse to hub-only in production (see
> [Reader tiers and cross-backend enumeration](#reader-tiers-and-cross-backend-enumeration)).

## What the Memory tab shows

The tab renders an interactive, force-directed graph of Simard's six cognitive
memory types. Every memory type is always present as a labelled **hub** node,
and the enumerable types fan out into per-item nodes connected to their hub by
an edge:

| Memory type (`node.type`) | Operator label (filter) | Colour | Live per-item nodes | Source read |
|---------------------------|-------------------------|--------|---------------------|-------------|
| `WorkingMemory`    | Currently thinking about | `#f0883e` (orange) | hub only¹ | `get_statistics()` count |
| `SemanticFact`     | Facts learned            | `#58a6ff` (blue)   | yes | `search_facts("", …)` |
| `EpisodicMemory`   | Events remembered        | `#3fb950` (green)  | yes | `list_all_episodes(…)` |
| `ProceduralMemory` | Known procedures         | `#a371f7` (purple) | yes | `recall_procedure("", …)` |
| `ProspectiveMemory`| Planned actions          | `#d29922` (gold)   | yes | `list_all_prospective(…)` |
| `SensoryBuffer`    | Recent observations      | `#8b949e` (grey)   | hub only¹ | `get_statistics()` count |

¹ Working memory and the sensory buffer are transient, task-scoped, and not
exposed by an enumeration method on the read path, so they appear as a single
labelled hub whose live magnitude is reflected in the stats line (below) rather
than as individual item nodes. All four long-term types (facts, episodes,
procedures, prospective triggers) are enumerated live on **every** reader
backend — in-process, direct-library, and the IPC/daemon socket — so per-item
nodes never silently collapse to hub-only in the normal daemon topology (see
[Reader tiers and cross-backend enumeration](#reader-tiers-and-cross-backend-enumeration)).

The panel provides:

- **Six per-type filter checkboxes** (`.mem-filter`, one per memory type, all
  checked by default). Un-checking a type hides its hub and all its item nodes
  and edges live, without re-fetching.
- **A live stats line** (`#mem-graph-stats`) summarising the store:
  `Thinking:<working> Facts:<semantic> Events:<episodic> Procedures:<procedural> Planned:<prospective> Observed:<sensory>`,
  sourced from the endpoint's `stats` object.
- **Pan and zoom** (drag the background to pan, wheel to zoom) plus **node
  drag/pin** — click-drag a node to pin it in place.
- **Hover tooltips** and a **node-details panel** — hovering a node shows its
  type (colour-coded) and a content preview; the details side-panel shows the
  full node content.
- **A "Refresh Graph" button** that re-fetches `/api/memory/graph` on demand,
  in addition to the automatic background refresh.

## Opening the tab

1. Start the dashboard: `simard dashboard serve --port=8080`.
2. Click **Memory** in the nav bar, or deep-link straight to it:

   ```
   http://localhost:8080/#memory
   ```

The tab is prefetched in the background like every other tab, so switching to
it is instant and the graph is already populated — see
[Background tab prefetch and refresh](./dashboard-background-tab-prefetch.md).

## Live data source: `GET /api/memory/graph`

The tab is driven entirely by one endpoint. It reads the **live** cognitive
store through a single shared reader and returns the graph plus aggregate
statistics.

### Response shape

```jsonc
{
  "available": true,
  "nodes": [
    { "id": "hub:SemanticFact",  "type": "SemanticFact",  "label": "Facts learned",      "content": "42 facts" },
    { "id": "fact:academic-3",   "type": "SemanticFact",  "label": "rust ownership",     "content": "Rust moves ownership on assignment unless the type is Copy." },
    { "id": "hub:EpisodicMemory","type": "EpisodicMemory","label": "Events remembered",  "content": "17 events" },
    { "id": "episode:8f2c…",     "type": "EpisodicMemory","label": "merged PR #2610",     "content": "Merged PR #2610 after CI went green." }
    // …
  ],
  "edges": [
    { "source": "fact:academic-3", "target": "hub:SemanticFact" },
    { "source": "episode:8f2c…",   "target": "hub:EpisodicMemory" }
    // …
  ],
  "stats": {
    "working": 3, "semantic": 42, "episodic": 17,
    "procedural": 9, "prospective": 4, "sensory": 1
  }
}
```

### Field contract

| Field | Type | Meaning |
|-------|------|---------|
| `available` | `bool` | `true` whenever the reader was reachable — **including when the store is empty** (an empty store returns the six hubs, no item nodes, `available:true`) and **including partial degradation** (reader reachable but one type could not enumerate — that type falls back to hub-only and `available` stays `true`; see [Partial degradation](#partial-degradation-reachable-but-a-type-cant-enumerate)). |
| `nodes[]` | array | One object per node. Fields consumed by the renderer: `id` (unique), `type` (one of the six type literals above), `label` (short display text), `content` (detail/tooltip text, UTF-8-safe truncated). |
| `edges[]` | array | `{ source, target }` node-id pairs. Every item node has exactly one edge to its type hub; there are **no dangling edges** (every endpoint id exists in `nodes`). |
| `stats` | object | Live per-type counts mirroring `CognitiveMemoryOps::get_statistics()`: `working, semantic, episodic, procedural, prospective, sensory`. |
| `note` | `string` (optional) | Present only on the degraded path when the reader could not be opened. It is **path-free** (no filesystem paths) and human-readable; `available` is `false` and `nodes`/`edges` are empty. |

### Example

```bash
curl -s --cookie "session=<code>" http://localhost:8080/api/memory/graph | jq '{available, nodes: (.nodes|length), edges: (.edges|length), stats}'
```

```json
{
  "available": true,
  "nodes": 78,
  "edges": 72,
  "stats": { "working": 3, "semantic": 42, "episodic": 17, "procedural": 9, "prospective": 4, "sensory": 1 }
}
```

## Backend architecture

### Single shared live reader

The handler obtains a reader through the shared accessor
`open_reader_client(state_root) -> SimardResult<ReaderClient>` and reads the
store via `ReaderClient::ops() -> &dyn CognitiveMemoryOps`. This is the same
live read path the rest of the dashboard uses (e.g. `memory_recent`,
`memory_history`), so the graph never diverges from the counts shown elsewhere.
There is **one** reader per request; the builder does not open a second handle.

> **No "Bridge" identifiers.** The accessor is `open_reader_client` and it
> returns `ReaderClient`; the read path introduces no new `*Bridge` symbol.

### Reader tiers and cross-backend enumeration

`open_reader_client` resolves to whichever cognitive-memory backend owns the
store for the current topology, all behind the one `&dyn CognitiveMemoryOps`
trait object:

| Tier | Backend | When it is used |
|------|---------|-----------------|
| In-process | `SharedMemory` (registered writer) | The dashboard process also owns the store (tests, single-process runs). |
| IPC socket | `RemoteCognitiveMemory` | **The normal production topology** — the daemon owns memory and `simard dashboard serve` runs as a separate process that talks to it over the memory IPC socket. |
| Direct library | `LibraryCognitiveMemory` | The reader opens the on-disk library store directly. |

The graph must enumerate the same live items on **all three** tiers. Facts
(`search_facts("", …)`) and procedures (`recall_procedure("", …)`) already
round-trip over IPC. Episodes and prospective triggers did **not**:
`list_all_episodes` / `list_all_prospective` have empty default trait impls
(`cognitive_memory/mod.rs` — "the default returns empty so non-library backends
degrade gracefully"), and the IPC client `RemoteCognitiveMemory` did not
override them. So under the daemon-socket tier those two types fell through to
`Ok(vec![])` and rendered hub-only **even while `get_statistics()` reported
non-zero `Events` / `Planned`** — the graph diverging from the counts shown
beside it. That divergence is exactly the failure this tab must not have.

This feature closes the gap by extending the memory IPC protocol so the two
missing enumerators traverse the socket like the others:

| New IPC element | Location | Purpose |
|-----------------|----------|---------|
| `MemoryRequest::ListAllEpisodes { limit }` | `memory_ipc/mod.rs` | Request variant carrying the per-type cap. |
| `MemoryRequest::ListAllProspective { limit }` | `memory_ipc/mod.rs` | Request variant carrying the per-type cap. |
| Server dispatch arms → `MemoryResponse::Episodes` / `Prospectives` | `memory_ipc/server.rs` | Forward each request to `memory.list_all_episodes` / `list_all_prospective` on the daemon-side store. |
| `RemoteCognitiveMemory::list_all_episodes` / `list_all_prospective` | `memory_ipc/client.rs` | Client overrides that issue the new requests instead of inheriting the empty defaults. |

The response variants already exist (`MemoryResponse::Episodes`,
`MemoryResponse::Prospectives`), so the change is purely additive: two new
request arms plus two client overrides, mirroring the existing
`SearchEpisodesByKeywords` round-trip. After it, every reader tier returns
identical live episode and prospective nodes, and the graph never diverges from
the stats counts regardless of topology.

### Pure builder: `build_live_memory_graph`

The graph is assembled by a pure function that takes the trait object and
returns the JSON value, so it is unit-testable without HTTP:

```rust
fn build_live_memory_graph(ops: &dyn CognitiveMemoryOps) -> serde_json::Value
```

It:

1. Emits the **six type hubs** unconditionally (`id = "hub:<Type>"`), so the
   graph always shows the full memory taxonomy even on an empty store.
2. Enumerates each long-term type from the live store and emits a node per item
   with an edge to its hub:
   - `SemanticFact` ← `search_facts("", GRAPH_MAX_PER_TYPE, 0.0)`
   - `EpisodicMemory` ← `list_all_episodes(GRAPH_MAX_PER_TYPE)`
   - `ProceduralMemory` ← `recall_procedure("", GRAPH_MAX_PER_TYPE)`
   - `ProspectiveMemory` ← `list_all_prospective(GRAPH_MAX_PER_TYPE)`

   Every call goes through the same `&dyn CognitiveMemoryOps` trait object, so on
   the daemon-socket tier `list_all_episodes` / `list_all_prospective` reach the
   store via the new IPC request arms above (not the empty default impls) — the
   builder itself is backend-agnostic and unchanged across tiers.
3. Attaches `stats` from `ops.get_statistics()`.
4. Sets `available: true`.

Node ids are namespaced per type (`graph_node_id`) so ids never collide across
types and every edge endpoint resolves to a real node.

### Bounds and truncation

Two constants keep the payload bounded without dropping live fidelity:

| Constant | Value | Purpose |
|----------|-------|---------|
| `GRAPH_MAX_PER_TYPE` | `200` | Maximum item nodes emitted per enumerable type. |
| `GRAPH_NODE_CONTENT_MAX` | `2048` | Maximum `content` length in bytes; longer content is truncated by `truncate_graph_content`, which cuts on a UTF-8 char boundary (never mid-codepoint). |

### Degraded path

If `open_reader_client` fails (reader unreachable), the handler returns
`available:false`, empty `nodes`/`edges`, the `stats` it could obtain (zeros if
none), and a short, **path-free** `note`. This is a real fallback, not a
placeholder for the normal path: when the reader is reachable the endpoint
always returns live data, even if the store happens to be empty.

### Partial degradation (reachable but a type can't enumerate)

The degraded path above is for a reader that cannot be opened at all. A
distinct, softer case is a reader that **is** reachable but cannot enumerate one
specific type — for example a version-skewed daemon that predates the
`ListAllEpisodes` / `ListAllProspective` IPC arms, or a per-type read error. In
that case the builder does **not** fail the whole request: the affected type
falls back to **hub-only** (its magnitude still shown from `stats`), the other
types still enumerate, and `available` stays `true`. This keeps the graph
resilient to a mismatched daemon while never fabricating item nodes it could not
read. It is a defence-in-depth backstop — with the IPC arms in place (above) the
normal daemon topology enumerates all four long-term types fully.

### No stray diagnostics

The read path emits **no** `println!` / `eprintln!` — errors are surfaced
through the `note` field and normal `Result` handling, keeping production
output clean.

## Registration and wiring

The Memory tab is registered in exactly the places every dashboard tab is:

| Concern | Location | Entry |
|---------|----------|-------|
| Tab identity (label, title, h1, lede, tooltip) | `index_html/tab_meta.rs` → `TAB_METADATA` | `slug: "memory", label: "Memory"` |
| Client tab allowlist | rendered `const CANONICAL_TABS=[…]` (`index_html/part_01.rs`) | includes `'memory'` |
| Panel markup (`<div class="tab-content" id="tab-memory">`) | `index_html/part_00.rs` | H1 `Memory`, lede, filters, `#mem-graph-canvas`, tooltip, details panel |
| Renderer + fetch (`fetchMemoryGraph`, `mgColors`, force layout) | `index_html/part_03.rs` | canvas graph engine |
| Background prefetch / refresh | rendered `const TAB_LOADERS={…}` (`index_html/part_05.rs`) | `'memory': [{ fn: fetchMemoryGraph, intervalMs: 120000, paths: ['/api/memory/graph'] }, …]` |
| Endpoint | `operator_commands_dashboard/memory.rs` → `memory_graph()` | `GET /api/memory/graph` |
| IPC enumeration plumbing (daemon tier) | `memory_ipc/{mod,server,client}.rs` | `MemoryRequest::ListAllEpisodes` / `ListAllProspective` variants + server dispatch arms + `RemoteCognitiveMemory` overrides |

Because Memory is a real top-level tab, `#memory` is now a **canonical slug**,
not a deep-link alias. The retired-tab alias table no longer maps `#memory` to
`#resources`; `#costs` continues to resolve to the **Resources** tab.

### Styling and refresh consistency

The Memory tab uses the same page shell as every other tab: one
`<h1 class="page-h1">Memory</h1>`, a plain-English `<p class="page-lede">`, and
the shared card styling. Its loaders run on the standard 120 s background
refresh cadence used by the other data-heavy tabs, so the graph and stats stay
current while the tab is open and are pre-warmed before it is first shown.

## Tests

Three tests lock the feature in place — an in-process build test, an IPC-path
enumeration roundtrip, and a registration/nav check — plus a reconciliation of a
pre-existing tooltip guard:

1. **`mod tests_memory_graph` — in-process build** (in
   `operator_commands_dashboard/memory.rs`). Seeds a live cognitive store via a
   `HermeticState` temp root and an in-process writer (`register_in_process_writer`,
   which selects the `SharedMemory` tier), stores at least one fact, episode,
   procedure, and prospective trigger, then asserts that
   `build_live_memory_graph(ops)` (reading through `open_reader_client`):
   - reports `available: true`;
   - contains all **six** type hubs;
   - emits live per-item nodes whose `type` is one of the six allowlisted
     literals (matching `mgColors`);
   - has **no dangling edges** (every `source`/`target` id resolves to a node,
     and every item node links to its hub);
   - has `stats` equal to `ops.get_statistics()`; and
   - honours `GRAPH_MAX_PER_TYPE` (per-type node cap) and
     `GRAPH_NODE_CONTENT_MAX` (UTF-8-safe content truncation).

2. **IPC-path enumeration roundtrip** (in `memory_ipc/`, alongside the existing
   transport roundtrip tests). Drives enumeration through a **real socket** so the
   reader is `RemoteCognitiveMemory` (the daemon-socket tier), seeds an episode
   and a prospective trigger on the server-side store, and asserts that
   `list_all_episodes` / `list_all_prospective` return them over IPC — i.e. that
   the new `ListAllEpisodes` / `ListAllProspective` request arms forward and the
   client overrides them. **This test is required, not optional:** the in-process
   test in (1) exercises the one backend that already overrides the enumerators,
   so without an explicit socket-tier test the production IPC path could silently
   return empty episode/prospective nodes while the suite stays green.

3. **Registration / nav test** (in `index_html/tests_tab_meta.rs`). Asserts that
   `TAB_METADATA` contains an entry with `slug == "memory"` and
   `label == "Memory"`, that `tab_nav_html()` renders a **Memory** nav button,
   and that the rendered `CANONICAL_TABS` allowlist lists `memory` so its panel
   is reachable. The existing tab-identity cross-checks (unique labels/titles/
   H1s, no banned jargon, every SoT string present in the HTML) continue to pass
   with the new tab included.

> **Reconcile the tab-count tooltip guard.** The pre-existing test
> `index_html_all_eleven_tabs_have_tooltips`
> (`operator_commands_dashboard/tests_routes_a.rs`) hard-codes its `tabs` array
> and a literal `assert_eq!(tabs.len(), …)` that is already stale relative to its
> own name (it lists nine slugs and asserts nine, despite "eleven" in the
> identifier). It iterates the canonical tab list to prove each tab has a
> non-empty `title="…"` tooltip, so it **must** gain `"memory"` and have its
> length assertion updated to the final canonical count; otherwise Memory's
> tooltip is never verified here and the test remains internally inconsistent.
> Keep the array and the count in lock-step with whatever tab set this guard
> covers.

## Constraints honoured

- **Additive / back-compatible.** No memory endpoint is removed;
  `/api/memory`, `/api/memory/recent`, `/api/memory/history`, and
  `/api/memory/search` are unchanged. The Memory tab is added, not swapped in.
- **Additive IPC protocol.** The two new memory-IPC request variants
  (`ListAllEpisodes`, `ListAllProspective`) are appended to `MemoryRequest` and
  reuse the existing `MemoryResponse::Episodes` / `Prospectives` variants; no
  existing variant or its wire encoding changes, and a reader still degrades
  gracefully (hub-only for that type) against a daemon that predates them — see
  [Partial degradation](#partial-degradation-reachable-but-a-type-cant-enumerate).
- **Live data only.** The graph renders real nodes/edges read from the live
  cognitive store; there is no stale snapshot or placeholder on the normal path.
- **No new `println!` / `eprintln!`** in the production read path.
- **No new `*Bridge` identifiers** — the reader is `open_reader_client` →
  `ReaderClient`.
- **Never `--admin` / `--no-verify`.**

## Related

- [Dashboard](../dashboard.md) — full tab catalogue and the Tab Identity Contract.
- [Memory architecture](../memory.md) — the cognitive memory model behind the graph.
- [Background tab prefetch and refresh](./dashboard-background-tab-prefetch.md) — how the Memory tab is pre-warmed and refreshed.
- [Cognitive-memory client helpers](./cognitive-memory-client-helpers.md) — `open_reader_client` and the read path.
- [Overview Health & live memory-consolidation](./dashboard-overview-health-and-live-memory.md) — the sibling live-memory display fix.
