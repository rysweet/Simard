#!/usr/bin/env bash
# Outside-in scenario for kgpacks-rs parity criterion KGP-T3 (connection reuse).
# It proves the native Rust knowledge client — Simard's port of the Python
# agent-kgpacks contract — reuses one open read-only SQLite connection per pack
# across repeated `knowledge.query` calls instead of re-opening (and re-parsing
# the schema of) the database on every request.
#
# Before this fix the handler cached only each pack's database *path* and opened
# a fresh `Connection` per query. It now caches the live connection itself in a
# `ConnCache` (one `Arc<Mutex<Connection>>` per pack), returning the same handle
# on subsequent queries.
#
# What this proves, without an LLM, via the in-tree native_knowledge tests:
#
#   (a) conn_cache_reuses_open_connection_across_queries — a second query to one
#       pack returns the SAME connection handle (Arc::ptr_eq) and the miss-only
#       path resolver does not re-run on a cache hit.
#   (b) conn_cache_keeps_distinct_connections_per_pack — different packs get
#       independent connections, never a shared handle.
#   (c) conn_cache_propagates_resolve_error_without_caching — a failed resolve
#       surfaces the error and does not poison the cache.
#   (d) native_knowledge_transport_repeated_query_reuses_connection — two
#       knowledge.query RPC calls to one pack both succeed against the reused
#       connection, end-to-end through the native transport.
#
# Each rung asserts the NAMED test actually ran+passed (a cargo filter that
# matches zero tests still exits 0), so a future rename cannot silently turn
# this scenario into a no-op.
#
# Reference: Specs/agent-kgpacks-rs-parity.md (criterion KGP-T3).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORK="$(mktemp -d /tmp/simard-kgpacks-rs-connreuse.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

TEST_LOG="$WORK/native-knowledge-connreuse-cargo-test.log"

echo "== kgpacks-rs KGP-T3: native knowledge client connection-reuse tests =="
cargo test --lib --locked native_knowledge -- --nocapture \
    >"$TEST_LOG" 2>&1

grep -qE 'test result: ok\.' "$TEST_LOG" \
  || { echo "FAIL: cargo test did not report an ok result" >&2; cat "$TEST_LOG" >&2; exit 1; }
if grep -qE 'test result: FAILED' "$TEST_LOG"; then
  echo "FAIL: one or more native_knowledge tests FAILED" >&2; cat "$TEST_LOG" >&2; exit 1
fi

for t in \
  'native_knowledge::tests::conn_cache_reuses_open_connection_across_queries' \
  'native_knowledge::tests::conn_cache_keeps_distinct_connections_per_pack' \
  'native_knowledge::tests::conn_cache_propagates_resolve_error_without_caching' \
  'native_knowledge::tests::native_knowledge_transport_repeated_query_reuses_connection'
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected KGP-T3 test did not run/pass: $t" >&2; cat "$TEST_LOG" >&2; exit 1; }
done

PASSED_LINE="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | tail -1 || true)"
echo "final ${PASSED_LINE:-<no ok line>}"
echo "PASS: kgpacks-rs KGP-T3 connection reuse (agent-kgpacks parity)"
