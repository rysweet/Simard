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
printf '%s\n' "$OUTPUT" | grep -F "Terminal command count: 2" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Terminal wait count: 1" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Terminal steps count: 3" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Terminal step 2: wait-for: terminal-foundation-ready" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Terminal checkpoint 1: terminal-foundation-ready" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Terminal last output line: terminal-foundation-ok" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "terminal-foundation-ok" >/dev/null

OBJECTIVE_FILE="$(mktemp /tmp/simard-terminal-objective.XXXXXX)"
cat > "$OBJECTIVE_FILE" <<'EOF'
working-directory: .
command: printf "terminal-file-ready\n"
wait-for: terminal-file-ready
input: printf "terminal-file-ok\n"
EOF

FILE_OUTPUT="$(
  cargo run --quiet --bin simard -- \
    engineer terminal-file single-process "$OBJECTIVE_FILE"
)"

printf '%s\n' "$FILE_OUTPUT"

printf '%s\n' "$FILE_OUTPUT" | grep -F "Terminal steps count: 3" >/dev/null
printf '%s\n' "$FILE_OUTPUT" | grep -F "Terminal checkpoint 1: terminal-file-ready" >/dev/null
printf '%s\n' "$FILE_OUTPUT" | grep -F "Terminal last output line: terminal-file-ok" >/dev/null

RECIPE_LIST_OUTPUT="$(
  cargo run --quiet --bin simard -- \
    engineer terminal-recipe-list
)"

printf '%s\n' "$RECIPE_LIST_OUTPUT"
printf '%s\n' "$RECIPE_LIST_OUTPUT" | grep -F "foundation-check" >/dev/null
printf '%s\n' "$RECIPE_LIST_OUTPUT" | grep -F "copilot-status-check" >/dev/null

RECIPE_RUN_OUTPUT="$(
  cargo run --quiet --bin simard -- \
    engineer terminal-recipe single-process foundation-check
)"

printf '%s\n' "$RECIPE_RUN_OUTPUT"
printf '%s\n' "$RECIPE_RUN_OUTPUT" | grep -F "Terminal checkpoint 1: terminal-recipe-ready" >/dev/null
printf '%s\n' "$RECIPE_RUN_OUTPUT" | grep -F "Terminal last output line: terminal-recipe-ok" >/dev/null

READ_OUTPUT="$(
  cargo run --quiet --bin simard -- \
    engineer terminal-read single-process
)"

printf '%s\n' "$READ_OUTPUT"

printf '%s\n' "$READ_OUTPUT" | grep -F "Probe mode: terminal-read" >/dev/null
printf '%s\n' "$READ_OUTPUT" | grep -F "Selected base type: terminal-shell" >/dev/null
printf '%s\n' "$READ_OUTPUT" | grep -F "Terminal command count: 2" >/dev/null
printf '%s\n' "$READ_OUTPUT" | grep -F "Terminal wait count: 1" >/dev/null
printf '%s\n' "$READ_OUTPUT" | grep -F "Terminal steps count: 3" >/dev/null
printf '%s\n' "$READ_OUTPUT" | grep -F "Terminal checkpoint 1: terminal-recipe-ready" >/dev/null
printf '%s\n' "$READ_OUTPUT" | grep -F "Terminal last output line: terminal-recipe-ok" >/dev/null
printf '%s\n' "$READ_OUTPUT" | grep -F "terminal-recipe-ok" >/dev/null

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
rm -f "$OBJECTIVE_FILE"
