#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

LIST_OUTPUT="$(
  cargo run --quiet --bin simard-gym -- list
)"

printf '%s\n' "$LIST_OUTPUT"
printf '%s\n' "$LIST_OUTPUT" | grep -F "repo-exploration-local" >/dev/null
printf '%s\n' "$LIST_OUTPUT" | grep -F "docs-refresh-copilot" >/dev/null
printf '%s\n' "$LIST_OUTPUT" | grep -F "safe-code-change-rusty-clawd" >/dev/null
printf '%s\n' "$LIST_OUTPUT" | grep -F "composite-session-review" >/dev/null

SUITE_OUTPUT="$(
  cargo run --quiet --bin simard-gym -- run-suite starter
)"

printf '%s\n' "$SUITE_OUTPUT"
printf '%s\n' "$SUITE_OUTPUT" | grep -F "Suite: starter" >/dev/null
printf '%s\n' "$SUITE_OUTPUT" | grep -F "Suite passed: true" >/dev/null
printf '%s\n' "$SUITE_OUTPUT" | grep -F "repo-exploration-local: passed" >/dev/null
printf '%s\n' "$SUITE_OUTPUT" | grep -F "docs-refresh-copilot: passed" >/dev/null
printf '%s\n' "$SUITE_OUTPUT" | grep -F "safe-code-change-rusty-clawd: passed" >/dev/null
printf '%s\n' "$SUITE_OUTPUT" | grep -F "composite-session-review: passed" >/dev/null

SUITE_REPORT="$(printf '%s\n' "$SUITE_OUTPUT" | sed -n 's/^Suite artifact report: //p')"
[ -n "$SUITE_REPORT" ]
[ -f "$SUITE_REPORT" ]

grep -F '"suite_id": "starter"' "$SUITE_REPORT" >/dev/null
grep -F '"passed": true' "$SUITE_REPORT" >/dev/null
