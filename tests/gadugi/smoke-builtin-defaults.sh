#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

OUTPUT="$(
  SIMARD_BOOTSTRAP_MODE='builtin-defaults' \
  cargo run --quiet
)"

printf '%s\n' "$OUTPUT"

printf '%s\n' "$OUTPUT" | grep -F "Bootstrap mode: builtin-defaults" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Bootstrap selection: identity=simard-engineer, base_type=local-harness, topology=single-process" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Adapter implementation: local-harness" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Shutdown: stopped" >/dev/null
