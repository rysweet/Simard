#!/usr/bin/env bash
set -euo pipefail

# Outside-in verification for the Bursar identity (investment-portfolio research
# & management). Exercises the operator-probe `bursar-run` surface, which
# constructs a target allocation and drives the runnable backtest / risk /
# rebalancing analysis end-to-end (research/advisory only — no order execution),
# and asserts the identity itself bootstraps.

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

BRIEF="Balanced growth portfolio for a 20 year horizon, \$250,000"

# 1. The Bursar allocation + backtest/risk analysis completes and self-verifies.
RUN_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    bursar-run single-process "$BRIEF"
)"

printf '%s\n' "$RUN_OUTPUT"

printf '%s\n' "$RUN_OUTPUT" | grep -F "Probe mode: bursar-run" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Target allocation:" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -E "Annualized return: -?[0-9]+\.[0-9]+%" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -E "Max drawdown: [0-9]+\.[0-9]+%" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -E "Sharpe ratio: -?[0-9]+\.[0-9]+" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Order execution: none (advisory only)" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Allocation verified: yes" >/dev/null
printf '%s\n' "$RUN_OUTPUT" | grep -F "Session phase: complete" >/dev/null

# The Bursar must NEVER report executing an order.
if printf '%s\n' "$RUN_OUTPUT" | grep -F "Order execution: PERFORMED" >/dev/null; then
  echo "bursar-identity: FAIL (order execution reported)"
  exit 1
fi

# 2. The `simard-bursar` identity is a first-class, bootstrappable identity.
BOOTSTRAP_OUTPUT="$(
  cargo run --quiet --bin simard_operator_probe -- \
    bootstrap-run simard-bursar local-harness single-process \
    "verify bursar identity bootstrap"
)"

printf '%s\n' "$BOOTSTRAP_OUTPUT"

printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Probe mode: bootstrap-run" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Identity: simard-bursar" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Selected base type: local-harness" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Session phase: complete" >/dev/null
printf '%s\n' "$BOOTSTRAP_OUTPUT" | grep -F "Shutdown: stopped" >/dev/null

echo "bursar-identity: PASS"
