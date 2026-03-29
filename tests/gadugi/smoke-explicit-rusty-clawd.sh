#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

export SIMARD_PROMPT_ROOT="$ROOT/prompt_assets"
export SIMARD_OBJECTIVE="verify explicit rusty-clawd runtime path"
export SIMARD_IDENTITY="simard-engineer"
export SIMARD_BASE_TYPE="rusty-clawd"
export SIMARD_RUNTIME_TOPOLOGY="single-process"
unset SIMARD_BOOTSTRAP_MODE

OUTPUT="$(cargo run --quiet)"

printf '%s\n' "$OUTPUT"

printf '%s\n' "$OUTPUT" | grep -F "Simard local runtime executed successfully." >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Bootstrap mode: explicit-config" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Config sources: prompt_root=env:SIMARD_PROMPT_ROOT, objective=env:SIMARD_OBJECTIVE, base_type=env:SIMARD_BASE_TYPE, topology=env:SIMARD_RUNTIME_TOPOLOGY" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Bootstrap selection: identity=simard-engineer, base_type=rusty-clawd, topology=single-process" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Snapshot: state=ready, topology=single-process, base_type=rusty-clawd" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Adapter implementation: rusty-clawd::session-backend" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Shutdown: stopped" >/dev/null
