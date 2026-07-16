#!/usr/bin/env bash
# bursar-identity.sh — Outside-in gate for the simard-bursar identity.
#
# Verifies that the research/advisory-only Bursar identity is advertised by the
# builtin loader and bootstraps end-to-end through the operator probe's
# repo-grounded engineer-loop surface (inspect -> act -> verify -> persist).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    bootstrap-run simard-bursar local-harness single-process \
    "verify bursar identity bootstrap"
)"

printf '%s\n' "$OUTPUT"

printf '%s\n' "$OUTPUT" | grep -F "Probe mode: bootstrap-run" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Identity: simard-bursar" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Selected base type: local-harness" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Topology: single-process" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Adapter implementation: local-harness" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Session phase: complete" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Shutdown: stopped" >/dev/null

# The Bursar identity must also carry the terminal-shell base type so it can run
# pandas/backtrader/QuantLib analytics, exactly like the engineer identity. The
# terminal-shell base type executes the objective as a shell command, so pass a
# harmless real command rather than a prose objective.
TERMINAL_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    bootstrap-run simard-bursar terminal-shell single-process \
    "echo bursar-analytics-path-ok" 2>&1
)"

printf '%s\n' "$TERMINAL_OUTPUT"

printf '%s\n' "$TERMINAL_OUTPUT" | grep -F "Identity: simard-bursar" >/dev/null
printf '%s\n' "$TERMINAL_OUTPUT" | grep -F "Selected base type: terminal-shell" >/dev/null
printf '%s\n' "$TERMINAL_OUTPUT" | grep -F "Session phase: complete" >/dev/null

echo "PASS: simard-bursar identity bootstraps on local-harness and terminal-shell"
