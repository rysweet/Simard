#!/usr/bin/env bash
# Outside-in scenario for kgpacks-rs parity criterion KGP-Q10 (hybrid ranking).
# It proves the native Rust knowledge client — Simard's port of the Python
# agent-kgpacks contract — answers a `knowledge.query` by BLENDING all three
# retrieval signals (multi-hop graph, vector-semantic cosine, keyword coverage)
# into one fused ranking, rather than selecting a single retrieval path.
#
# Per the operator directive (issue #4321): "No keyword search is not good
# enough. It needs to be the same" GraphRAG method. `query_hybrid` now runs
# `query_graph`, `query_vector`, and `query_articles` and fuses their ranked
# outputs by weighted Reciprocal Rank Fusion (`fuse_rankings`, RRF_K = 60), with
# the vector and graph signals weighted above keyword. A semantically-near or
# graph-linked answer therefore outranks an item that merely shares more literal
# keywords. A pack exposing only one signal fuses a single list — which RRF
# returns in its original order — so graph-only, embedding-only, and keyword-only
# packs behave exactly as before hybrid ranking.
#
# What this proves, without an LLM, via the in-tree native_knowledge tests:
#
#   (a) query_hybrid_fuses_signals_so_semantic_and_graph_outrank_keyword — on a
#       shared fixture where the signals disagree, the top-vector article and a
#       graph-linked entity (both with ZERO keyword overlap) outrank the item
#       with the highest literal-keyword coverage; a single-signal keyword ranker
#       would invert this order.
#   (b) query_hybrid_end_to_end_blends_via_query_pack_db — the blend is what the
#       public query_pack_db path (and thus the RPC handler) runs: a graph-linked
#       entity with no keyword overlap surfaces in the blended sources.
#   (c) fuse_rankings_accumulates_cross_signal_agreement — a candidate several
#       signals agree on outranks a single list's leader (RRF sums contributions).
#   (d) fuse_rankings_backfills_missing_citation_url — identity is the case-folded
#       title; a citation url / non-empty section from any signal survives fusion
#       (KGP-Q1 preserved).
#   (e) fuse_rankings_single_list_preserves_order_and_respects_limit — one signal
#       fuses to its own order, capped at the limit (single-signal packs unchanged).
#
# The same run re-asserts the pre-existing KGP-Q5 (graph), KGP-Q9 (vector), and
# KGP-Q1/Q4/Q8 behaviours are unregressed. Each rung asserts the NAMED test
# actually ran+passed (a cargo filter that matches zero tests still exits 0), so
# a future rename cannot silently turn this scenario into a no-op.
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
  'native_knowledge::tests::query_hybrid_fuses_signals_so_semantic_and_graph_outrank_keyword' \
  'native_knowledge::tests::query_hybrid_end_to_end_blends_via_query_pack_db' \
  'native_knowledge::tests::fuse_rankings_accumulates_cross_signal_agreement' \
  'native_knowledge::tests::fuse_rankings_backfills_missing_citation_url' \
  'native_knowledge::tests::fuse_rankings_single_list_preserves_order_and_respects_limit'
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected KGP-Q10 test did not run/pass: $t" >&2; cat "$TEST_LOG" >&2; exit 1; }
done

PASSED_LINE="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | tail -1 || true)"
echo "final ${PASSED_LINE:-<no ok line>}"
echo "PASS: kgpacks-rs KGP-Q10 hybrid ranking (agent-kgpacks parity)"
