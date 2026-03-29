#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    bootstrap-run simard-composite-engineer local-harness single-process \
    "verify composite identity bootstrap"
)"

printf '%s\n' "$OUTPUT"

printf '%s\n' "$OUTPUT" | grep -F "Probe mode: bootstrap-run" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Identity: simard-composite-engineer" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Identity components: simard-engineer, simard-meeting, simard-gym" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Selected base type: local-harness" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Topology: single-process" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Adapter implementation: local-harness" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Session phase: complete" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Shutdown: stopped" >/dev/null
