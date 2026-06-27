#!/usr/bin/env bash
# qa-team scenario for issue #2321 — meeting REPL UX prong.
#
# Outside-in verification of the two deferred UX features for goal
# `enhance-simard-meeting-experience`:
#
#   1. Unknown-slash-command suggestions ("did you mean '/close'?").
#   2. A grouped, colorized `/help`.
#
# The REPL-level tests drive the REAL interactive loop end-to-end:
# `run_meeting_repl` is fed piped stdin (e.g. "/colse\n/close\n") with a mock
# agent and its captured stdout is asserted — so the behavior is exercised
# through the actual command dispatch, deterministically and with no live LLM
# backend. The parser- and dispatch-level tests pin the narrow detection rules
# (typo distance, file paths, bare empty-payload commands) and the grouped-help
# single-source-of-truth. Substring filters are OR'd by the libtest harness;
# `--quiet` is omitted so the per-test `... ok` lines below can be asserted.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

OUTPUT="$(
  cargo test --lib --locked -- \
    repl_unknown_command_suggests_closest \
    repl_unknown_command_without_match_lists_commands \
    repl_help_is_grouped_into_sections \
    repl_grouped_help_colorizes_section_titles \
    repl_grouped_help_honors_no_color \
    repl_file_path_is_not_treated_as_unknown_command \
    parse_typo_within_distance_two_suggests_closest \
    parse_distant_garbage_is_unknown_with_no_suggestion \
    parse_unknown_preserves_typed_case_in_input \
    parse_file_path_is_conversation_not_unknown \
    parse_common_path_roots_stay_conversation \
    parse_command_typo_with_payload_stays_conversation \
    parse_bare_empty_payload_commands_stay_conversation \
    parse_conversational_slash_with_args_is_untouched \
    parse_exact_commands_are_not_unknown \
    closest_command_respects_distance_threshold \
    help_groups_render_all_user_commands_exactly_once \
    render_help_plain_contains_groups_and_commands \
    chat_routes_unknown_command_with_suggestion \
    chat_help_uses_grouped_reference \
    2>&1
)"

printf '%s\n' "$OUTPUT"

# Assert the suite passed with zero failures.
printf '%s\n' "$OUTPUT" | grep -E "test result: ok\. [0-9]+ passed; 0 failed" >/dev/null

# Assert the specific behaviors required by issue #2321 actually ran and passed.
# Feature 1 — unknown-slash-command suggestions (REPL end-to-end):
printf '%s\n' "$OUTPUT" | grep -F "repl_unknown_command_suggests_closest ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "repl_unknown_command_without_match_lists_commands ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "repl_file_path_is_not_treated_as_unknown_command ... ok" >/dev/null
# Feature 1 — detection rules (parser-level):
printf '%s\n' "$OUTPUT" | grep -F "parse_typo_within_distance_two_suggests_closest ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "parse_distant_garbage_is_unknown_with_no_suggestion ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "parse_file_path_is_conversation_not_unknown ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "parse_common_path_roots_stay_conversation ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "parse_command_typo_with_payload_stays_conversation ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "parse_bare_empty_payload_commands_stay_conversation ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "parse_conversational_slash_with_args_is_untouched ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "parse_exact_commands_are_not_unknown ... ok" >/dev/null
# Feature 2 — grouped, colorized /help:
printf '%s\n' "$OUTPUT" | grep -F "repl_help_is_grouped_into_sections ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "repl_grouped_help_colorizes_section_titles ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "repl_grouped_help_honors_no_color ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "help_groups_render_all_user_commands_exactly_once ... ok" >/dev/null
# Dashboard chat dispatch parity:
printf '%s\n' "$OUTPUT" | grep -F "chat_routes_unknown_command_with_suggestion ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "chat_help_uses_grouped_reference ... ok" >/dev/null

echo "[gadugi] meeting REPL UX (#2321): suggestions + grouped /help verified"
