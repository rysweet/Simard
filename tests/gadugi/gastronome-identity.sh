#!/usr/bin/env bash
# qa-team scenario: Gastronome culinary/menu/event-design identity + prep app.
#
# Outside-in gate proving the "done when" surface: the pluggable Gastronome
# identity bootstraps through the operator probe (inspect -> act -> verify ->
# persist), and the runnable prep app turns an event/menu brief into a costed,
# nutritionally analysed, prep-scheduled menu plan end-to-end.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# ---------------------------------------------------------------------------
# 1. Identity bootstrap through the operator probe (repo-grounded surface).
# ---------------------------------------------------------------------------
BOOTSTRAP_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    bootstrap-run simard-gastronome local-harness single-process \
    "design a costed, scheduled menu plan"
)"

printf '%s\n' "$BOOTSTRAP_OUTPUT"

printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Probe mode: bootstrap-run" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Identity: simard-gastronome" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Selected base type: local-harness" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Topology: single-process" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Session phase: complete" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Shutdown: stopped" >/dev/null

# ---------------------------------------------------------------------------
# 2. Prep app demo — builtin recipe book, full end-to-end plan.
# ---------------------------------------------------------------------------
DEMO_OUTPUT="$(cargo run --quiet --bin simard-gastronome -- demo)"

printf '%s\n' "$DEMO_OUTPUT"

printf '%s\n' "$DEMO_OUTPUT" | grep -F "MENU PLAN" >/dev/null
printf '%s\n' "$DEMO_OUTPUT" | grep -F "Cost" >/dev/null
printf '%s\n' "$DEMO_OUTPUT" | grep -F "TOTAL" >/dev/null
printf '%s\n' "$DEMO_OUTPUT" | grep -F "PER GUEST" >/dev/null
printf '%s\n' "$DEMO_OUTPUT" | grep -F "Nutrition (per guest)" >/dev/null
printf '%s\n' "$DEMO_OUTPUT" | grep -F "Shopping list" >/dev/null
printf '%s\n' "$DEMO_OUTPUT" | grep -F "Prep schedule" >/dev/null
printf '%s\n' "$DEMO_OUTPUT" | grep -F "kitchen call" >/dev/null

# ---------------------------------------------------------------------------
# 3. Prep app plan — external event brief + pluggable recipe book (the
#    "event/menu brief -> costed, scheduled menu plan" acceptance criterion).
# ---------------------------------------------------------------------------
PLAN_OUTPUT="$(
  cargo run --quiet --bin simard-gastronome -- plan \
    --brief examples/gastronome/event_brief.json \
    --recipes examples/gastronome/recipes.json
)"

printf '%s\n' "$PLAN_OUTPUT"

printf '%s\n' "$PLAN_OUTPUT" | grep -F "MENU PLAN" >/dev/null
printf '%s\n' "$PLAN_OUTPUT" | grep -F "Shopping list" >/dev/null
printf '%s\n' "$PLAN_OUTPUT" | grep -F "Prep schedule" >/dev/null

# ---------------------------------------------------------------------------
# 4. Prep app plan --json — machine-readable plan for downstream tooling.
# ---------------------------------------------------------------------------
JSON_OUTPUT="$(
  cargo run --quiet --bin simard-gastronome -- plan \
    --brief examples/gastronome/event_brief.json \
    --recipes examples/gastronome/recipes.json \
    --json
)"

printf '%s\n' "$JSON_OUTPUT" | grep -F "\"event\"" >/dev/null
printf '%s\n' "$JSON_OUTPUT" | grep -F "\"menu\"" >/dev/null
printf '%s\n' "$JSON_OUTPUT" | grep -F "\"shopping_list\"" >/dev/null
printf '%s\n' "$JSON_OUTPUT" | grep -F "\"schedule\"" >/dev/null

echo "gastronome-identity: PASS"
