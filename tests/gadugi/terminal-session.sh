#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

OBJECTIVE=$'working-directory: .\ncommand: printf "terminal-foundation-ready\\n"\nwait-for: terminal-foundation-ready\ncommand: printf "terminal-foundation-ok\\n"'

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
printf '%s\n' "$OUTPUT" | grep -F "Terminal evidence: terminal-wait-count=1" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "terminal-foundation-ok" >/dev/null

MARKER="$(mktemp /tmp/simard-terminal-injection.XXXXXX)"
rm -f "$MARKER"
BAD_OBJECTIVE="$(printf 'shell: /usr/bin/bash$(printf pwned>%s)\ncommand: pwd\n' "$MARKER")"

set +e
BAD_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    terminal-run single-process "$BAD_OBJECTIVE" 2>&1
)"
BAD_STATUS=$?
set -e

[ "$BAD_STATUS" -ne 0 ]
[ ! -e "$MARKER" ]
printf '%s\n' "$BAD_OUTPUT" | grep -F "terminal-shell only accepts an absolute shell executable path using safe path characters" >/dev/null
