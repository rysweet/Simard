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

## What "agent-kgpacks-rs" is here

"kgpacks-rs" is Simard's **native Rust reimplementation** of the knowledge-graph
pack client, which replaces the Python subprocess client:

| Layer | Rust (kgpacks-rs) | Python reference (agent-kgpacks) |
|-------|-------------------|-----------------------------------|
| Typed client | [`src/knowledge_client.rs`](../src/knowledge_client.rs) | `simard_knowledge_client.py` |
| In-process handlers | [`src/native_knowledge.rs`](../src/native_knowledge.rs) | `python/simard_knowledge_bridge.py` wrapping `KnowledgeGraphAgent` |
| Transport wiring | [`src/rpc_subprocess_launcher.rs`](../src/rpc_subprocess_launcher.rs) `launch_knowledge_client_native` | Python subprocess client |

**Reference contract.** "Parity" is measured against the observable
`knowledge.*` JSON-line RPC contract and the documented agent-kgpacks guarantee
that *every answer traces back to a specific source article* (see
[`docs/ecosystem-map.md`](../docs/ecosystem-map.md) and the provenance guarantee
mirrored in [`src/rust_expertise/pack.rs`](../src/rust_expertise/pack.rs)). The
canonical upstream is `rysweet/agent-kgpacks`. Criteria below are derived from
that contract and the in-tree port markers (e.g. the `native_knowledge.rs`
comment "simplified version of the Python `KnowledgeGraphAgent.query()`").

## Scope boundary

Per the Implementation Plan Phase 2, kgpacks-rs covers **read-only queries
against existing packs**. Pack *building* and *installation* are explicitly
Phase 9+ and are **out of scope** for "full parity" here (criteria `KGP-B*`).

## Measurable parity criteria

Status legend: **DONE** (acceptance test present + green) · **OPEN** (not yet
implemented; acceptance test is the definition of done) · **OUT-OF-SCOPE**.

### Discovery & metadata

| ID | Criterion | Acceptance check | Status | Evidence |
|----|-----------|------------------|--------|----------|
| KGP-M1 | `knowledge.list_packs` returns installed packs with name/description/article/section counts | `native_knowledge_transport_list_packs` green | DONE | `native_knowledge.rs::discover_packs`, `register_knowledge_handlers` |
| KGP-M2 | `knowledge.pack_info` returns one pack's metadata; errors on unknown pack | `native_knowledge_transport_pack_info`, `native_knowledge_transport_pack_not_found` green | DONE | `native_knowledge.rs` `knowledge.pack_info` handler |
| KGP-M3 | `manifest.json` (`graph_stats`) parsed with directory-name fallback | `discover_packs_finds_packs_with_manifests` green | DONE | `native_knowledge.rs::PackManifest`, `discover_packs` |

### Query & retrieval

