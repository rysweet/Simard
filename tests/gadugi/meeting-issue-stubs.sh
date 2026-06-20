#!/usr/bin/env bash
# qa-team scenario for issue #2309 — meeting-close issue stubs.
#
# Outside-in verification that closing a meeting emits ready-to-file GitHub
# issue stubs under the per-meeting bundle's `issues/` directory. The
# `write_meeting_bundle_emits_issue_stubs_and_markdown_section` test exercises
# the real chokepoint end-to-end: it points SIMARD_MEETINGS_ROOT at a temp dir,
# calls `write_meeting_bundle`, and asserts the `issues/NN-*.md` files and the
# `## Issue stubs` markdown section appear on the real filesystem. The empty
# case asserts the no-op (no `issues/` dir).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# Run the issue-stub unit tests plus the two chokepoint integration tests.
# Substring filters are OR'd by the libtest harness. No `--quiet` so the
# per-test `... ok` lines are emitted for the behavior assertions below.
OUTPUT="$(
  cargo test --lib -- \
    meeting_facilitator::handoff::issue_stubs \
    meeting_facilitator::handoff::persistence::bundle_tests::write_meeting_bundle_emits_issue_stubs_and_markdown_section \
    meeting_facilitator::handoff::persistence::bundle_tests::write_meeting_bundle_empty_handoff_writes_no_issue_stubs \
    2>&1
)"

printf '%s\n' "$OUTPUT"

# Assert the suite passed with zero failures.
printf '%s\n' "$OUTPUT" | grep -E "test result: ok\. [0-9]+ passed; 0 failed" >/dev/null

# Assert the specific behaviors required by issue #2309 actually ran and passed.
printf '%s\n' "$OUTPUT" | grep -F "empty_handoff_writes_no_issues_dir ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "single_action_item_writes_one_file ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "decision_with_rationale_stub_includes_rationale ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "filenames_are_sanitized_and_traversal_safe ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "malicious_description_stays_inside_issues_dir ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "regeneration_clears_stale_stubs ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "write_meeting_bundle_emits_issue_stubs_and_markdown_section ... ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "write_meeting_bundle_empty_handoff_writes_no_issue_stubs ... ok" >/dev/null

echo "[gadugi] meeting-close issue stubs (#2309): all behaviors verified"
