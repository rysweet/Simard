#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

OUTPUT="$(
  SIMARD_PROMPT_ROOT="$ROOT/prompt_assets" \
  SIMARD_OBJECTIVE='verify rusty-clawd bootstrap' \
  SIMARD_IDENTITY='simard-engineer' \
  SIMARD_BASE_TYPE='rusty-clawd' \
  SIMARD_RUNTIME_TOPOLOGY='single-process' \
  cargo run --quiet
)"

printf '%s\n' "$OUTPUT"

printf '%s\n' "$OUTPUT" | grep -F "Bootstrap mode: explicit-config" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Bootstrap selection: identity=simard-engineer, base_type=rusty-clawd, topology=single-process" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Adapter implementation: rusty-clawd::session-backend" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Shutdown: stopped" >/dev/null
