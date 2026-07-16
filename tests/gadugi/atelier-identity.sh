#!/usr/bin/env bash
set -euo pipefail

# Outside-in verification for the Atelier identity (furniture / industrial
# product design + fabrication). Exercises the operator-probe `atelier-run`
# surface, which designs a product and drives the runnable fabrication package
# (cut list, BOM, and fabrication-ready exports + render) end-to-end, and
# asserts the identity itself bootstraps.

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

BRIEF="Larch dining table in solid oak, 1800x900x740mm"

# 1. The Atelier design + runnable fabrication completes and self-verifies.
RUN_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    atelier-run single-process "$BRIEF"
)"

printf '%s\n' "$RUN_OUTPUT"

printf '%s\n' "$RUN_OUTPUT" | grep -F "Probe mode: atelier-run" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Category: table" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Material: solid oak" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Dimensions (mm): 1800 x 900 x 740" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Total parts per unit: 9" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -E "OpenSCAD model -> .+\.scad" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -E "STEP \(ISO-10303-21\) -> .+\.step" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -E "Render: .+-elevation\.svg" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Prototype verified: yes" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Session phase: complete" >/dev/null

# 2. The `simard-atelier` identity is a first-class, bootstrappable identity.
BOOTSTRAP_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    bootstrap-run simard-atelier local-harness single-process \
    "verify atelier identity bootstrap"
)"

printf '%s\n' "$BOOTSTRAP_OUTPUT"

printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Probe mode: bootstrap-run" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Identity: simard-atelier" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Selected base type: local-harness" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Session phase: complete" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Shutdown: stopped" >/dev/null

echo "atelier-identity: PASS"
