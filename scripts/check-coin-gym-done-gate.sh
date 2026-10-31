#!/usr/bin/env bash
# check-coin-gym-done-gate.sh — Machine-checkable done-gate for the goal
# "build a local coin benchmark harness and a self-improvement loop"
# (slug build-a-local-coin-benchmark-harness-and-a-self-09e65e35).
#
# WHY THIS EXISTS
# ----------------
# The goal stayed Blocked cycle after cycle with the same diagnosis — "no
# tracked PR/issue the done-gate can verify" (why=UNCLEAR-CRITERIA) — even though
# the LOCAL COIN Gym harness AND its Phase-5 self-improvement loop were already
# built and green. The blocker was that its finish condition had no definition a
# check could confirm. This script turns that finish condition into a single
# command the done-gate can run: it is the machine-checkable artifact the goal's
# done-criteria points at (see Specs/coin-gym-benchmark-harness.md).
#
# The goal is DONE (exit 0) only when the built-in `coin-gym verify` acceptance
# self-check passes every LOCAL criterion:
#   CG-1 target-loader      CG-2 baseline-runner   CG-3 team-runner
#   CG-4 scorer             CG-5 leaderboard-comparator
#   CG-6 self-improvement-loop                     CG-7 contract-wiring
#
# Live VM grading (`coin evaluate`/`coin verify` on a provisioned Docker host) is
# Phase 3 (issue #2823) and is intentionally OUT of this LOCAL done-gate.
#
# Usage:
#   scripts/check-coin-gym-done-gate.sh          # done-gate: build + coin-gym verify
#   scripts/check-coin-gym-done-gate.sh --full   # + the two outside-in gadugi scenarios
#
# Exit codes:
#   0  harness + self-improvement loop delivered — the goal can be certified complete
#   1  not yet done — failing acceptance criteria are printed above

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SPEC="$REPO_ROOT/Specs/coin-gym-benchmark-harness.md"
MODE="${1:-}"

cd "$REPO_ROOT"

if [[ ! -f "$SPEC" ]]; then
  echo "❌ done-gate spec not found: $SPEC"
  echo "   The done-gate cannot certify the goal without its criteria spec."
  exit 1
fi

# ── 1. Build the harness binary ───────────────────────────────────────────────
echo "Building the coin-gym harness..."
if ! cargo build --quiet --bin coin-gym --manifest-path "$REPO_ROOT/Cargo.toml"; then
  echo "❌ coin-gym failed to build — cannot run the done-gate."
  exit 1
fi

# ── 2. Run the built-in acceptance self-check (the measurable done-gate) ───────
echo ""
echo "Running the LOCAL COIN Gym acceptance self-check (coin-gym verify)..."
echo ""
if cargo run --quiet --bin coin-gym --manifest-path "$REPO_ROOT/Cargo.toml" -- verify; then
  VERIFY_OK=1
else
  VERIFY_OK=0
fi

# ── 3. Optional: full outside-in confirmation via the gadugi scenarios ─────────
GADUGI_OK=1
if [[ "$MODE" == "--full" ]]; then
  echo ""
  echo "Running the outside-in gadugi scenarios (--full)..."
  for scenario in \
    "$REPO_ROOT/tests/gadugi/coin-gym-harness.sh" \
    "$REPO_ROOT/tests/gadugi/coin-gym-self-improve.sh"; do
    if [[ -x "$scenario" ]]; then
      echo "  → $(basename "$scenario")"
      bash "$scenario" || GADUGI_OK=0
    else
      echo "  ⚠️  missing scenario: $scenario"
      GADUGI_OK=0
    fi
  done
fi

# ── Verdict ───────────────────────────────────────────────────────────────────
echo ""
if [[ "$VERIFY_OK" == "1" && "$GADUGI_OK" == "1" ]]; then
  echo "✅ DONE — the LOCAL COIN Gym harness and its self-improvement loop pass"
  echo "   every acceptance criterion. The goal"
  echo "   build-a-local-coin-benchmark-harness-and-a-self-09e65e35 can be"
  echo "   certified complete."
  exit 0
fi

echo "⏳ Not done yet:"
[[ "$VERIFY_OK" == "1" ]] && echo "   • coin-gym verify: all LOCAL criteria PASS" \
                          || echo "   • coin-gym verify: acceptance criteria FAILING (see above)"
if [[ "$MODE" == "--full" ]]; then
  [[ "$GADUGI_OK" == "1" ]] && echo "   • gadugi scenarios: green" \
                            || echo "   • gadugi scenarios: failing (see above)"
fi
exit 1
