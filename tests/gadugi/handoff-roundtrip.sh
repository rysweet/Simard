#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    handoff-roundtrip simard-composite-engineer rusty-clawd multi-process \
    "verify composite runtime handoff roundtrip"
)"

printf '%s\n' "$OUTPUT"

STATE_ROOT="$(printf '%s\n' "$OUTPUT" | sed -n 's/^State root: //p')"
[ -n "$STATE_ROOT" ]
[ -d "$STATE_ROOT" ]
[ -f "$STATE_ROOT/memory_records.json" ]
[ -f "$STATE_ROOT/evidence_records.json" ]
[ -f "$STATE_ROOT/latest_handoff.json" ]

printf '%s\n' "$OUTPUT" | grep -F "Probe mode: handoff-roundtrip" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Identity: simard-composite-engineer" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Identity components: simard-engineer, simard-meeting, simard-gym" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Selected base type: rusty-clawd" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Topology: multi-process" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Runtime node: node-loopback-mesh" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Mailbox address: loopback://node-loopback-mesh" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Exported memory records: 2" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Exported evidence records: 5" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Restored state: initializing" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Restored session phase: complete" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Restored adapter implementation: rusty-clawd::session-backend" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Restored topology backend: topology::loopback-mesh" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Restored transport backend: transport::loopback-mailbox" >/dev/null
