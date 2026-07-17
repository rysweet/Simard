#!/usr/bin/env bash
# Outside-in scenario for the native knowledge client's RECALL-QUALITY relevance
# ranking (a recall axis of the standing cognition-improvement goal). It proves
# that `native_knowledge::query_articles` ranks candidate articles by keyword
# COVERAGE — weighting a title hit above a content-only mention — so the most
# on-topic article survives the `limit` cut instead of being crowded out by an
# arbitrary earlier-in-table row that matched a single keyword.
#
# Before this fix the query had no `ORDER BY`, so SQLite returned matching rows
# in arbitrary storage (rowid) order: an article inserted earlier that matched a
# single keyword could displace an article matching every keyword, starving the
# reasoner's planning context (enrich_planning_context -> knowledge.query) of the
# most relevant knowledge.
#
# What this proves, without an LLM, via the in-tree native_knowledge tests:
#
#   (a) query_articles_ranks_most_relevant_first — the article covering every
#       query keyword ranks first even when inserted LAST.
#   (b) query_articles_limit_keeps_most_relevant — with limit=1 the single kept
#       result is the full-coverage article, not an arbitrary earlier
#       single-keyword row (the regression this fix targets).
#   (c) query_articles_prefers_title_over_content_match — a title keyword hit
#       outranks a passing content-only mention.
#
# Each rung asserts the NAMED test actually ran+passed (a cargo filter that
# matches zero tests still exits 0), so a future rename cannot silently turn this
# scenario into a no-op.
#
# Reference: Specs/agent-kgpacks-rs-parity.md (native knowledge recall path).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORK="$(mktemp -d /tmp/simard-kgpacks-rs-ranking.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

TEST_LOG="$WORK/native-knowledge-ranking-cargo-test.log"

echo "== kgpacks-rs recall: native knowledge relevance-ranking tests =="
cargo test --lib --locked native_knowledge -- --nocapture \
    >"$TEST_LOG" 2>&1

grep -qE 'test result: ok\.' "$TEST_LOG" \
  || { echo "FAIL: cargo test did not report an ok result" >&2; cat "$TEST_LOG" >&2; exit 1; }
if grep -qE 'test result: FAILED' "$TEST_LOG"; then
  echo "FAIL: one or more native_knowledge tests FAILED" >&2; cat "$TEST_LOG" >&2; exit 1
fi

for t in \
  'native_knowledge::tests::query_articles_ranks_most_relevant_first' \
  'native_knowledge::tests::query_articles_limit_keeps_most_relevant' \
  'native_knowledge::tests::query_articles_prefers_title_over_content_match'
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected relevance-ranking test did not run/pass: $t" >&2; cat "$TEST_LOG" >&2; exit 1; }
done

PASSED_LINE="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | tail -1 || true)"
echo "final ${PASSED_LINE:-<no ok line>}"
echo "PASS: kgpacks-rs native knowledge relevance ranking (recall quality)"
