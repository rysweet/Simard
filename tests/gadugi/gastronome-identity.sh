#!/usr/bin/env bash
set -euo pipefail

# Outside-in verification for the Gastronome identity (culinary/menu/event design
# + kitchen operations app). Exercises the operator-probe `gastronome-run`
# surface, which designs a menu and takes the brief to a costed, scaled, and
# scheduled plan end-to-end, and asserts the identity itself bootstraps.

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

BRIEF="Harvest Feast menu for a wedding of 120 guests, elegant plated"

# 1. The Gastronome design + runnable plan completes and self-verifies.
RUN_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    gastronome-run single-process "$BRIEF"
)"

printf '%s\n' "$RUN_OUTPUT"

printf '%s\n' "$RUN_OUTPUT" | grep -F "Probe mode: gastronome-run" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Menu: Harvest Feast" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Occasion: wedding" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Guests: 120" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -E "Sample scaled dish: C[0-9]+" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -E "Prep schedule: [0-9]+ minutes" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Plan verified: yes" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Session phase: complete" >/dev/null

# 2. The `simard-gastronome` identity is a first-class, bootstrappable identity.
BOOTSTRAP_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    bootstrap-run simard-gastronome local-harness single-process \
    "verify gastronome identity bootstrap"
)"

printf '%s\n' "$BOOTSTRAP_OUTPUT"

printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Probe mode: bootstrap-run" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Identity: simard-gastronome" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Selected base type: local-harness" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Session phase: complete" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Shutdown: stopped" >/dev/null

echo "gastronome-identity: PASS"
