#!/usr/bin/env bash
# Outside-in scenario for kgpacks-rs parity criterion KGP-Q10 (hybrid ranking).
# It proves the native Rust knowledge client — Simard's port of the Python
# agent-kgpacks contract — answers a `knowledge.query` by BLENDING all three
# retrieval signals (vector-semantic cosine, GraphRAG multi-hop, and keyword
# coverage) into one fused ranking, the same ranker the original performs, rather
# than committing to a single retrieval path.
#
# Per the operator directive (issue #4321): "No keyword search is not good
# enough. It needs to be the same." The knowledge query now routes through
# `query_hybrid`: it gathers every available signal and, when two or more are
# present, `fuse_signals` normalizes each by its own max and blends them by
# weight (vector = graph = 1.0 > keyword = 0.6), so a semantic/graph match
# outranks literal keyword overlap. When only one signal is available the native
# single-signal output is returned unchanged (nothing to blend), so keyword-only,
# vector-only, and graph-only packs behave exactly as before.
#
# What this proves, without an LLM, via the in-tree native_knowledge tests:
#
#   (a) fuse_signals_normalizes_and_weights_signals — the fusion math in
#       isolation: heterogeneous per-signal score scales are normalized, weighted,
#       and accumulated across signals; a citation url carries over.
#   (b) query_hybrid_blends_semantic_graph_and_keyword — the decisive one: on a
#       fixture where the vector, graph, and keyword signals DISAGREE, the fused
#       ranking places the semantic and graph matches above the keyword-only
#       match and surfaces the graph-only neighbour the keyword scan misses.
#   (c) query_hybrid_semantic_outranks_keyword_only_with_two_signals — a
#       semantic-only article outranks a keyword-only article (vector weight >
#       keyword weight).
#   (d) query_hybrid_single_signal_matches_component_ranking — a single-signal
#       pack ranks identically to its component (no regression).
#   (e) native_knowledge_transport_query_hybrid_ranks_semantic_over_keyword — the
#       semantic match ranks first in the wire response end-to-end through the
#       native transport, where a pure keyword scan would rank the keyword
#       article first.
#
# Each rung asserts the NAMED test actually ran+passed (a cargo filter that
# matches zero tests still exits 0), so a future rename cannot silently turn this
# scenario into a no-op.
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
  'native_knowledge::tests::fuse_signals_normalizes_and_weights_signals' \
  'native_knowledge::tests::query_hybrid_blends_semantic_graph_and_keyword' \
  'native_knowledge::tests::query_hybrid_semantic_outranks_keyword_only_with_two_signals' \
  'native_knowledge::tests::query_hybrid_single_signal_matches_component_ranking' \
  'native_knowledge::tests::native_knowledge_transport_query_hybrid_ranks_semantic_over_keyword'
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected KGP-Q10 test did not run/pass: $t" >&2; cat "$TEST_LOG" >&2; exit 1; }
done

PASSED_LINE="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | tail -1 || true)"
echo "final ${PASSED_LINE:-<no ok line>}"
echo "PASS: kgpacks-rs KGP-Q10 hybrid ranking (agent-kgpacks parity)"
