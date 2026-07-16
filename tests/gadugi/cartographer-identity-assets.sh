#!/usr/bin/env bash
# qa-team scenario — Simard Cartographer data-storytelling identity.
#
# Outside-in verification that the `simard-cartographer` identity is wired
# end-to-end: it bootstraps through the operator probe (inspect -> act ->
# verify -> persist) and its prompt assets + the cartographer-dashboard recipe
# are present and encode the four-stage, serve-and-verify workflow.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# --- 1. The identity bootstraps end-to-end through the operator probe. --------
OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    bootstrap-run simard-cartographer local-harness single-process \
    "verify cartographer identity bootstrap"
)"

printf '%s\n' "$OUTPUT"

printf '%s\n' "$OUTPUT" | grep -F "Probe mode: bootstrap-run" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Identity: simard-cartographer" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Selected base type: local-harness" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Topology: single-process" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Session phase: complete" >/dev/null
printf '%s\n' "$OUTPUT" | grep -F "Shutdown: stopped" >/dev/null

# --- 2. The identity's prompt assets exist. -----------------------------------
for asset in \
  prompt_assets/simard/cartographer_system.md \
  prompt_assets/simard/cartographer_explore.md \
  prompt_assets/simard/cartographer_visualize.md \
  prompt_assets/simard/cartographer_deliver.md \
  prompt_assets/simard/cartographer_narrative.md; do
  test -f "$asset" || { echo "missing prompt asset: $asset" >&2; exit 1; }
done

# The system prompt carries the untrusted-data guard (data is not instructions).
grep -Fi "untrusted" prompt_assets/simard/cartographer_system.md >/dev/null

# --- 3. The recipe orchestrates the four stages and serve-and-verify. ---------
RECIPE=prompt_assets/simard/recipes/cartographer-dashboard.yaml
test -f "$RECIPE" || { echo "missing recipe: $RECIPE" >&2; exit 1; }

for step in \
  'id: "explore"' \
  'id: "design-visualizations"' \
  'id: "deliver-dashboard"' \
  'id: "write-narrative"'; do
  grep -F "$step" "$RECIPE" >/dev/null || { echo "recipe missing step: $step" >&2; exit 1; }
done

for var in dataset_path question output_dir serve_port; do
  grep -F "$var" "$RECIPE" >/dev/null || { echo "recipe missing var: $var" >&2; exit 1; }
done

# Delivery must fetch the served URL to verify it renders (not just "started").
grep -F "http://127.0.0.1:" "$RECIPE" >/dev/null

echo "PASS: cartographer identity, prompts, and recipe are wired end-to-end"
