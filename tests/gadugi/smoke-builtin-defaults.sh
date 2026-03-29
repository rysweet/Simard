#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    bootstrap-run simard-engineer copilot-sdk single-process \
    "verify copilot sdk alias bootstrap"
)"

printf '%s\n' "$OUTPUT"

printf '%s\n' "$OUTPUT" | grep -F "Probe mode: bootstrap-run" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Identity: simard-engineer" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Selected base type: copilot-sdk" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Topology: single-process" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Adapter implementation: local-harness" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Topology backend: topology::in-process" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Transport backend: transport::in-memory-mailbox" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Session phase: complete" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Shutdown: stopped" >/dev/null
