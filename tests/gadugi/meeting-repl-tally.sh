#!/usr/bin/env bash
# qa-team scenario for issue #2376 — meeting REPL live capture-count tally.
#
# Outside-in verification that the interactive meeting REPL prints a compact,
# grep-safe running tally after every structured-capture command
# (`/decision`, `/action`, `/question`, `/risk`, `/disagree`). The deterministic
# unit + integration tests in `src/meeting_repl/tests_repl.rs` exercise the real
# chokepoint: the pure `format_capture_tally` formatter (counts, pluralization,
# category order, grep-safety) and the emission/accumulation behaviour driven
# end-to-end through `run_meeting_repl` with a mock agent (no LLM).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# Run the tally tests. Substring filters are OR'd by the libtest harness. No
# `--quiet` so the per-test `... ok` lines are emitted for the assertions below.
OUTPUT="$(
  cargo test --lib -- \
    meeting_repl::tests_repl::format_capture_tally \
    meeting_repl::tests_repl::repl_decision_command_emits_capture_tally \
    meeting_repl::tests_repl::repl_emits_a_tally_after_every_structured_capture_command \
    meeting_repl::tests_repl::repl_tally_counts_accumulate_within_a_category \
    2>&1
)"

printf '%s\n' "$OUTPUT"

# Assert the suite passed with zero failures.
printf '%s\n' "$OUTPUT" | grep -E "test result: ok\. [0-9]+ passed; 0 failed" >/dev/null

# Assert the specific behaviors required by issue #2376 actually ran and passed.
printf '%s\n' "$OUTPUT" | grep -F "format_capture_tally_all_zero_uses_plural_nouns ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "format_capture_tally_uses_singular_nouns_for_one ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "format_capture_tally_uses_plural_nouns_for_many ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "format_capture_tally_orders_categories_like_state_view ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "format_capture_tally_is_grep_safe_single_line_plain_text ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "repl_decision_command_emits_capture_tally ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "repl_emits_a_tally_after_every_structured_capture_command ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "repl_tally_counts_accumulate_within_a_category ... ok" >/dev/null

echo "[gadugi] meeting REPL capture-count tally (#2376): all behaviors verified"
