#!/usr/bin/env bash
# Outside-in scenario for KGP-Q4 (query safety / better-than-original hardening,
# issue #4321 F9). It proves that the native Rust knowledge client's keyword
# search binds each keyword as a `LIKE ?N ESCAPE '\'` parameter instead of
# string-interpolating it into the SQL, so a keyword containing LIKE
# metacharacters (`%`, `_`) or an injection-shaped payload (`'`, `; DROP …`) is a
# harmless LITERAL search that cannot alter or execute SQL.
#
# The Python reference (agent-kgpacks) interpolated escaped strings into the
# LIKE clause; the Rust port binds parameters — the "or better" clause of the
# parity done-gate.
#
# What this proves, without an LLM, via the in-tree native_knowledge tests:
#
#   (a) query_articles_matches_like_metacharacters_literally — a keyword with
#       `%`/`_` matches literally (no wildcard over-matching).
#   (b) query_articles_keyword_cannot_alter_sql — a tautology keyword matches
#       nothing and a DROP-shaped keyword never executes: the table survives and
#       still answers real queries.
#
# Each rung asserts the NAMED test actually ran+passed (a cargo filter that
# matches zero tests still exits 0), so a future rename cannot silently turn this
# scenario into a no-op.
#
# Reference: Specs/agent-kgpacks-rs-parity.md (KGP-Q4, native knowledge query path).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORK="$(mktemp -d /tmp/simard-kgpacks-rs-query-safety.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

TEST_LOG="$WORK/native-knowledge-query-safety-cargo-test.log"

echo "== kgpacks-rs query safety: native knowledge KGP-Q4 tests =="
cargo test --lib --locked native_knowledge -- --nocapture \
    >"$TEST_LOG" 2>&1

grep -qE 'test result: ok\.' "$TEST_LOG" \
  || { echo "FAIL: cargo test did not report an ok result" >&2; cat "$TEST_LOG" >&2; exit 1; }
if grep -qE 'test result: FAILED' "$TEST_LOG"; then
  echo "FAIL: one or more native_knowledge tests FAILED" >&2; cat "$TEST_LOG" >&2; exit 1
fi

for t in \
  'native_knowledge::tests::query_articles_matches_like_metacharacters_literally' \
  'native_knowledge::tests::query_articles_keyword_cannot_alter_sql'
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected query-safety test did not run/pass: $t" >&2; cat "$TEST_LOG" >&2; exit 1; }
done

PASSED_LINE="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | tail -1 || true)"
echo "final ${PASSED_LINE:-<no ok line>}"
echo "PASS: kgpacks-rs native knowledge query safety (KGP-Q4 parameterized LIKE)"
