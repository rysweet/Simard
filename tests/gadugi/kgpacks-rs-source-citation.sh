#!/usr/bin/env bash
# Outside-in scenario for kgpacks-rs parity criterion KGP-Q1 (source-citation
# URLs). It proves the native Rust knowledge client — Simard's port of the
# Python agent-kgpacks contract — surfaces each matched article's source URL so
# answers trace back to a specific source (the agent-kgpacks guarantee), while
# degrading gracefully to no citation for packs whose schema has no `url`
# column.
#
# What this proves, without an LLM, via the in-tree native_knowledge tests:
#
#   (a) query_pack_db_returns_source_urls_when_present — a pack whose `articles`
#       table carries a `url` column yields SourceInfo citations with that URL.
#   (b) query_pack_db_treats_empty_url_as_no_citation — a present-but-empty URL
#       degrades to None (not Some("")).
#   (c) query_pack_db_omits_urls_when_column_absent — a urlless pack schema still
#       returns matches with no citation URL, never an error (back-compat).
#   (d) native_knowledge_transport_query_surfaces_source_url — the URL propagates
#       end-to-end through the knowledge.query RPC handler into sources[].url.
#
# Each rung asserts the NAMED test actually ran+passed (a cargo filter that
# matches zero tests still exits 0), so a future rename cannot silently turn
# this scenario into a no-op.
#
# Reference: Specs/agent-kgpacks-rs-parity.md (criterion KGP-Q1 / KGP-P1).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORK="$(mktemp -d /tmp/simard-kgpacks-rs-citation.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

TEST_LOG="$WORK/native-knowledge-cargo-test.log"

echo "== kgpacks-rs KGP-Q1: native knowledge client source-citation URL tests =="
cargo test --lib --locked native_knowledge -- --nocapture \
    >"$TEST_LOG" 2>&1

grep -qE 'test result: ok\.' "$TEST_LOG" \
  || { echo "FAIL: cargo test did not report an ok result" >&2; cat "$TEST_LOG" >&2; exit 1; }
if grep -qE 'test result: FAILED' "$TEST_LOG"; then
  echo "FAIL: one or more native_knowledge tests FAILED" >&2; cat "$TEST_LOG" >&2; exit 1
fi

for t in \
  'native_knowledge::tests::query_pack_db_returns_source_urls_when_present' \
  'native_knowledge::tests::query_pack_db_treats_empty_url_as_no_citation' \
  'native_knowledge::tests::query_pack_db_omits_urls_when_column_absent' \
  'native_knowledge::tests::native_knowledge_transport_query_surfaces_source_url'
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected KGP-Q1 test did not run/pass: $t" >&2; cat "$TEST_LOG" >&2; exit 1; }
done

PASSED_LINE="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | tail -1 || true)"
echo "final ${PASSED_LINE:-<no ok line>}"
echo "PASS: kgpacks-rs KGP-Q1 source-citation URLs (agent-kgpacks parity)"