| ID | Criterion | Acceptance check | Status | Evidence |
|----|-----------|------------------|--------|----------|
| KGP-Q1 | Query answers carry **source citations with URLs** (the traceability guarantee) | `query_pack_db_returns_source_urls_when_present`, `query_pack_db_treats_empty_url_as_no_citation`, `native_knowledge_transport_query_surfaces_source_url` green; urlless packs still work (`query_pack_db_omits_urls_when_column_absent`) | DONE | `native_knowledge.rs::query_articles` (url column projection), `table_has_column` |
| KGP-Q2 | Confidence score matches the Python `_estimate_confidence` heuristic | `estimate_confidence_matches_python_heuristics` green | DONE | `native_knowledge.rs::estimate_confidence` |
| KGP-Q3 | Empty / too-short questions degrade to a graceful low-confidence answer | `native_knowledge_transport_empty_question`, `query_pack_db_handles_empty_question_keywords` green | DONE | `native_knowledge.rs::query_pack_db` keyword filter |
| KGP-Q6 | Answer synthesis truncates snippets on a UTF-8 char boundary | `query_pack_db_finds_matching_articles` green + no panic on multibyte content | DONE | `native_knowledge.rs::build_answer` + `util::string_truncate` |
| KGP-Q7 | Each source surfaces its `section` | asserted within KGP-M/query tests | DONE | `native_knowledge.rs::SourceInfo.section` |
| KGP-Q8 | Retrieval **ranks candidates by keyword coverage** (a title hit weighted above a content-only mention) so the `limit` cut keeps the most on-topic article instead of returning matches in arbitrary storage (rowid) order | `query_articles_ranks_most_relevant_first`, `query_articles_limit_keeps_most_relevant`, `query_articles_prefers_title_over_content_match` green | DONE | `native_knowledge.rs::query_articles` (`ORDER BY <coverage score> DESC`), `TITLE_MATCH_WEIGHT` / `CONTENT_MATCH_WEIGHT` |
| KGP-Q4 | Keyword search binds parameters instead of string-interpolating LIKE clauses | `like_contains_pattern_escapes_metacharacters`, `query_articles_treats_like_wildcards_as_literal`, `query_articles_binds_keywords_and_resists_injection` green | DONE | `native_knowledge.rs::query_articles` binds each keyword as `?n` via `like_contains_pattern` (`LIKE ?n ESCAPE '\'`) |
| KGP-Q5 | GraphRAG retrieval: traverse entity + relationship tables (multi-hop), not only a single-table LIKE scan | NEW test: a pack fixture with `relationships` yields a graph-grounded answer joining linked entities | DONE | `native_knowledge.rs::query_graph` runs before the article fallback: it seeds keyword-matched `entities`, traverses `relationships` up to `MAX_GRAPH_HOPS` (2), and builds an answer naming the linked entities + relations. Tests: `query_graph_traverses_relationships_for_linked_entities`, `query_graph_reaches_two_hop_neighbor` |

### Transport, health & lifecycle

| ID | Criterion | Acceptance check | Status | Evidence |
|----|-----------|------------------|--------|----------|
| KGP-T1 | Native in-process transport — no Python subprocess | `launch_knowledge_client_native` wired; `knowledge_client.rs` tests green | DONE | `rpc_subprocess_launcher.rs`, `native_knowledge.rs::register_knowledge_handlers` |
| KGP-T2 | Health endpoint reports server liveness | `health_check_succeeds` green | DONE | `knowledge_client.rs::health`, `RpcHealth` |
| KGP-T3 | Connection reuse across queries | `conn_cache_reuses_open_connection_across_queries`, `native_knowledge_transport_repeated_query_reuses_connection` green | DONE | `native_knowledge.rs::ConnCache` caches a live read-only `Connection` per pack; the `knowledge.query` handler reuses it via `get_or_open` + `query_open_pack` |

### Cross-cutting guarantee

| ID | Criterion | Acceptance check | Status | Evidence |
|----|-----------|------------------|--------|----------|
| KGP-P1 | Every answer's sources trace back to a specific source article | Satisfied by KGP-Q1 + KGP-Q7 | DONE | see KGP-Q1, KGP-Q7 |

### Out of scope (Phase 9+)

| ID | Criterion | Status |
|----|-----------|--------|
| KGP-B1 | Install a knowledge pack | OUT-OF-SCOPE (deferred) |
| KGP-B2 | Build a pack from documentation | OUT-OF-SCOPE (deferred) |

## Definition of "full parity" (the done-gate)

kgpacks-rs is at **full parity** when **every in-scope criterion**
(`KGP-M*`, `KGP-Q*`, `KGP-T*`, `KGP-P*`) is **DONE** — i.e. each row's named
acceptance test exists and the following are green:

```
cargo test --lib native_knowledge
cargo test --lib knowledge_client
```

Out-of-scope `KGP-B*` criteria do **not** gate parity; they are tracked
separately for the Phase 9+ pack-authoring work.

## Ordered backlog (so the next cycle is never stuck)

**All in-scope parity criteria are now DONE — kgpacks-rs is at full parity.**
The done-gate below is green with every `KGP-M*`/`KGP-Q*`/`KGP-T*`/`KGP-P*` row
marked DONE. No OPEN in-scope criteria remain.

