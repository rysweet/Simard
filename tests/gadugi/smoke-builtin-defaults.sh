#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

unset SIMARD_PROMPT_ROOT SIMARD_OBJECTIVE SIMARD_IDENTITY SIMARD_BASE_TYPE SIMARD_RUNTIME_TOPOLOGY
export SIMARD_BOOTSTRAP_MODE=builtin-defaults

OUTPUT="$(cargo run --quiet)"

printf '%s\n' "$OUTPUT"

printf '%s\n' "$OUTPUT" | grep -F "Simard local runtime executed successfully." >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Bootstrap mode: builtin-defaults" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Config sources: prompt_root=opt-in:SIMARD_BOOTSTRAP_MODE, objective=opt-in:SIMARD_BOOTSTRAP_MODE, base_type=opt-in:SIMARD_BOOTSTRAP_MODE, topology=opt-in:SIMARD_BOOTSTRAP_MODE" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Bootstrap selection: identity=simard-engineer, base_type=local-harness, topology=single-process" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Snapshot: state=ready, topology=single-process, base_type=local-harness" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Adapter implementation: local-harness" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Shutdown: stopped" >/dev/null
