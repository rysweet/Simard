#!/usr/bin/env bash
# qa-team scenario for the meeting live capture-count tally.
#
# Outside-in verification that the interactive meeting REPL prints a running
# `[meeting] captured:` tally after each structured capture command
# (/decision, /action, /question, /risk, /disagree), bringing the interactive
# path to parity with the batch meeting probe's capture-count summary.
#
# The REPL needs a live LLM provider to start, so it cannot be driven
# hermetically through the binary. Instead we exercise the real chokepoint:
# `run_meeting_repl` is driven directly with a deterministic mock agent in
# `meeting_repl::tests_repl::repl_*_tally*`, asserting the exact tally text,
# per-category counts, pluralization, and the plain-text (grep-safe) contract.
# We also assert the `[meeting] captured:` marker is wired into the REPL
# implementation and documented in the operator howto.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# Run the capture-tally REPL tests. Substring filters are OR'd by the libtest
# harness. No `--quiet` so the per-test `... ok` lines are emitted for the
# behavior assertions below.
OUTPUT="$(
  cargo test --lib -- \
    meeting_repl::tests_repl::repl_decision_emits_capture_tally \
    meeting_repl::tests_repl::repl_capture_tally_counts_each_category_and_pluralizes \
    meeting_repl::tests_repl::repl_capture_tally_is_plain_text_for_grep \
    2>&1
)"

printf '%s\n' "$OUTPUT"

# Assert the suite passed with zero failures.
printf '%s\n' "$OUTPUT" | grep -E "test result: ok\. [0-9]+ passed; 0 failed" >/dev/null

# Assert the specific behaviors actually ran and passed.
printf '%s\n' "$OUTPUT" | grep -F "repl_decision_emits_capture_tally ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "repl_capture_tally_counts_each_category_and_pluralizes ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "repl_capture_tally_is_plain_text_for_grep ... ok" >/dev/null

# Contract: the greppable marker must be wired into the REPL implementation.
grep -F "[meeting] captured:" src/meeting_repl/repl.rs >/dev/null

# Contract: the operator howto must document the tally for discoverability.
grep -F "[meeting] captured:" docs/howto/start-a-meeting.md >/dev/null

echo "[gadugi] meeting capture tally: all behaviors verified"