(KGP-Q5 — GraphRAG multi-hop retrieval — closed on 2026-07-21; see the progress
log below. KGP-T3 — reuse an open `Connection` in `conn_cache` — and KGP-Q4 —
parameterize the keyword LIKE search — are likewise **DONE**.)

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
- **2026-07-21** — **KGP-T3 closed** (connection reuse): the `knowledge.query`
  handler previously cached only each pack's database *path* and re-opened a
  fresh read-only `Connection` (re-parsing the schema, paying the file-open
  cost) on every query. It now caches the **live connection** itself. A new
  `ConnCache` newtype holds one `Arc<Mutex<Connection>>` per pack
  (`Arc<Mutex<HashMap<String, Arc<Mutex<Connection>>>>>` internally); its
  `get_or_open(pack_name, resolve_path)` returns the cached handle on a hit and
  opens + caches on a miss, invoking `resolve_path` (which discovers the pack and
  validates its database) **only on the miss** so the warm path avoids
  re-scanning packs on disk. Because rusqlite's `Connection` is `Send` but not
  `Sync`, each connection carries its own `Mutex`, so queries against one pack
  serialize on that per-pack mutex while different packs proceed independently.
  The retrieval logic was extracted from `query_pack_db` into
  `query_open_pack(conn, question, limit)` so the reused connection and the
  path-based unit tests share one query path (`query_pack_db` is now a
  `#[cfg(test)]` helper). Acceptance tests:
  `conn_cache_reuses_open_connection_across_queries` (proves reuse via
  `Arc::ptr_eq` and that the resolver does not re-run on a hit),
  `conn_cache_keeps_distinct_connections_per_pack`,
  `conn_cache_propagates_resolve_error_without_caching`, and
  `native_knowledge_transport_repeated_query_reuses_connection` (two RPC queries
  to one pack both succeed against the reused connection). The prior
  "pack not found" / "no database" error contracts are preserved. Remaining OPEN
  parity criterion: KGP-Q5 (GraphRAG multi-hop).
- **2026-07-21** — **KGP-Q5 closed** (GraphRAG multi-hop retrieval) — **full
  parity achieved**. `knowledge.query` previously answered only from a single
  `articles` LIKE scan, an approximation of the Python
  `KnowledgeGraphAgent.query()` that never traversed the pack's knowledge graph.
  A new `query_graph(conn, keywords, limit)` now runs **before** the article
  fallback: when the pack exposes both an `entities` and a `relationships` table
  (the SQLite port of upstream `Entity` / `ENTITY_RELATION`), it (1) selects
  keyword-matched **seed** entities — reusing the same parameterized
  `like_contains_pattern` LIKE search and `TITLE_MATCH_WEIGHT`/
  `CONTENT_MATCH_WEIGHT` coverage ranking as `query_articles`, and projecting the
  entity `url` for citations (KGP-Q1) — then (2) traverses `relationships` edges
  breadth-first up to `MAX_GRAPH_HOPS` (2), pulling in **linked** entities the
  keyword never matched, and (3) builds an answer that names those linked
  entities and the relations joining them ("Ownership enables Borrowing; Borrowing
  constrained by Lifetimes"). `query_graph` returns `None` — falling back to the
  article scan, so article-only packs are unchanged — when either graph table is
  absent or no seed entity matches. Edge/entity ids and keywords are all bound as
  parameters (`?n`, reused across both `IN` lists), so the traversal carries no
  injection surface. Descriptions are truncated on a UTF-8 char boundary (KGP-Q6).
  Acceptance tests: `query_graph_traverses_relationships_for_linked_entities`
  (hop-1 linked entity surfaced + cited), `query_graph_reaches_two_hop_neighbor`
  (hop-2 neighbour), `query_graph_returns_none_without_graph_tables`,
  `query_graph_ignores_pack_with_entities_but_no_relationships`,
  `query_graph_returns_none_when_no_seed_entity_matches`,
  `query_graph_empty_url_and_null_url_yield_no_citation`, and
  `native_knowledge_transport_query_graph_surfaces_linked_entity` (end-to-end via
  the RPC transport). With this, **every in-scope parity criterion is DONE** and
  the done-gate (`cargo test --lib native_knowledge` + `cargo test --lib
  knowledge_client`) is green.
