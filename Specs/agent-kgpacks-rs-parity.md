# agent-kgpacks-rs Parity Specification

## Purpose

The goal **"advance agent-kgpacks-rs to full parity"** repeatedly stalled with
"no shippable progress" (diagnosis: `GENUINELY-STUCK`). The root cause was not a
technical blocker — it was that **"full parity" had no measurable definition**.
Each OODA cycle could not tell what "done" meant or what the next concrete step
was, so it produced `NO ACTION`.

This spec fixes that WHY. It makes the done-criteria **measurable**: it
enumerates every parity criterion as an id + a checkable acceptance test + a
current status + code evidence, and it defines an ordered backlog so the next
cycle always has a concrete, non-stuck next step.

## What "agent-kgpacks-rs" is here — two components, one name

Two distinct Rust artifacts have both been called "kgpacks-rs", and **conflating
them is exactly what let earlier done-gates go green while the port was
observably weaker than the original** (operator directive, meeting 2026-07-20:
*"It needs to be at feature parity, ie enumerate all the features of the original
and then compare with the new."*).

**Component 1 — `rysweet/agent-kgpacks-rs` (the standalone full port).**
A Cargo workspace (`kgpacks-query`, `kgpacks-ingestion`, `kgpacks-embeddings`,
`kgpacks-mcp`, `kgpacks-backend`, `kgpacks-cli`, `kgpacks-db`, `kgpacks-packs`,
`kgpacks-eval`, `kgpacks-corpus`, `kgpacks-agent`) whose mission is a **complete**
reimplementation of the Python `agent-kgpacks`. **Issue #4321 ("advance
agent-kgpacks-rs to full parity") targets THIS repo.** Its done-gate is the
[full feature-parity matrix](#full-feature-parity-matrix-component-1-original-features--rust-port)
below: *every* enumerated original feature must be functionally equivalent — or
explicitly operator-ratified out-of-scope. Nothing may be silently dropped.

**Operator directive (2026-07-20): retrieval must be _the same_, not merely
"good enough."** Keyword / substring / FTS-only search is **not** an acceptable
substitute for the original's retrieval. The original answers queries with real
**GraphRAG**: embedding-based vector semantic search + multi-hop graph traversal
+ hybrid ranking. The port's retrieval (R1/R3/R4, and the Component-2 query path)
is at parity only when it uses the *same retrieval method* and produces
same-or-better results on a shared fixture. A green test over a keyword LIKE scan
does **not** close a retrieval row — it must be the same.

**Component 2 — Simard's in-tree read-only knowledge client.**
`src/native_knowledge.rs` + `src/knowledge_client.rs`: a deliberately **thin**
consumer that answers Simard's `knowledge.*` RPC (read-only queries against
already-installed packs) with no Python subprocess. It does **not** need the
original's web UI, HTTP server, or ingestion/pack-build pipeline — Simard's
reasoner only *reads* packs. **It does, however, need the same _retrieval_ as the
original**: reading a pack still means answering a question, and per the operator
directive a read-only client must retrieve with real embedding-based semantic
search + multi-hop graph traversal + hybrid ranking, not a keyword LIKE scan.
Its narrower gate is the `KGP-*` criteria in
[§ Component-2 gate](#component-2-gate-simard-in-tree-read-only-client).

| Layer (Component 2) | Rust | Python reference (agent-kgpacks) |
|-------|-------------------|-----------------------------------|
| Typed client | [`src/knowledge_client.rs`](../src/knowledge_client.rs) | `simard_knowledge_client.py` |
| In-process handlers | [`src/native_knowledge.rs`](../src/native_knowledge.rs) | `python/simard_knowledge_bridge.py` wrapping `KnowledgeGraphAgent` |
| Transport wiring | [`src/rpc_subprocess_launcher.rs`](../src/rpc_subprocess_launcher.rs) `launch_knowledge_client_native` | Python subprocess client |

**Reference contract.** The canonical upstream is `rysweet/agent-kgpacks`
(Python, v0.4.1). The full matrix (Component 1) is enumerated directly from that
codebase's runtime surface; the `knowledge.*` provenance guarantee (*every answer
traces back to a specific source article*) is the cross-cutting invariant for
Component 2 (see [`docs/ecosystem-map.md`](../docs/ecosystem-map.md)).

## Full feature-parity matrix (Component 1: original features → Rust port)

This is the **done-gate for issue #4321**. It enumerates the runtime feature
surface of the original Python `agent-kgpacks` (v0.4.1 — 136 features inventoried
across 11 dimensions from `mcp_server.py`, `wikigr/agent/*`, `bootstrap/*`,
`backend/*`, `frontend/*`) and records the status of each in the standalone Rust
port `rysweet/agent-kgpacks-rs` (workspace @ 2026-07-15).

**Status legend:** ✅ PARITY (same behavior, verified by a test; "or better" only
if it matches/exceeds on a shared fixture — never via a weaker keyword path for
retrieval) · 🟡 PARTIAL
(present but simplified, gated behind an optional feature, or limited) · ❌ MISSING
(not implemented) · ⚠️ OOS? (**proposed** out-of-scope — requires explicit
operator ratification; default is in-scope, nothing is silently dropped).

**The gate is closed only when every row is ✅ or ⚠️→ratified-OOS.** A 🟡 or ❌
row means the port is not yet at parity. Equivalence means the **same behavior**,
verified on a shared fixture encoded as the acceptance test. A rare "or better"
alternative is allowed only when it *demonstrably matches or exceeds* the original
on that fixture — "simpler but weaker" fails. **For the retrieval rows (R1, R3,
R4) there is no keyword-search shortcut:** substring / LIKE / FTS-only retrieval
never satisfies these rows, because the original retrieves via embedding-based
vector semantic search, multi-hop graph traversal, and hybrid ranking. Parity
requires the same retrieval method, not just a green test on a weaker one.

### D1 — Retrieval & query

| # | Original feature | Rust port status | Evidence / gap |
|---|------------------|------------------|----------------|
| R1 | Vector semantic search (embedding cosine over sections) | 🟡 PARTIAL | HNSW index is real (`kgpacks-db`), but default embeddings are **deterministic-hash**, not a real model — semantic recall not at parity until a real embedder is wired |
| R2 | Vector index query (`QUERY_VECTOR_INDEX`) | ✅ PARITY | LadybugDB HNSW `embedding_idx` |
| R3 | Multi-hop graph traversal (arbitrary-depth `LINKS_TO`) | 🟡 PARTIAL | `hybrid.rs` graph signal is **1-hop only**; `entity_graph.rs` traverses `ENTITY_RELATION` with depth but not article `LINKS_TO` |
| R4 | Hybrid ranking (vector + graph + keyword) | 🟡 PARTIAL | `kgpacks-query/hybrid.rs` blends cosine + 1-hop graph + title keyword; graph depth-limited vs original |
| R5 | Keyword / FTS retrieval | ✅ PARITY | LadybugDB FTS index |
| R6 | Multi-query expansion (LLM alt phrasings) | 🟡 PARTIAL | `kgpacks-agent` `multi_query`/`expand_query` — gated on `copilot` feature |
| R7 | Direct title lookup (exact/regex) | ❌ MISSING | no equivalent found in `kgpacks-query` |
| R8 | Entity lookup (facts + edges) | ✅ PARITY | `kgpacks-query/entity_graph.rs`, `kgpacks-backend/graph_entities.rs` |
| R9 | Relationship path (shortest path between entities) | ❌ MISSING | no BFS/shortest-path over entity graph |
| R10 | Confidence estimation | ✅ PARITY | `estimate_confidence` (native) / agent confidence |
| R11 | Graph reranker (degree + PageRank centrality) | ❌ MISSING | ENHANCEMENTS layer **deferred to M5** (`kgpacks-query/lib.rs`) |
| R12 | Cross-encoder reranker (opt-in) | ❌ MISSING | deferred to M5 |
| R13 | Section / content-quality filtering | 🟡 PARTIAL | quality thresholds not fully ported |
| R14 | Provenance (answer → source article/url) | ✅ PARITY | cited source ids in results |

### D2 — Ingestion, indexing & build

| # | Original feature | Rust port status | Evidence / gap |
|---|------------------|------------------|----------------|
| I1 | Source fetch | 🟡 PARTIAL | Rust fetches **CVE corpus** (`kgpacks-corpus`, SSRF-guarded); original's **Wikipedia + generic-web** sources not ported |
| I2 | Article/section parsing | ✅ PARITY | `kgpacks-ingestion/content.rs` |
| I3 | Semantic chunking | ✅ PARITY | `kgpacks-ingestion` chunker |
| I4 | Entity/relationship extraction (LLM) | 🟡 PARTIAL | pipeline + sanitization present; **live LLM backend gated on `copilot`** |
| I5 | Schema creation (nodes/edges/indexes) | ✅ PARITY | `kgpacks-ingestion/schema.rs` DDL |
| I6 | Article loader (batched) | ✅ PARITY | `entity_relations.rs` bulk load |
| I7 | Embedding generation | 🟡 PARTIAL | pipeline present, hash-based default (see R1) |
| I8 | Expansion orchestration (seed → batch) | 🟡 PARTIAL | build pipeline present; graph-expansion orchestration not at parity |
| I9 | Seed generation (LLM) | 🟡 PARTIAL | `identify_seed_articles` — gated on `copilot` |
| I10 | Incremental / resumable ingest | ✅ PARITY | checkpoint/resume (params-hash) |
| I11 | Pack create / update | ✅ PARITY | `kgpacks build` |

### D3 — MCP server

| # | Original feature | Rust port status | Evidence / gap |
|---|------------------|------------------|----------------|
| M1 | `list_packs` tool | 🟡 PARTIAL | `kgpacks-mcp` registers tools; **stdio transport deferred to M5** |
| M2 | `pack_info` tool | 🟡 PARTIAL | scaffold only |
| M3 | `query_knowledge_pack` tool | 🟡 PARTIAL | `handle_query` delegates to retriever, but no real stdio MCP server |

### D4 — CLI

| # | Original feature | Rust port status | Evidence / gap |
|---|------------------|------------------|----------------|
| C1 | Query / ask | ✅ PARITY | `kgpacks ask` |
| C2 | Build / create pack | ✅ PARITY | `kgpacks build` |
| C3 | Pack list / info | ✅ PARITY | `kgpacks pack …` |
| C4 | Pack install / remove / validate | 🟡 PARTIAL | write-path commands partially stubbed |
| C5 | status / monitor / explore / research-sources | ❌ MISSING | introspection/ops commands not ported |

### D5 — Embeddings

| # | Original feature | Rust port status | Evidence / gap |
|---|------------------|------------------|----------------|
| E1 | Real transformer model (BGE-base, 768-d) | 🟡 PARTIAL | **pluggable trait present but wired to a hash embedder by default** — the single largest quality gap (drives R1/R4) |
| E2 | Query-prefix / batching / device control | 🟡 PARTIAL | batching present; BGE query prefix + GPU device control N/A without a real model |
| E3 | int8 / PQ quantization | 🟡 PARTIAL | codec implemented but **disabled** in query path (pending recall baseline) |
| E4 | Embedding cache | ❌ MISSING | plan cache not ported |

### D6 — Storage / DB backend

| # | Original feature | Rust port status | Evidence / gap |
|---|------------------|------------------|----------------|
| S1 | Graph store (LadybugDB + Cypher) | ✅ PARITY | `kgpacks-db` |
| S2 | Vector index | ✅ PARITY | HNSW |
| S3 | FTS index | ✅ PARITY | LadybugDB FTS |
| S4 | Schema nodes/edges + chunk nodes | ✅ PARITY | `schema.rs` |
| S5 | Directory-based persistence | ✅ PARITY | pack `.db` dir |
| S6 | Pack format (`pack.db` + manifest + urls + few-shot) | 🟡 PARTIAL | db+manifest+urls present; `few_shot_examples.json` not consumed |

### D7 — HTTP / REST / frontend

| # | Original feature | Rust port status | Evidence / gap |
|---|------------------|------------------|----------------|
| H1 | FastAPI HTTP server (search, chat, graph, articles, stats, health, hybrid, autocomplete) | ❌ MISSING | `kgpacks-backend` is **request-contract logic only — no HTTP server** (deferred M5) |
| H2 | SSE streaming chat | ❌ MISSING | not ported |
| H3 | Security headers / CORS / rate-limit / cache middleware | ❌ MISSING | tied to absent HTTP server |
| H4 | React + TS + Vite frontend, PWA, e2e | ⚠️ OOS? | **proposed out-of-scope** — a web UI is arguably not part of a library/agent port; **needs operator ratification** |

### D8 — Eval / quality

| # | Original feature | Rust port status | Evidence / gap |
|---|------------------|------------------|----------------|
| V1 | Eval harness + metrics | 🟡 PARTIAL | `kgpacks-eval` orchestrator present; **real judge gated on `copilot`** |
| V2 | Gold / semantic / benchmark suites | 🟡 PARTIAL | fixtures partial; blocked on R1/E1 |
| V3 | Token/usage tracking | ✅ PARITY | `kgpacks-agent/usage.rs` |

### D9 — Advanced synthesis

| # | Original feature | Rust port status | Evidence / gap |
|---|------------------|------------------|----------------|
| Y1 | LLM answer synthesis (grounded) | 🟡 PARTIAL | `copilot_agent::synthesize_answer` — gated on `copilot` |
| Y2 | Graph-RAG synthesis | ❌ MISSING | deferred M5 |
| Y3 | Multi-doc synthesis | 🟡 PARTIAL | `--multidoc` flag present, not optimized |
| Y4 | Few-shot in-context learning | ❌ MISSING | deferred M5 |
| Y5 | Cypher-RAG generation | ❌ MISSING | deferred M5 |
| Y6 | Cypher safety validation (block dangerous ops) | ✅ PARITY | `kgpacks-query/cypher_safety.rs` (21 patterns) — **better** (parameterized) |
| Y7 | Context truncation (UTF-8 safe) | ✅ PARITY | boundary-safe truncation |
| Y8 | Markdown-fence stripping | ✅ PARITY | `kgpacks-agent/json.rs` |

### D10 — Packaging / distribution

| # | Original feature | Rust port status | Evidence / gap |
|---|------------------|------------------|----------------|
| P1 | Pack manifest / format / discovery / versioning | ✅ PARITY | `kgpacks-packs` |
| P2 | Registry client (remote search/download) | 🟡 PARTIAL | corpus fetch present; remote pack registry client partial |
| P3 | Pack signing | ✅ PARITY | Ed25519 (`kgpacks-packs`) — **better** (original unsigned) |
| P4 | Release tags → SemVer | ✅ PARITY | `versioning.rs` |
| P5 | Docker (backend + MCP images) | ❌ MISSING | no Dockerfile |
| P6 | Embeddable pack skills | ⚠️ OOS? | **proposed out-of-scope** — needs operator ratification |

### D11 — Config

| # | Original feature | Rust port status | Evidence / gap |
|---|------------------|------------------|----------------|
| G1 | Query/build/db config surface | 🟡 PARTIAL | env-var subset; much of the YAML surface (expansion, wikipedia, CORS, cache TTLs) tied to unported dimensions |

### Parity scorecard (as of 2026-07-20 inventory)

| Status | Count (of ~55 consolidated rows) |
|--------|----------------------------------|
| ✅ PARITY | 22 |
| 🟡 PARTIAL | 20 |
| ❌ MISSING | 11 |
| ⚠️ OOS? (needs ratification) | 2 |

**Headline: the port is NOT at feature parity today.** The decisive gaps are
(1) **real embeddings** (E1 → R1/R4 semantic quality), (2) **multi-hop graph +
GraphRAG synthesis** (R3/R9/Y2), (3) the **ENHANCEMENTS layer** deferred to M5
(rerankers, few-shot, Cypher-RAG — R11/R12/Y4/Y5), (4) the **real MCP stdio
server** (M1–M3), and (5) the **HTTP surface** (H1–H3). The earlier
read-only `KGP-*` gate could go all-green while every one of these remained
unbuilt — which is the exact failure this matrix closes.

## Component-2 gate: Simard in-tree read-only client

This is a **separate, deliberately narrower** gate for `native_knowledge.rs` /
`knowledge_client.rs` — Simard's read-only `knowledge.*` consumer. It does **not**
gate issue #4321 (the full-port matrix above does); it gates only Simard's own
query path. Read-only scope is legitimate here because Simard's reasoner never
builds packs or serves a UI.

Status legend: **DONE** (acceptance test present + green) · **OPEN** (not yet
implemented; acceptance test is the definition of done) · **OUT-OF-SCOPE**.

### Discovery & metadata

| ID | Criterion | Acceptance check | Status | Evidence |
|----|-----------|------------------|--------|----------|
| KGP-M1 | `knowledge.list_packs` returns installed packs with name/description/article/section counts | `native_knowledge_transport_list_packs` green | DONE | `native_knowledge.rs::discover_packs`, `register_knowledge_handlers` |
| KGP-M2 | `knowledge.pack_info` returns one pack's metadata; errors on unknown pack | `native_knowledge_transport_pack_info`, `native_knowledge_transport_pack_not_found` green | DONE | `native_knowledge.rs` `knowledge.pack_info` handler |
| KGP-M3 | `manifest.json` (`graph_stats`) parsed with directory-name fallback | `discover_packs_finds_packs_with_manifests` green | DONE | `native_knowledge.rs::PackManifest`, `discover_packs` |

### Query & retrieval

> **Retrieval-parity rule (operator directive 2026-07-20):** the DONE keyword-tier
> items (KGP-Q4, KGP-Q8) are *hardening of the interim keyword path* — they do
> **not** count as retrieval parity. Gate B's retrieval is at parity only when the
> three REQUIRED rows (KGP-Q5 graph, KGP-Q9 semantic vector, KGP-Q10 hybrid) are
> DONE against shared fixtures. Keyword search is not good enough; it must be the
> same GraphRAG retrieval the original performs.


| ID | Criterion | Acceptance check | Status | Evidence |
|----|-----------|------------------|--------|----------|
| KGP-Q1 | Query answers carry **source citations with URLs** (the traceability guarantee) | `query_pack_db_returns_source_urls_when_present`, `query_pack_db_treats_empty_url_as_no_citation`, `native_knowledge_transport_query_surfaces_source_url` green; urlless packs still work (`query_pack_db_omits_urls_when_column_absent`) | DONE | `native_knowledge.rs::query_articles` (url column projection), `table_has_column` |
| KGP-Q2 | Confidence score matches the Python `_estimate_confidence` heuristic | `estimate_confidence_matches_python_heuristics` green | DONE | `native_knowledge.rs::estimate_confidence` |
| KGP-Q3 | Empty / too-short questions degrade to a graceful low-confidence answer | `native_knowledge_transport_empty_question`, `query_pack_db_handles_empty_question_keywords` green | DONE | `native_knowledge.rs::query_pack_db` keyword filter |
| KGP-Q6 | Answer synthesis truncates snippets on a UTF-8 char boundary | `query_pack_db_finds_matching_articles` green + no panic on multibyte content | DONE | `native_knowledge.rs::build_answer` + `util::string_truncate` |
| KGP-Q7 | Each source surfaces its `section` | asserted within KGP-M/query tests | DONE | `native_knowledge.rs::SourceInfo.section` |
| KGP-Q8 | *(keyword-tier hardening — NOT retrieval parity)* Ranks candidates by keyword coverage (a title hit weighted above a content-only mention) so the `limit` cut keeps the most on-topic article instead of arbitrary rowid order | `query_articles_ranks_most_relevant_first`, `query_articles_limit_keeps_most_relevant`, `query_articles_prefers_title_over_content_match` green | DONE (sub-step) | `native_knowledge.rs::query_articles` (`ORDER BY <coverage score> DESC`), `TITLE_MATCH_WEIGHT` / `CONTENT_MATCH_WEIGHT` |
| KGP-Q4 | *(keyword-tier hardening — NOT retrieval parity)* Keyword search binds parameters instead of string-interpolating LIKE clauses | `like_contains_pattern_escapes_metacharacters`, `query_articles_treats_like_wildcards_as_literal`, `query_articles_binds_keywords_and_resists_injection` green | DONE (sub-step) | `native_knowledge.rs::query_articles` binds each keyword as `?n` via `like_contains_pattern` (`LIKE ?n ESCAPE '\'`) |
| KGP-Q5 | **[RETRIEVAL PARITY — REQUIRED]** GraphRAG retrieval: traverse entity + relationship tables (multi-hop), not only a single-table LIKE scan | NEW test: a pack fixture with `relationships` yields a graph-grounded answer joining linked entities, matching the original on a shared fixture | OPEN | `native_knowledge.rs::query_articles` comment: "simplified version of the Python `KnowledgeGraphAgent.query()`" |
| KGP-Q9 | **[RETRIEVAL PARITY — REQUIRED]** Vector **semantic** search: embedding-cosine retrieval over section/article embeddings, so a question retrieves semantically-related articles that share no literal keyword. Keyword/LIKE search does **not** satisfy this | NEW test: a shared fixture where the on-topic article uses different wording than the question is retrieved by semantic similarity (a keyword scan would miss it), matching the original's recall | OPEN | no embedding index in `native_knowledge.rs`; current path is keyword LIKE only |
| KGP-Q10 | **[RETRIEVAL PARITY — REQUIRED]** Hybrid ranking that blends vector-semantic + graph + keyword signals (the original's ranker), not keyword coverage alone | NEW test: on a shared fixture, ranking matches the original's ordering where semantic/graph signal outweighs literal keyword overlap | OPEN | `native_knowledge.rs` ranks by keyword coverage only (see KGP-Q8) |

### Transport, health & lifecycle

| ID | Criterion | Acceptance check | Status | Evidence |
|----|-----------|------------------|--------|----------|
| KGP-T1 | Native in-process transport — no Python subprocess | `launch_knowledge_client_native` wired; `knowledge_client.rs` tests green | DONE | `rpc_subprocess_launcher.rs`, `native_knowledge.rs::register_knowledge_handlers` |
| KGP-T2 | Health endpoint reports server liveness | `health_check_succeeds` green | DONE | `knowledge_client.rs::health`, `RpcHealth` |
| KGP-T3 | Connection reuse across queries | NEW test: two queries to one pack reuse an open `Connection` (or document why path-caching suffices) | OPEN | `native_knowledge.rs` `conn_cache` currently caches the db *path*, not a live connection |

### Cross-cutting guarantee

| ID | Criterion | Acceptance check | Status | Evidence |
|----|-----------|------------------|--------|----------|
| KGP-P1 | Every answer's sources trace back to a specific source article | Satisfied by KGP-Q1 + KGP-Q7 | DONE | see KGP-Q1, KGP-Q7 |

### Out of scope (Phase 9+)

| ID | Criterion | Status |
|----|-----------|--------|
| KGP-B1 | Install a knowledge pack | OUT-OF-SCOPE (deferred) |
| KGP-B2 | Build a pack from documentation | OUT-OF-SCOPE (deferred) |

## Definition of "full parity" (the two done-gates)

**Gate A — issue #4321 (the full port, `rysweet/agent-kgpacks-rs`).**
The port is at **full parity** when **every row of the
[Full feature-parity matrix](#full-feature-parity-matrix-component-1-original-features--rust-port)
is ✅ PARITY or a ⚠️ row that the operator has explicitly ratified OUT-OF-SCOPE**
(with the rationale recorded in this spec). Every ✅ must carry a named acceptance
test — for retrieval rows (R1/R3/R4), the test compares the port against the
original's output on a **shared query fixture** and the port must match or exceed
it. This gate is what closes #4321. Per the 2026-07-20 inventory it is **NOT met**
(20 🟡 + 11 ❌ + 2 unratified ⚠️).

**Gate B — Simard's in-tree read-only client (Component 2).**
`native_knowledge.rs` is at parity for Simard's needs when every in-scope `KGP-*`
criterion (`KGP-M*`, `KGP-Q*`, `KGP-T*`, `KGP-P*`) is **DONE** with its named
acceptance test green — **including the three REQUIRED retrieval-parity rows
(KGP-Q5 multi-hop graph, KGP-Q9 semantic vector, KGP-Q10 hybrid)**. Per the
operator directive, a keyword-only query path (even with KGP-Q4/Q8 green) does
**not** satisfy Gate B: the read-only client must retrieve with the same GraphRAG
method as the original. Gate B is **NOT met** today (retrieval is keyword LIKE
only; KGP-Q5/Q9/Q10 OPEN).

```
cargo test --lib native_knowledge
cargo test --lib knowledge_client
```

Gate B does **not** close #4321; it is Simard's own consumer contract. Keeping
the two gates separate is what prevents a green read-only client from masking an
incomplete full port.

## Ordered backlog (so the next cycle is never stuck)

**Full-port (#4321) — highest-leverage gaps first (each row → shared-fixture test):**

1. **E1 → R1** — wire a real embedding model (replace the default hash embedder);
   unblocks vector semantic search and hybrid ranking parity.
2. **R3 / R9** — multi-hop `LINKS_TO` traversal + entity shortest-path.
3. **M1–M3** — real MCP stdio server + tool schemas.
4. **R11 / R12 / Y2 / Y4 / Y5** — the ENHANCEMENTS layer (rerankers, Graph-RAG,
   few-shot, Cypher-RAG) currently deferred to M5.
5. **H1–H3** — HTTP surface (or ratify OOS if the agent/library scope excludes it).
6. **Operator ratification** of the ⚠️ rows (H4 frontend, P6 pack skills) — decide
   in-scope vs out-of-scope; default is in-scope until ratified.

**Component-2 (`native_knowledge.rs`) OPEN criteria:**

1. **KGP-Q9** — vector **semantic** retrieval (embedding-cosine), so questions
   retrieve semantically-related articles that share no literal keyword. This is
   the operator's "it needs to be the same" requirement: keyword search is
   insufficient. Needs an embedding index in the read path.
2. **KGP-Q10** — hybrid ranking blending semantic + graph + keyword, replacing the
   keyword-coverage-only ordering.
3. **KGP-Q5** — GraphRAG multi-hop retrieval over entities + relationships.
   Consider splitting into a fixture step and a traversal step.
4. **KGP-T3** — reuse an open `Connection` in `conn_cache` (or document the
   path-cache decision and add the reuse test).

The three retrieval-parity rows (KGP-Q5/Q9/Q10) are REQUIRED for Gate B —
keyword-tier hardening (KGP-Q4/Q8, DONE) does not substitute for them.

## Progress log

- **2026-07-08** — Spec created; **KGP-Q1 closed**: `native_knowledge.rs` now
  projects the pack's `url` column into `SourceInfo`, so query answers carry
  source citation URLs (degrading to `None` for urlless pack schemas). This is
  the first measurable parity advance and the reason this goal is no longer
  `GENUINELY-STUCK`.
- **2026-07-17** — **KGP-Q8 closed** (recall quality): `query_articles` now
  ranks candidate articles by keyword coverage (`ORDER BY` a score summing
  `TITLE_MATCH_WEIGHT` per title hit and `CONTENT_MATCH_WEIGHT` per content hit,
  DESC) before applying `limit`. Previously the query had no `ORDER BY`, so
  SQLite returned matches in arbitrary rowid order and an earlier-inserted
  single-keyword article could crowd the full-coverage article out of the
  `limit` results — starving the reasoner's planning-context enrichment
  (`enrich_planning_context` → `knowledge.query`) of the most relevant
  knowledge. The LIKE membership probes stay substring-based (recall breadth);
  ranking governs which candidates survive the cut. KGP-Q4 (parameterize the
  LIKE search) remains OPEN and orthogonal.
- **2026-07-20** — **KGP-Q4 closed** (correctness + injection-shape removal):
  `query_articles` no longer string-interpolates keywords into its `LIKE`
  clauses. Each distinct keyword is now bound as a parameter (`?n`) built by the
  new `like_contains_pattern` helper, which wraps the keyword as `%keyword%` and
  escapes the keyword's own `LIKE` metacharacters (`%`, `_`, and the escape
  char) so the search stays a literal-substring probe (`LIKE ?n ESCAPE '\'`).
  The same `?n` is reused by both the `WHERE` membership clause and the
  `ORDER BY` coverage score, so each keyword is bound exactly once. Previously a
  question word containing `%` or `_` silently widened the match (wildcards) and
  single quotes were hand-escaped by interpolation; now such tokens are matched
  literally and an injection-shaped keyword (e.g. `'; DROP TABLE articles; --`)
  is inert. Acceptance tests:
  `like_contains_pattern_escapes_metacharacters`,
  `query_articles_treats_like_wildcards_as_literal`, and
  `query_articles_binds_keywords_and_resists_injection`. Remaining OPEN parity
  criteria: KGP-T3 (connection reuse) and KGP-Q5 (GraphRAG multi-hop).
- **2026-07-20** — **Done-gate reframed to full feature-parity via enumeration**
  (operator directive: *"It needs to be at feature parity, ie enumerate all the
  features of the original and then compare with the new."*). Two actions: (1)
  **De-conflated** the two artifacts both called "kgpacks-rs" — issue #4321
  targets the **standalone `rysweet/agent-kgpacks-rs` full port**, while the
  `KGP-*` criteria gate only Simard's in-tree read-only `native_knowledge.rs`
  client (Component 2). Collapsing them is what let a green read-only client
  masquerade as a complete port. (2) Inventoried the **entire** original
  `agent-kgpacks` runtime surface (136 features across 11 dimensions) and the
  Rust port, producing the [Full feature-parity matrix](#full-feature-parity-matrix-component-1-original-features--rust-port).
  Result: **~22 ✅ / 20 🟡 / 11 ❌ / 2 ⚠️** — the port is materially not at parity.
  Decisive gaps: real embeddings (default is a hash embedder, so "vector search"
  is not semantically equivalent), multi-hop graph + Graph-RAG synthesis, the M5
  ENHANCEMENTS layer (rerankers/few-shot/Cypher-RAG), the real MCP stdio server,
  and the HTTP surface. The done-gate (Gate A) is now: every matrix row ✅ or
  operator-ratified OOS, retrieval rows proven on a shared fixture.
- **2026-07-20** — **Retrieval parity tightened to "same, not good enough"**
  (operator directive, meeting: *"No keyword search is not good enough. It needs
  to be the same."*). Removed the blanket "or better" hatch from the done-gate
  and made retrieval sameness mandatory: keyword / substring / LIKE / FTS-only
  search never satisfies R1/R3/R4 or Gate B. The port's retrieval must use the
  same GraphRAG method as the original — embedding-based vector semantic search
  (KGP-Q9 / R1), multi-hop graph traversal (KGP-Q5 / R3), and hybrid ranking
  (KGP-Q10 / R4) — verified on shared fixtures. Reframed Component 2: it still
  skips the UI/HTTP/pack-build surface, but it does **need** the original's
  retrieval, so the "does not need the embedding-model stack" carve-out was
  removed. KGP-Q4/Q8 relabeled DONE (sub-step) keyword-tier hardening — they no
  longer count as retrieval parity. Added KGP-Q9 (semantic vector) and KGP-Q10
  (hybrid) as REQUIRED/OPEN. Net effect: neither gate can go green on a keyword
  scan.
