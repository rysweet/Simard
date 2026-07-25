---
title: Knowledge-pack article relevance ranking
description: How the native knowledge reader ranks matching articles within a knowledge pack by keyword coverage — weighting title hits above body hits, de-duplicating query keywords, and truncating by the LIMIT only after a deterministic ordering — so a knowledge.query answer cites the most relevant sources reproducibly.
last_updated: 2026-07-25
owner: simard
doc_type: reference
related:
  - ../architecture/cognitive-memory.md
  - ./recall-precision-hybrid-api.md
---

# Knowledge-pack article relevance ranking

The native, Python-free knowledge reader (`src/native_knowledge.rs`) answers a
`knowledge.query` RPC by searching a pack's SQLite database for articles whose
`title` or `content` match the question's keywords, then returning the top
matches as cited sources. This page documents how those matches are **ranked** —
a recall-quality property of the `knowledge.query` surface that feeds the
reasoner's planning-context enrichment (`enrich_planning_context`).

## The ranking

Candidate articles are gathered by a `LIKE`-based keyword search — any keyword
hit on `title` or `content` qualifies, preserving recall breadth. Candidates are
then ordered by a **keyword-coverage** relevance score, computed in SQL, *before*
the `LIMIT` truncation, so the cap keeps the most on-topic articles rather than
whatever rows the pack database happened to store first:

```
score = Σ over DISTINCT query keywords k of  (title matches k ? 2 : 0)
                                           + (content matches k ? 1 : 0)
ORDER BY score DESC, title ASC
```

- **Coverage.** An article matching more distinct keywords ranks higher.
- **Title weighting.** A keyword in the `title` (`TITLE_MATCH_WEIGHT = 2`) is a
  far stronger topical signal than one buried in a large `content` body
  (`CONTENT_MATCH_WEIGHT = 1`), so a title match outranks a body-only mention.
- **Distinct keywords.** Query keywords are de-duplicated **case-insensitively**
  before scoring (`to_ascii_lowercase`, matching SQLite's ASCII-case-insensitive
  `LIKE`), so a word repeated in the question ("rust ... rust") cannot
  double-count and over-reward an article that merely mentions it once.
- **Deterministic tie-break.** Equal-score articles order by `title ASC`, so
  recall is reproducible run to run instead of falling back to SQLite's
  arbitrary storage (rowid) order.

## Why it matters

Without ranking, SQLite returned matching rows in arbitrary rowid order and the
`LIMIT` kept whatever rows were stored first — an article matching a single
keyword purely because it was inserted earlier could crowd a genuinely on-topic
article (one matching every keyword) out of the results, starving planning of
the most relevant knowledge. The coverage ranking landed in
[#4281](https://github.com/rysweet/Simard/pull/4281); the distinct-keyword dedup
and the deterministic `title ASC` tie-break harden it so the ranking is neither
skewed by a repeated query word nor left non-deterministic among equal-score
matches.

## Scope and compatibility

- The `knowledge.query` wire response shape (`{ answer, sources, confidence }`)
  is **unchanged** — only the *order* of `sources` (and therefore which sources
  survive the `limit`) is affected.
- Packs whose `articles` schema lacks a `url` column, and the `nodes` /
  `entities` fallback tables, are handled exactly as before; the ranking rides on
  the same `title` / `content` columns the match filter already requires.
- This is per-pack, article-level ranking — distinct from the objective→pack
  selection ranking in `src/knowledge_context.rs` (whole-word, distinct-token
  overlap), which chooses *which packs* to consult; this page covers ranking
  *within* a chosen pack.

## Retrieval strategy: hybrid ranking (graph + vector + keyword)

The keyword-coverage ranking above governs the **keyword** retrieval path
(`query_articles`), which is one of three signals `knowledge.query` blends. Per
the operator directive (issue #4321) that retrieval use *the same GraphRAG
method* as the original agent-kgpacks — "no keyword search is not good enough" —
`query_hybrid` runs all three retrievers and **fuses** their rankings rather than
picking a single one:

1. **Graph (KGP-Q5).** When the pack exposes `entities` + `relationships`
   tables, `query_graph` seeds keyword-matched entities and traverses
   relationship edges multi-hop, surfacing linked entities the keyword never
   matched. See `Specs/agent-kgpacks-rs-parity.md`.
2. **Vector semantic search (KGP-Q9).** When the pack ships precomputed
   embeddings — a JSON float array in an `embedding` column (the SQLite-port
   stand-in for the upstream `Section.embedding DOUBLE[768]` vector) —
   `query_vector` ranks articles by **cosine similarity** between the query
   embedding and each stored embedding. Because it ranks by *vector proximity*
   rather than literal token overlap, it can retrieve an on-topic article that
   shares **no keyword** with the question — which the keyword scan misses. The
   query is embedded by the deterministic default embedder `embed_text` (a
   normalized FNV-1a feature-hash), the port of upstream's default
   deterministic-hash embedder. Semantic recall *quality* tracks the pack's
   embedder; wiring a real-model embedder is pack-build-time (out of scope here).
3. **Keyword (KGP-Q4/Q8).** The `LIKE`-based coverage-ranked scan documented
   above.

### Fusion (KGP-Q10)

The three ranked lists are combined by **weighted Reciprocal Rank Fusion**
(`fuse_rankings`): each list `l` with weight `w` contributes `w / (RRF_K + rank)`
(0-based rank, `RRF_K = 60`) to a candidate's fused score, so a candidate several
signals agree on accumulates their contributions and rises to the top. Ranking by
*position* rather than each signal's raw score fuses the heterogeneous scales
(cosine in `[0,1]`, integer keyword coverage, graph hop order) without any
per-signal normalization.

- **Semantic/graph outweigh keyword.** The vector and graph signals are weighted
  above keyword (`HYBRID_VECTOR_WEIGHT = HYBRID_GRAPH_WEIGHT = 1.0` >
  `HYBRID_KEYWORD_WEIGHT = 0.5`), so a semantically-near or graph-linked answer
  outranks an item that merely shares more literal keywords.
- **Identity + citations.** Candidates are matched across lists by their
  case-folded `title`; a citation `url` or non-empty `section` supplied by any
  signal backfills a variant that lacked one, so fusion never strips a citation
  (KGP-Q1).
- **Graceful degradation.** A pack exposing only one signal fuses a single list,
  which RRF returns in its original order — so graph-only, embedding-only, and
  keyword-only packs behave exactly as they did before hybrid ranking. A
  `query_vector`/`query_graph` signal that does not apply (no `embedding` column,
  no graph tables, dimension mismatch, or no query signal) simply contributes an
  empty list.

The `knowledge.query` wire response shape (`{ answer, sources, confidence }`) is
identical regardless of which signals contributed, so callers are unaffected by
the blend.

