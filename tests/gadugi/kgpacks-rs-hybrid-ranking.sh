#!/usr/bin/env bash
# Outside-in scenario for kgpacks-rs parity criterion KGP-Q10 (hybrid ranking).
# It proves the native Rust knowledge client — Simard's port of the Python
# agent-kgpacks contract — answers a `knowledge.query` by BLENDING the
# vector-semantic, graph, and keyword signals into one fused ranking (the same
# ranker the original performs) instead of selecting a single retrieval path.
#
# Per the operator directive (issue #4321): "No keyword search is not good
# enough. It needs to be the same." Hybrid fusion is the last REQUIRED
# retrieval-parity row (multi-hop graph KGP-Q5 and vector semantic search
# KGP-Q9 are already DONE). Each retrieval function now has a scored core
# (query_articles_scored / query_vector_scored / query_graph_scored); a new
# query_hybrid gathers each signal's scored candidates and fuse_signal
# min-max-normalizes each signal and accumulates a weighted score per candidate,
# with the vector (0.5) and graph (0.3) weights above the keyword weight (0.2)
# so a multi-signal candidate is boosted above a single-signal one and a
# semantically-relevant candidate outranks one that only shares a literal
# keyword.
#
# What this proves, without an LLM, via the in-tree native_knowledge tests:
#
#   (a) query_hybrid_fuses_semantic_over_literal_keyword — the decisive one: on
#       a three-article fixture isolating the vector-only, keyword-only, and
#       both-signal cases, the both-signal article ranks first (fusion boost)
#       and the semantic-only article outranks the keyword-only one (semantic
#       outweighs literal keyword overlap) — an order no single signal produces.
#   (b) native_knowledge_transport_query_hybrid_blends_signals — the same fused
#       order and candidate set appear in the wire response end-to-end through
#       the native RPC transport.
#
# The same run re-asserts the pre-existing KGP-Q1/Q4/Q5/Q8/Q9/T3 citation,
# parameterized-search, GraphRAG, ranking, vector, and connection-reuse
# behaviour is unregressed (the whole native_knowledge suite must be green).
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
  'native_knowledge::tests::query_hybrid_fuses_semantic_over_literal_keyword' \
  'native_knowledge::tests::query_hybrid_answer_stays_grounded_when_graph_is_truncated_out' \
  'native_knowledge::tests::native_knowledge_transport_query_hybrid_blends_signals'
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected KGP-Q10 test did not run/pass: $t" >&2; cat "$TEST_LOG" >&2; exit 1; }
done

PASSED_LINE="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | tail -1 || true)"
echo "final ${PASSED_LINE:-<no ok line>}"
echo "PASS: kgpacks-rs KGP-Q10 hybrid ranking (agent-kgpacks parity)"
