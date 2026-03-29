#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

OBJECTIVE="$(cat <<'EOF'
agenda: align the next Simard product block
update: durable memory foundation merged in PR 29
decision: prioritize facilitator behavior before remote orchestration
risk: workflow automation is still unreliable in clean worktrees
next-step: ship operator-visible meeting validation
open-question: how should meeting decisions influence engineer planning?
EOF
)"

OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    meeting-run local-harness single-process \
    "$OBJECTIVE"
)"

printf '%s\n' "$OUTPUT"

printf '%s\n' "$OUTPUT" | grep -F "Probe mode: meeting-run" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Identity: simard-meeting" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Selected base type: local-harness" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Topology: single-process" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Decision records: 1" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "prioritize facilitator behavior before remote orchestration" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "workflow automation is still unreliable in clean worktrees" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "ship operator-visible meeting validation" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "captured 1 decisions, 1 risks, 1 next steps, and 1 open questions" >/dev/null
