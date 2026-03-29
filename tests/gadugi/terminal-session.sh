#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

OBJECTIVE=$'working-directory: .\ncommand: pwd\ncommand: printf "terminal-foundation-ok\\n"'

OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    terminal-run single-process "$OBJECTIVE"
)"

printf '%s\n' "$OUTPUT"

printf '%s\n' "$OUTPUT" | grep -F "Probe mode: terminal-run" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Identity: simard-engineer" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Selected base type: terminal-shell" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Topology: single-process" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Adapter implementation: terminal-shell::local-pty" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Adapter capabilities: prompt-assets, session-lifecycle, memory, evidence, reflection, terminal-session" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Session phase: complete" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Terminal evidence: terminal-command-count=2" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "terminal-foundation-ok" >/dev/null
