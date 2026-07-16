#!/usr/bin/env bash
set -euo pipefail

# Outside-in verification for the Concierge identity (hospitality design +
# operations software). Exercises the operator-probe `concierge-run` surface,
# which designs a hotel and drives the runnable reservations/PMS prototype
# end-to-end, and asserts the identity itself bootstraps.

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

BRIEF="Harbor Light in Lisbon, a 120-room boutique waterfront hotel"

# 1. The Concierge design + runnable prototype completes and self-verifies.
RUN_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    concierge-run single-process "$BRIEF"
)"

printf '%s\n' "$RUN_OUTPUT"

printf '%s\n' "$RUN_OUTPUT" | grep -F "Probe mode: concierge-run" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Hotel: Harbor Light" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Location: Lisbon" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Total rooms: 120" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Concept verified: yes" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -E "Sample reservation: RES-[0-9]+" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "status CheckedOut" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Prototype verified: yes" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Session phase: complete" >/dev/null

# 2. The `simard-concierge` identity is a first-class, bootstrappable identity.
BOOTSTRAP_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    bootstrap-run simard-concierge local-harness single-process \
    "verify concierge identity bootstrap"
)"

printf '%s\n' "$BOOTSTRAP_OUTPUT"

printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Probe mode: bootstrap-run" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Identity: simard-concierge" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Selected base type: local-harness" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Session phase: complete" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Shutdown: stopped" >/dev/null

echo "concierge-identity: PASS"
