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
| KGP-Q4 | Keyword search binds parameters instead of string-interpolating LIKE clauses | NEW test: a question keyword containing `%`, `_`, and `'` returns correct rows and cannot alter the SQL | OPEN | `native_knowledge.rs::query_articles` currently interpolates escaped keywords |
| KGP-Q5 | GraphRAG retrieval: traverse entity + relationship tables (multi-hop), not only a single-table LIKE scan | NEW test: a pack fixture with `relationships` yields a graph-grounded answer joining linked entities | OPEN | `native_knowledge.rs::query_articles` comment: "simplified version of the Python `KnowledgeGraphAgent.query()`" |

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

### Machine-checkable finish signal

So the OODA done-gate can certify completion automatically (rather than
re-observing "full parity" as unmeasurable and hard-parking the goal), this
finish condition is bound to a single observable artifact:

- **Tracking issue [rysweet/Simard#4321](https://github.com/rysweet/Simard/issues/4321)** —
  its **CLOSED** state means full parity is reached. #4321 closes exactly when the
  remaining in-scope criteria (**KGP-Q4**, **KGP-T3**, **KGP-Q5**) ship and both
  `cargo test` commands above are green.
- **[`scripts/check-agent-kgpacks-rs-parity-done-gate.sh`](../scripts/check-agent-kgpacks-rs-parity-done-gate.sh)** —
  one command the done-gate can run. It exits `0` (certified complete) when #4321
  is CLOSED, and otherwise exits non-zero after printing the exact remaining
  criteria as the concrete next step, so this goal is never merely "stuck".

The three in-scope criteria above remain **OPEN**; this binding only makes the
*definition of done* automatically verifiable — it does not claim the parity work
is finished.

## Ordered backlog (so the next cycle is never stuck)

Work the OPEN criteria top-to-bottom; each is a self-contained, shippable unit
with its acceptance test already specified above:

1. **KGP-Q4** — parameterize the keyword LIKE search (correctness + removes the
   injection-shaped interpolation). Smallest, highest-confidence next step.
2. **KGP-T3** — reuse an open `Connection` in `conn_cache` (or document the
   path-cache decision and add the reuse test).
3. **KGP-Q5** — GraphRAG multi-hop retrieval over entities + relationships.
   Largest; do last and consider splitting into a fixture step and a traversal
   step.

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
