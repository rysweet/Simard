#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

STATE_ROOT="$(mktemp -d /tmp/simard-improvement-curation.XXXXXX)"
trap 'rm -rf "$STATE_ROOT"' EXIT

REVIEW_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    review-run local-harness single-process \
    "inspect the current Simard review surface and preserve concrete proposals" \
    "$STATE_ROOT"
)"

printf '%s\n' "$REVIEW_OUTPUT"

printf '%s\n' "$REVIEW_OUTPUT" | grep -F "Probe mode: review-run" >/dev/null
printf '%s\n' "$REVIEW_OUTPUT" | grep -F "Review proposals: 2" >/dev/null

IMPROVEMENT_OBJECTIVE="$(cat <<'EOF'
approve: Capture denser execution evidence | priority=1 | status=active | rationale=operators need denser execution evidence now
approve: Promote this pattern into a repeatable benchmark | priority=2 | status=proposed | rationale=carry this into the next benchmark planning pass
EOF
)"

IMPROVEMENT_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    improvement-curation-run local-harness single-process \
    "$IMPROVEMENT_OBJECTIVE" \
    "$STATE_ROOT"
)"

printf '%s\n' "$IMPROVEMENT_OUTPUT"

printf '%s\n' "$IMPROVEMENT_OUTPUT" | grep -F "Probe mode: improvement-curation-run" >/dev/null
printf '%s\n' "$IMPROVEMENT_OUTPUT" | grep -F "Identity: simard-improvement-curator" >/dev/null
printf '%s\n' "$IMPROVEMENT_OUTPUT" | grep -F "Approved proposals: 2" >/dev/null
printf '%s\n' "$IMPROVEMENT_OUTPUT" | grep -F "Active goals count: 1" >/dev/null
printf '%s\n' "$IMPROVEMENT_OUTPUT" | grep -F "Active goal 1: p1 [active] Capture denser execution evidence" >/dev/null
printf '%s\n' "$IMPROVEMENT_OUTPUT" | grep -F "Proposed goals count: 1" >/dev/null
printf '%s\n' "$IMPROVEMENT_OUTPUT" | grep -F "Proposed goal 1: p2 [proposed] Promote this pattern into a repeatable benchmark" >/dev/null

ENGINEER_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    engineer-loop-run single-process "$ROOT" \
    "inspect the repo and preserve explicit improvement context" \
    "$STATE_ROOT"
)"

printf '%s\n' "$ENGINEER_OUTPUT"

printf '%s\n' "$ENGINEER_OUTPUT" | grep -F "Probe mode: engineer-loop-run" >/dev/null
printf '%s\n' "$ENGINEER_OUTPUT" | grep -F "Active goals count: 1" >/dev/null
printf '%s\n' "$ENGINEER_OUTPUT" | grep -F "Active goal 1: p1 [active] Capture denser execution evidence" >/dev/null
printf '%s\n' "$ENGINEER_OUTPUT" | grep -F "Verification status: verified" >/dev/null
