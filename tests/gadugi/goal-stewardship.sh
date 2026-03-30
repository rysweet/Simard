#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

STATE_ROOT="$(mktemp -d /tmp/simard-goal-stewardship.XXXXXX)"
trap 'rm -rf "$STATE_ROOT"' EXIT

MEETING_OBJECTIVE="$(cat <<'EOF'
agenda: align the next Simard workstream
decision: preserve meeting-to-engineer continuity
risk: workflow routing is still unreliable
next-step: keep durable priorities visible
open-question: how aggressively should Simard reprioritize?
goal: Preserve meeting handoff | priority=1 | status=active | rationale=meeting decisions must shape later work
goal: Keep outside-in verification strong | priority=2 | status=active | rationale=operator confidence depends on real product exercise
EOF
)"

MEETING_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    meeting-run local-harness single-process \
    "$MEETING_OBJECTIVE" \
    "$STATE_ROOT"
)"

printf '%s\n' "$MEETING_OUTPUT"

printf '%s\n' "$MEETING_OUTPUT" | grep -F "Probe mode: meeting-run" >/dev/null
printf '%s\n' "$MEETING_OUTPUT" | grep -F "Active goals count: 2" >/dev/null
printf '%s\n' "$MEETING_OUTPUT" | grep -F "Active goal 1: p1 [active] Preserve meeting handoff" >/dev/null

ENGINEER_OBJECTIVE=$'inspect the repository state\nrun one safe local engineering action\nverify the outcome explicitly\npersist truthful local evidence and memory'

ENGINEER_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    engineer-loop-run single-process "$ROOT" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
)"

printf '%s\n' "$ENGINEER_OUTPUT"

printf '%s\n' "$ENGINEER_OUTPUT" | grep -F "Probe mode: engineer-loop-run" >/dev/null
printf '%s\n' "$ENGINEER_OUTPUT" | grep -F "Active goals count: 2" >/dev/null
printf '%s\n' "$ENGINEER_OUTPUT" | grep -F "Active goal 1: p1 [active] Preserve meeting handoff" >/dev/null
printf '%s\n' "$ENGINEER_OUTPUT" | grep -F "Active goal 2: p2 [active] Keep outside-in verification strong" >/dev/null
printf '%s\n' "$ENGINEER_OUTPUT" | grep -F "Verification status: verified" >/dev/null
