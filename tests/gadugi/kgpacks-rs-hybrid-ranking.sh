#!/usr/bin/env bash
# Outside-in scenario for kgpacks-rs parity criterion KGP-Q10 (hybrid ranking).
# It proves the native Rust knowledge client — Simard's port of the Python
# agent-kgpacks contract — answers a `knowledge.query` by a HYBRID GraphRAG
# ranker that blends the three retrieval signals (vector-semantic, graph
# multi-hop, keyword-coverage) into one fused ranking, instead of selecting a
# single retrieval path.
#
# Per the operator directive (issue #4321): "No keyword search is not good
# enough. It needs to be the same." A new `query_hybrid` now replaces the
# single-path selector in `query_open_pack`: it gathers candidates from
# `query_vector_scored` (cosine), `query_graph_scored` (hop-decayed graph
# score), and `query_articles_scored` (keyword coverage), normalizes each to
# [0,1], keys candidates by title so a source found by several signals blends
# into one (preserving its `url` citation), and ranks by the equal-weighted sum.
# A candidate strong in semantic+graph therefore outranks one that merely shares
# a literal keyword — the reverse of the keyword-only order. Packs the
# vector/graph signals cannot serve reduce exactly to the keyword ranking.
#
# What this proves, without an LLM, via the in-tree native_knowledge tests:
#
#   (a) query_hybrid_ranks_semantic_and_graph_above_literal_keyword_overlap
#       — the decisive one: on a shared fixture the blended vector+graph
#       candidate outranks the literal-keyword candidate (the reverse of the
#       keyword-only order) and keeps its citation url.
#   (b) query_hybrid_matches_keyword_ranking_when_only_keyword_signal_fires
#       — a keyword-only pack (no graph tables, no embeddings) reduces exactly to
#       the keyword-coverage ranking (no regression).
#   (c) graph_hop_score_decays_with_distance — the graph signal weights a seed
#       above a one-hop above a two-hop neighbour.
#   (d) native_knowledge_transport_query_hybrid_blends_signals_end_to_end
#       — the fused ranking surfaces graph-linked and semantically-near sources
#       together in the wire response end-to-end through the native transport.
#
# Each rung asserts the NAMED test actually ran+passed (a cargo filter that
# matches zero tests still exits 0), so a future rename cannot silently turn
# this scenario into a no-op.
#
# Reference: Specs/agent-kgpacks-rs-parity.md (criterion KGP-Q10).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORK="$(mktemp -d /tmp/simard-kgpacks-rs-hybrid.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

TEST_LOG="$WORK/native-knowledge-hybrid-cargo-test.log"

echo "== kgpacks-rs KGP-Q10: native knowledge client hybrid-ranking tests =="
cargo test --lib --locked native_knowledge -- --nocapture \
    >"$TEST_LOG" 2>&1

grep -qE 'test result: ok\.' "$TEST_LOG" \
  || { echo "FAIL: cargo test did not report an ok result" >&2; cat "$TEST_LOG" >&2; exit 1; }
if grep -qE 'test result: FAILED' "$TEST_LOG"; then
  echo "FAIL: one or more native_knowledge tests FAILED" >&2; cat "$TEST_LOG" >&2; exit 1
fi

for t in \
  'native_knowledge::tests::query_hybrid_ranks_semantic_and_graph_above_literal_keyword_overlap' \
  'native_knowledge::tests::query_hybrid_matches_keyword_ranking_when_only_keyword_signal_fires' \
  'native_knowledge::tests::graph_hop_score_decays_with_distance' \
  'native_knowledge::tests::native_knowledge_transport_query_hybrid_blends_signals_end_to_end'
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected KGP-Q10 test did not run/pass: $t" >&2; cat "$TEST_LOG" >&2; exit 1; }
done

PASSED_LINE="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | tail -1 || true)"
echo "final ${PASSED_LINE:-<no ok line>}"
echo "PASS: kgpacks-rs KGP-Q10 hybrid ranking (agent-kgpacks parity)"
