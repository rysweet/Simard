#!/usr/bin/env bash
# Outside-in scenario for kgpacks-rs parity criterion KGP-Q10 (hybrid ranking).
# It proves the native Rust knowledge client — Simard's port of the Python
# agent-kgpacks contract — answers a `knowledge.query` by BLENDING all three
# GraphRAG signals (multi-hop graph + vector-semantic + keyword) into one ranker
# instead of selecting a single retrieval path.
#
# Per the operator directive (issue #4321): "No keyword search is not good
# enough. It needs to be the same." A new `query_hybrid` now fuses the graph,
# vector, and keyword signal rankers with weighted Reciprocal Rank Fusion — the
# semantic and graph signals weighted above the keyword signal — so a source
# that is semantically near (or graph-linked to) the query can outrank one that
# merely shares a literal keyword, and cross-signal agreement rises above any
# single-signal match. `query_open_pack` now retrieves through `query_hybrid`
# rather than the prior graph->vector->keyword first-non-empty dispatch. Every
# signal is optional, so single-signal packs degrade to exactly that signal's
# order (no regression).
#
# What this proves, without an LLM, via the in-tree native_knowledge tests:
#
#   (a) query_hybrid_ranks_semantic_signal_over_keyword_only_match — the
#       decisive KGP-Q10 property: on a shared fixture the semantic-only source
#       outranks the keyword-only source (semantic outweighs literal keyword
#       overlap) and the both-signals source tops the fused list; the fused
#       representative keeps its source-citation URL (KGP-Q1).
#   (b) query_hybrid_degrades_to_single_signal_order — a keyword-only pack (no
#       embeddings, no graph) reduces to that signal's order (no regression).
#   (c) query_hybrid_returns_none_when_no_signal_matches — no matching signal
#       declines so the caller emits the graceful "not found" answer.
#   (d) native_knowledge_transport_query_hybrid_ranks_semantic_over_keyword —
#       end-to-end through the native RPC transport, the semantically-near,
#       keyword-absent article ranks above the keyword-only one.
#
# It also re-asserts the single-signal parity tests still pass through the now
# hybrid-wired path, so the blend did not regress graph (KGP-Q5) or vector
# (KGP-Q9) retrieval:
#
#   (e) native_knowledge_transport_query_graph_surfaces_linked_entity
#   (f) native_knowledge_transport_query_uses_vector_search_when_embeddings_present
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
  'native_knowledge::tests::query_hybrid_ranks_semantic_signal_over_keyword_only_match' \
  'native_knowledge::tests::query_hybrid_degrades_to_single_signal_order' \
  'native_knowledge::tests::query_hybrid_returns_none_when_no_signal_matches' \
  'native_knowledge::tests::native_knowledge_transport_query_hybrid_ranks_semantic_over_keyword' \
  'native_knowledge::tests::native_knowledge_transport_query_graph_surfaces_linked_entity' \
  'native_knowledge::tests::native_knowledge_transport_query_uses_vector_search_when_embeddings_present'
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected KGP-Q10 test did not run/pass: $t" >&2; cat "$TEST_LOG" >&2; exit 1; }
done

PASSED_LINE="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | tail -1 || true)"
echo "final ${PASSED_LINE:-<no ok line>}"
echo "PASS: kgpacks-rs KGP-Q10 hybrid ranking (agent-kgpacks parity)"
