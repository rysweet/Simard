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
printf '%s\n' "$LIST_OUTPUT" | grep -F "interactive-terminal-driving" >/dev/null

# Issue #2087: the default list is the high-signal V1 core set — only the four
# spec-mandated classes, and NONE of the opt-in extended classes.
printf '%s\n' "$LIST_OUTPUT" | grep -F "class=repo-exploration" >/dev/null
printf '%s\n' "$LIST_OUTPUT" | grep -F "class=documentation" >/dev/null
printf '%s\n' "$LIST_OUTPUT" | grep -F "class=safe-code-change" >/dev/null
printf '%s\n' "$LIST_OUTPUT" | grep -F "class=session-quality" >/dev/null
if printf '%s\n' "$LIST_OUTPUT" | grep -F "class=chaos-engineering" >/dev/null; then
  echo "FAIL: default gym list leaked an extended class (chaos-engineering)" >&2
  exit 1
fi

# The extended classes must remain reachable via explicit opt-in.
EXTENDED_LIST_OUTPUT="$(
  cargo run --quiet --bin simard-gym -- list extended
)"
printf '%s\n' "$EXTENDED_LIST_OUTPUT" | grep -F "class=chaos-engineering" >/dev/null
printf '%s\n' "$EXTENDED_LIST_OUTPUT" | grep -F "class=event-sourcing" >/dev/null
# Extended must be a strict superset of core.
CORE_COUNT="$(printf '%s\n' "$LIST_OUTPUT" | grep -c '^- ')"
EXTENDED_COUNT="$(printf '%s\n' "$EXTENDED_LIST_OUTPUT" | grep -c '^- ')"
[ "$EXTENDED_COUNT" -gt "$CORE_COUNT" ]

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
printf '%s\n' "$SUITE_OUTPUT" | grep -F "interactive-terminal-driving: passed" >/dev/null

SUITE_REPORT="$(printf '%s\n' "$SUITE_OUTPUT" | sed -n 's/^Suite artifact report: //p')"
[ -n "$SUITE_REPORT" ]
[ -f "$SUITE_REPORT" ]

grep -F '"suite_id": "starter"' "$SUITE_REPORT" >/dev/null
grep -F '"passed": true' "$SUITE_REPORT" >/dev/null
