#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    review-run local-harness single-process \
    "inspect the current operator flow and preserve reviewable artifacts"
)"

printf '%s\n' "$OUTPUT"

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
