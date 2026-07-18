#!/usr/bin/env bash
# check-coin-benchmark-harness-done-gate.sh — Machine-checkable done-gate for the
# goal "build a local COIN benchmark harness and a self-improvement loop"
# (slug build-a-local-coin-benchmark-harness-and-a-self-09e65e35).
#
# WHY THIS EXISTS
# ----------------
# The goal stayed parked as blocked cycle after cycle with the same diagnosis —
# "no tracked PR/issue the done-gate can verify" (why=UNCLEAR-CRITERIA). The
# blocker was NOT technical: the harness already shipped on `main` under
# src/coin_gym/, and its measurable acceptance self-check landed in merged
# PR #4171 ("feat(coin-gym): `verify` acceptance self-check — a measurable
# done-gate"). The problem was that the goal's finish condition had no artifact a
# check could confirm, so every OODA cycle re-observed it as unfinished and
# emitted NO ACTION — even while its own `verify` command could already certify
# the harness locally.
#
# This script turns the goal's finish condition into a single command the
# done-gate can run. It confirms the three seams that make up the delivered goal
# still exist on `main` and re-asserts the harness's own test suite:
#   1. the local COIN benchmark harness CLI (run / score / compare / verify),
#   2. the `verify` acceptance self-check delivered by merged PR #4171, and
#   3. the self-improvement loop (run_self_improvement).
# It is the machine-checkable artifact the goal's done-criteria points at
# (see Specs/coin-benchmark-harness-done-gate.md).
#
# Because the work is already delivered, this gate certifies the goal as
# COMPLETE. It exits 0 while the delivered behaviour is present and green, and
# turns red only if that behaviour is ever removed or regresses.
#
# Usage:
#   scripts/check-coin-benchmark-harness-done-gate.sh          # run the harness tests
#   scripts/check-coin-benchmark-harness-done-gate.sh --full   # + confirm the Phase-1 research doc
#
# Exit codes:
#   0  the harness + self-check + self-improvement seams hold — the goal is certified complete
#   1  not done — a failing check is printed above

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SPEC="$REPO_ROOT/Specs/coin-benchmark-harness-done-gate.md"
CLI_SRC="$REPO_ROOT/src/coin_gym/mod.rs"
IMPROVE_SRC="$REPO_ROOT/src/coin_gym/improve_loop.rs"
PHASE1_DOC="$REPO_ROOT/docs/research/coin-benchmark-phase1.md"
MODE="${1:-}"

cd "$REPO_ROOT"

if [[ ! -f "$SPEC" ]]; then
  echo "❌ done-gate spec not found: $SPEC"
  echo "   The done-gate cannot certify the goal without its criteria spec."
  exit 1
fi

# ── 1. The local COIN benchmark harness CLI must still exist ───────────────────
if [[ ! -f "$CLI_SRC" ]] || ! grep -q "fn dispatch_with_home" "$CLI_SRC"; then
  echo "❌ harness CLI dispatch missing: dispatch_with_home in $CLI_SRC"
  echo "   The local COIN benchmark harness command entry point is gone."
  exit 1
fi

# ── 2. The `verify` acceptance self-check (merged PR #4171) must still exist ────
# This is the measurable done-gate the harness runs against a built-in offline
# snapshot: `coin-gym verify` exits 0 only when every LOCAL acceptance criterion
# passes. run_acceptance_checks is the seam that powers it.
if ! grep -q "fn run_acceptance_checks" "$CLI_SRC" || ! grep -q '"verify" => cmd_verify' "$CLI_SRC"; then
  echo "❌ acceptance self-check missing: run_acceptance_checks / verify in $CLI_SRC"
  echo "   The measurable done-gate delivered by merged PR #4171 is gone."
  exit 1
fi

# ── 3. The self-improvement loop must still exist ──────────────────────────────
if [[ ! -f "$IMPROVE_SRC" ]] || ! grep -q "fn run_self_improvement" "$IMPROVE_SRC"; then
  echo "❌ self-improvement loop missing: run_self_improvement in $IMPROVE_SRC"
  echo "   The loop that proposes, verifies, and rolls back tactics on a held-out"
  echo "   slice (the '...and a self-improvement' half of the goal) is gone."
  exit 1
fi

# ── 4. Re-assert the harness's own test suite (the measurable gate) ────────────
# These are the tests shipped with the harness and its self-improvement loop.
# They fail if the harness stops scoring against the leaderboard shape, if the
# acceptance self-check regresses, or if the self-improvement loop accepts an
# overfit tactic.
echo "Running the COIN benchmark harness tests..."
TESTS_OK=1
if ! cargo test --quiet --lib --manifest-path "$REPO_ROOT/Cargo.toml" -- coin_gym::; then
  TESTS_OK=0
fi

# ── 5. Optional: confirm the Phase-1 research record is present ────────────────
DOC_OK=1
if [[ "$MODE" == "--full" ]]; then
  echo ""
  echo "Confirming the Phase-1 completion record (--full)..."
  if [[ -f "$PHASE1_DOC" ]]; then
    echo "  → present: $(basename "$PHASE1_DOC")"
  else
    echo "  ⚠️  missing Phase-1 record: $PHASE1_DOC"
    DOC_OK=0
  fi
fi

# ── Verdict ───────────────────────────────────────────────────────────────────
echo ""
if [[ "$TESTS_OK" == "1" && "$DOC_OK" == "1" ]]; then
  echo "✅ DONE — the local COIN benchmark harness runs, scores against the"
  echo "   published-leaderboard shape, self-checks its own acceptance criteria"
  echo "   (coin-gym verify), and drives a self-improvement loop that keeps only"
  echo "   tactics that generalise on a held-out slice. The goal"
  echo "   build-a-local-coin-benchmark-harness-and-a-self-09e65e35 is certified complete."
  exit 0
fi

echo "⏳ Not done yet:"
[[ "$TESTS_OK" == "1" ]] && echo "   • harness test suite: PASS" \
                         || echo "   • harness test suite: FAILING (see above)"
if [[ "$MODE" == "--full" ]]; then
  [[ "$DOC_OK" == "1" ]] && echo "   • Phase-1 research record: present" \
                         || echo "   • Phase-1 research record: missing (see above)"
fi
exit 1
