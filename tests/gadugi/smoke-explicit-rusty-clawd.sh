#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    bootstrap-run simard-engineer rusty-clawd multi-process \
    "verify rusty clawd operator bootstrap"
)"

printf '%s\n' "$OUTPUT"

printf '%s\n' "$OUTPUT" | grep -F "Probe mode: bootstrap-run" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Identity: simard-engineer" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Selected base type: rusty-clawd" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Topology: multi-process" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Adapter implementation: rusty-clawd::session-backend" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Topology backend: topology::loopback-mesh" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Transport backend: transport::loopback-mailbox" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Session phase: complete" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Shutdown: stopped" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Execution summary: RustyClawd session backend executed objective-metadata(" >/dev/null
