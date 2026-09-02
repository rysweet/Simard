#!/usr/bin/env bash
# Outside-in scenario for kgpacks-rs parity criterion KGP-Q9 (vector semantic
# search). It proves the native Rust knowledge client — Simard's port of the
# Python agent-kgpacks contract — answers a `knowledge.query` by
# embedding-cosine vector retrieval over the pack's stored embeddings, the same
# retrieval method as the original, instead of only a keyword LIKE scan.
#
# Per the operator directive (issue #4321): "No keyword search is not good
# enough. It needs to be the same." A new `query_vector` now runs before the
# keyword article fallback (graph -> vector -> keyword): it ranks the pack's
# stored `embedding` vectors by cosine similarity to the query embedding (the
# deterministic default `embed_text`), so it retrieves an on-topic article that
# shares no literal keyword with the question — which a LIKE scan misses. Packs
# without an `embedding` column fall back unchanged to the keyword scan.
#
# What this proves, without an LLM, via the in-tree native_knowledge tests:
#
#   (a) query_vector_retrieves_semantically_near_article_without_keyword_overlap
#       — a near-embedding article with ZERO keyword overlap is retrieved by
#       cosine where the keyword scan (query_articles) returns nothing.
#   (b) query_vector_ranks_by_cosine_descending — cosine ranking is descending
#       and drops orthogonal (unrelated) articles.
#   (c) query_vector_respects_limit — the limit caps the returned sources.
#   (d) query_vector_projects_url_citation_and_empty_url_is_no_citation — the
#       nearest article's url is surfaced (KGP-Q1); an empty url degrades to no
#       citation (None), never Some("").
#   (e) query_vector_returns_none_without_embedding_column — a keyword-only pack
#       falls back to the LIKE scan (no regression).
#   (f) query_vector_returns_none_for_zero_query_and_dimension_mismatch — no
#       query signal or a mismatched dimension declines rather than mis-ranking.
#   (g) embed_text_is_deterministic_and_l2_normalized,
#       cosine_similarity_handles_identity_orthogonal_and_mismatch,
#       parse_embedding_reads_json_array_and_rejects_garbage — the embedder,
#       cosine, and parser primitives.
#   (h) native_knowledge_transport_query_uses_vector_search_when_embeddings_present
#       — the vector-ranked article appears first in the wire response end-to-end
#       through the native transport.
#
# Each rung asserts the NAMED test actually ran+passed (a cargo filter that
# matches zero tests still exits 0), so a future rename cannot silently turn
# this scenario into a no-op.
#
# Reference: Specs/agent-kgpacks-rs-parity.md (criterion KGP-Q9).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORK="$(mktemp -d /tmp/simard-kgpacks-rs-vector.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

TEST_LOG="$WORK/native-knowledge-vector-cargo-test.log"

echo "== kgpacks-rs KGP-Q9: native knowledge client vector semantic-search tests =="
cargo test --lib --locked native_knowledge -- --nocapture \
    >"$TEST_LOG" 2>&1

grep -qE 'test result: ok\.' "$TEST_LOG" \
  || { echo "FAIL: cargo test did not report an ok result" >&2; cat "$TEST_LOG" >&2; exit 1; }
if grep -qE 'test result: FAILED' "$TEST_LOG"; then
  echo "FAIL: one or more native_knowledge tests FAILED" >&2; cat "$TEST_LOG" >&2; exit 1
fi

for t in \
  'native_knowledge::tests::query_vector_retrieves_semantically_near_article_without_keyword_overlap' \
  'native_knowledge::tests::query_vector_ranks_by_cosine_descending' \
  'native_knowledge::tests::query_vector_respects_limit' \
  'native_knowledge::tests::query_vector_projects_url_citation_and_empty_url_is_no_citation' \
  'native_knowledge::tests::query_vector_returns_none_without_embedding_column' \
  'native_knowledge::tests::query_vector_returns_none_for_zero_query_and_dimension_mismatch' \
  'native_knowledge::tests::embed_text_is_deterministic_and_l2_normalized' \
  'native_knowledge::tests::cosine_similarity_handles_identity_orthogonal_and_mismatch' \
  'native_knowledge::tests::parse_embedding_reads_json_array_and_rejects_garbage' \
  'native_knowledge::tests::native_knowledge_transport_query_uses_vector_search_when_embeddings_present'
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected KGP-Q9 test did not run/pass: $t" >&2; cat "$TEST_LOG" >&2; exit 1; }
done

PASSED_LINE="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | tail -1 || true)"
echo "final ${PASSED_LINE:-<no ok line>}"
echo "PASS: kgpacks-rs KGP-Q9 vector semantic search (agent-kgpacks parity)"
