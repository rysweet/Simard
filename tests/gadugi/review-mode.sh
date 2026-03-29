#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    review-run local-harness single-process \
    "inspect the current operator flow and preserve reviewable artifacts"
)"
READ_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    review-read local-harness single-process
)"

printf '%s\n' "$OUTPUT"
printf '%s\n' "$READ_OUTPUT"

printf '%s\n' "$OUTPUT" | grep -F "Probe mode: review-run" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Identity: simard-engineer" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Selected base type: local-harness" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Topology: single-process" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Review proposals: " >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Capture denser execution evidence" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Promote this pattern into a repeatable benchmark" >/dev/null

REVIEW_ARTIFACT="$(printf '%s\n' "$OUTPUT" | sed -n 's/^Review artifact: //p')"
[ -n "$REVIEW_ARTIFACT" ]
[ -f "$REVIEW_ARTIFACT" ]

grep -F '"target_kind": "session"' "$REVIEW_ARTIFACT" >/dev/null
grep -F '"proposals": [' "$REVIEW_ARTIFACT" >/dev/null

printf '%s\n' "$READ_OUTPUT" | grep -F "Probe mode: review-read" >/dev/null
printf '%s\n' "$READ_OUTPUT" | grep -F "Latest review artifact: $REVIEW_ARTIFACT" >/dev/null
printf '%s\n' "$READ_OUTPUT" | grep -F "Review proposals: " >/dev/null
printf '%s\n' "$READ_OUTPUT" | grep -F "Decision review records: " >/dev/null
printf '%s\n' "$READ_OUTPUT" | grep -F "Latest decision review record: review-summary |" >/dev/null
