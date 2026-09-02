#!/usr/bin/env bash
# Outside-in scenario for kgpacks-rs parity criterion KGP-M2 (issue #4321 F2):
# the native Rust knowledge client — Simard's port of the Python agent-kgpacks
# contract — must return, from knowledge.pack_info, the same two computed
# booleans the original exposes: `db_exists` (the pack's pack.db database file is
# present) and `urls_file_exists` (the pack's urls.json provenance file is
# present). Native packs keep citations in the database `url` column (KGP-Q1),
# so `urls_file_exists` is truthfully `false` for them — the flag reports genuine
# on-disk state, never a stubbed constant.
#
# What this proves, without an LLM, via the in-tree native_knowledge test:
#
#   native_knowledge_transport_pack_info_reports_computed_file_flags — a pack
#   with pack.db + urls.json reports db_exists=true / urls_file_exists=true; a
#   manifest-only pack (no database, no urls file) reports both false; both are
#   surfaced end-to-end through the knowledge.pack_info RPC handler.
#
# The rung asserts the NAMED test actually ran+passed (a cargo filter that
# matches zero tests still exits 0), so a future rename cannot silently turn this
# scenario into a no-op.
#
# Reference: Specs/agent-kgpacks-rs-parity.md (criterion KGP-M2).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORK="$(mktemp -d /tmp/simard-kgpacks-rs-pack-info-flags.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

TEST_LOG="$WORK/native-knowledge-cargo-test.log"

echo "== kgpacks-rs KGP-M2: pack_info computed db_exists/urls_file_exists tests =="
cargo test --lib --locked native_knowledge -- --nocapture \
    >"$TEST_LOG" 2>&1

grep -qE 'test result: ok\.' "$TEST_LOG" \
  || { echo "FAIL: cargo test did not report an ok result" >&2; cat "$TEST_LOG" >&2; exit 1; }
if grep -qE 'test result: FAILED' "$TEST_LOG"; then
  echo "FAIL: one or more native_knowledge tests FAILED" >&2; cat "$TEST_LOG" >&2; exit 1
fi

for t in \
  'native_knowledge::tests::native_knowledge_transport_pack_info' \
  'native_knowledge::tests::native_knowledge_transport_pack_info_reports_computed_file_flags'
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected KGP-M2 test did not run/pass: $t" >&2; cat "$TEST_LOG" >&2; exit 1; }
done

PASSED_LINE="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | tail -1 || true)"
echo "final ${PASSED_LINE:-<no ok line>}"
echo "PASS: kgpacks-rs KGP-M2 pack_info computed flags (agent-kgpacks parity)"
