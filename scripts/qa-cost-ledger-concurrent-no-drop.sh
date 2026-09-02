#!/usr/bin/env bash
# qa-cost-ledger-concurrent-no-drop.sh
#
# Regression gate for the intermittent `verify` failure in
# `base_type_copilot::tests::meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective`
# ("a copilot-meeting cost entry for this session must be recorded").
#
# Root cause (fixed): `cost_tracking::write_entry` appended each JSONL record
# with `writeln!(file, "{}", line)`, which emits the JSON payload and the
# trailing newline as two SEPARATE `write()` syscalls. The lib test binary runs
# every test in one multi-threaded process, and `record_cost` resolves the
# ledger from the process-global `HOME`. When the meeting-cost test redirects
# `HOME` to a temp ledger, any parallel, non-serialized test that also calls
# `record_cost` (e.g. the lightweight-chat adapter turn) appends to the SAME
# file concurrently. Two interleaved two-syscall writes splice their bytes onto
# a single line (`{a}{b}\n\n`); the spliced line fails JSON parse and the entry
# is silently dropped by the `serde_json::from_str(...).ok()` read filter — so
# the meeting test observed "file exists but my entry is missing" and flaked.
#
# The fix (`src/cost_tracking.rs`) routes every append through `append_line`,
# which (a) serializes in-process writers with a process-wide `Mutex` and
# (b) writes the whole record (payload + newline) in a SINGLE buffered
# `write_all`, so concurrent appends can never tear a line.
#
# This script is a hermetic, network-free pass/fail gate. It:
#   1. Runs the deterministic concurrency regression that hammers the real
#      ledger writer from many threads and asserts every line round-trips and
#      no entry is dropped.
#   2. Structurally guards the production write site so a future edit cannot
#      quietly revert to the non-atomic `writeln!` append without also deleting
#      these lines.
#
# A non-zero exit on any failed assertion is treated by the gadugi `cli` agent
# runner as a step failure, so this is a real gate (not a cosmetic assertion).
set -uo pipefail

fail() {
  echo "QA-COST-LEDGER-CONCURRENCY: FAIL - $1"
  exit 1
}

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || fail "cannot cd to repo root"

# 1. Deterministic concurrency regression (no network, no daemon). Fails loudly
#    if concurrent ledger appends interleave or drop entries.
TEST="cost_tracking::tests::concurrent_appends_never_interleave_or_drop_entries"
echo "QA-COST-LEDGER-CONCURRENCY: running hermetic concurrency regression ($TEST)…"
if ! cargo test --quiet --lib -- --exact "$TEST"; then
  fail "concurrent cost-ledger append regression test failed (interleaved or dropped entries)"
fi

# 2. Structural source-contract guards on the production write site. Every
#    ledger append must go through the atomic `append_line` helper, which holds
#    the process-wide lock and writes the whole record with a single
#    `write_all`. The non-atomic `writeln!` append must NOT return.
SRC="src/cost_tracking.rs"
grep -q "static LEDGER_WRITE_LOCK: Mutex<()> = Mutex::new(());" "$SRC" \
  || fail "the process-wide ledger write lock (LEDGER_WRITE_LOCK) is gone — concurrent appends can interleave again"
grep -q "file.write_all(record.as_bytes())?;" "$SRC" \
  || fail "append_line no longer writes the whole record with a single write_all — a line can be torn across syscalls"
if grep -qE "writeln!\(file, \"\{\}\", line\)" "$SRC"; then
  fail "cost ledger append reverted to the non-atomic writeln! writer — the concurrent-append corruption flake is reintroduced"
fi

echo "QA-COST-LEDGER-CONCURRENCY: PASS - concurrent cost-ledger appends are atomic; no interleaving or dropped entries"
