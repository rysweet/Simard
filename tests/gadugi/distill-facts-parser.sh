#!/usr/bin/env bash
# Outside-in regression gate for issue #2496: the episode->fact
# distillation parser must recover the `{ "facts": [...] }` payload even
# when the Copilot CLI prepends ANSI-colored launch / INFO log lines
# (e.g. `launching copilot`, `NODE_OPTIONS=...`, `\x1b[2m<timestamp>...`)
# ahead of the JSON. Drives the parser unit tests and asserts the
# preamble-tolerance cases pass.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

OUTPUT="$(
  cargo test -p simard --lib \
    memory_consolidation::distillation::unit_tests \
    --no-fail-fast -- --nocapture 2>&1
)"

printf '%s\n' "$OUTPUT"

# Regression cases introduced by the fix.
printf '%s\n' "$OUTPUT" | grep -F "parse_recipe_output_tolerates_ansi_copilot_preamble ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "parse_recipe_output_skips_non_facts_json_log_line ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "parse_recipe_output_handles_braces_inside_string_values ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "strip_ansi_escapes_removes_sgr_and_preserves_text ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "matching_brace_end_ignores_braces_in_strings ... ok" >/dev/null

# Pre-existing clean-input behaviour must remain green (no regression).
printf '%s\n' "$OUTPUT" | grep -F "parse_recipe_output_accepts_plain_object ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "parse_recipe_output_extracts_json_from_prose ... ok" >/dev/null

# Whole suite must pass.
printf '%s\n' "$OUTPUT" | grep -E "test result: ok\. [0-9]+ passed; 0 failed" >/dev/null

echo "distill-facts-parser: PASS"
