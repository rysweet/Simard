#!/usr/bin/env bash
# Outside-in scenario for kgpacks-rs parity criterion KGP-Q5 (GraphRAG
# multi-hop retrieval). It proves the native Rust knowledge client — Simard's
# port of the Python agent-kgpacks contract — answers a `knowledge.query` by
# traversing the pack's knowledge graph (its `entities` + `relationships`
# tables) instead of only running a single-table LIKE scan.
#
# Before this fix `knowledge.query` answered solely from an `articles` LIKE
# scan, an approximation of the Python `KnowledgeGraphAgent.query()` that never
# followed relationship edges. A new `query_graph` now runs before the article
# fallback: it seeds keyword-matched entities, traverses `relationships`
# breadth-first up to MAX_GRAPH_HOPS (2), and grounds the answer in the linked
# entities and the relations joining them. Packs without graph tables fall back
# unchanged to the article scan.
#
# What this proves, without an LLM, via the in-tree native_knowledge tests:
#
#   (a) query_graph_traverses_relationships_for_linked_entities — a query that
#       only matches "Ownership" by keyword surfaces the graph-linked "Borrowing"
#       (hop 1) entity plus its citation URL, proving a graph join (not a LIKE
#       scan).
#   (b) query_graph_reaches_two_hop_neighbor — the "Lifetimes" entity, two
#       relationship hops from the seed, is retrieved (multi-hop traversal).
#   (c) query_graph_returns_none_without_graph_tables — an article-only pack
#       falls back to the single-table scan (no graph path, no regression).
#   (d) query_graph_ignores_pack_with_entities_but_no_relationships — half a
#       graph (entities but no relationships table) is not traversed.
#   (e) query_graph_returns_none_when_no_seed_entity_matches — graph tables with
#       no keyword-matched seed fall back rather than emitting an empty answer.
#   (f) query_graph_empty_url_and_null_url_yield_no_citation — empty and NULL
#       entity urls degrade to no citation (None), never Some("").
#   (g) native_knowledge_transport_query_graph_surfaces_linked_entity — the
#       graph-linked entity appears in the wire response end-to-end through the
#       native transport.
#
# Each rung asserts the NAMED test actually ran+passed (a cargo filter that
# matches zero tests still exits 0), so a future rename cannot silently turn
# this scenario into a no-op.
#
# Reference: Specs/agent-kgpacks-rs-parity.md (criterion KGP-Q5).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORK="$(mktemp -d /tmp/simard-kgpacks-rs-graphrag.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

TEST_LOG="$WORK/native-knowledge-graphrag-cargo-test.log"

echo "== kgpacks-rs KGP-Q5: native knowledge client GraphRAG multi-hop tests =="
cargo test --lib --locked native_knowledge -- --nocapture \
    >"$TEST_LOG" 2>&1

grep -qE 'test result: ok\.' "$TEST_LOG" \
  || { echo "FAIL: cargo test did not report an ok result" >&2; cat "$TEST_LOG" >&2; exit 1; }
if grep -qE 'test result: FAILED' "$TEST_LOG"; then
  echo "FAIL: one or more native_knowledge tests FAILED" >&2; cat "$TEST_LOG" >&2; exit 1
fi

for t in \
  'native_knowledge::tests::query_graph_traverses_relationships_for_linked_entities' \
  'native_knowledge::tests::query_graph_reaches_two_hop_neighbor' \
  'native_knowledge::tests::query_graph_returns_none_without_graph_tables' \
  'native_knowledge::tests::query_graph_ignores_pack_with_entities_but_no_relationships' \
  'native_knowledge::tests::query_graph_returns_none_when_no_seed_entity_matches' \
  'native_knowledge::tests::query_graph_empty_url_and_null_url_yield_no_citation' \
  'native_knowledge::tests::native_knowledge_transport_query_graph_surfaces_linked_entity'
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected KGP-Q5 test did not run/pass: $t" >&2; cat "$TEST_LOG" >&2; exit 1; }
done

PASSED_LINE="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | tail -1 || true)"
echo "final ${PASSED_LINE:-<no ok line>}"
echo "PASS: kgpacks-rs KGP-Q5 GraphRAG multi-hop retrieval (agent-kgpacks parity)"
