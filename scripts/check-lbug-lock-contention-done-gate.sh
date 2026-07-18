#!/usr/bin/env bash
# check-lbug-lock-contention-done-gate.sh — Machine-checkable done-gate for the goal
# "stop lbug lock contention from being mistaken for catalog corruption"
# (slug stop-lbug-lock-contention-from-being-mistaken-f-0ebf1bc7).
#
# WHY THIS EXISTS
# ----------------
# The goal stayed Blocked cycle after cycle with the same diagnosis — "no
# tracked PR/issue the done-gate can verify" (why=UNCLEAR-CRITERIA). The blocker
# was NOT technical: the fix already shipped in merged PR #4317
# ("serialize opens so lock-contention never wipes memory"). lbug
# (amplihack-memory-lib) used to mis-classify a transient cross-process file-lock
# conflict as catalog corruption and rebuild the store EMPTY — wiping memory. PR
# #4317 closed that door at Simard's own open seam (a cross-process advisory lock
# with fail-loud semantics) and shipped regression tests that prove it.
#
# The blocker was that the goal's finish condition had no definition a check
# could confirm. This script turns that finish condition into a single command
# the done-gate can run: it re-asserts the exact regression tests that prove
# lbug lock contention can no longer be mistaken for corruption and wipe memory.
# It is the machine-checkable artifact the goal's done-criteria points at
# (see Specs/lbug-lock-contention-done-gate.md).
#
# This is a STANDING (regression-protection) goal: the done-gate stays green as
# long as a contended open FAILS LOUD instead of wiping records. The moment the
# guard regresses, these tests fail and the gate reports the goal as not-done.
#
# Usage:
#   scripts/check-lbug-lock-contention-done-gate.sh          # run the regression tests
#   scripts/check-lbug-lock-contention-done-gate.sh --full   # + confirm the qa scenario asset
#
# Exit codes:
#   0  the open-serialization guard + its regression tests hold — the goal is certified
#   1  not done — a failing check is printed above

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SPEC="$REPO_ROOT/Specs/lbug-lock-contention-done-gate.md"
GUARD_SRC="$REPO_ROOT/src/cognitive_memory/open_guard.rs"
QA_SCENARIO="$REPO_ROOT/tests/qa-scenarios/cognitive-memory-open-lock-contention-no-wipe.yaml"
MODE="${1:-}"

cd "$REPO_ROOT"

if [[ ! -f "$SPEC" ]]; then
  echo "❌ done-gate spec not found: $SPEC"
  echo "   The done-gate cannot certify the goal without its criteria spec."
  exit 1
fi

# ── 1. The open-serialization guard delivered by #4317 must still exist ────────
if [[ ! -f "$GUARD_SRC" ]]; then
  echo "❌ open-serialization guard missing: $GUARD_SRC"
  echo "   The protection that stops lock-contention from wiping memory is gone."
  exit 1
fi

# ── 2. Re-assert the regression tests that prove the fix (the measurable gate) ─
# These are the exact tests shipped by merged PR #4317. They fail if a contended
# open ever again proceeds into lbug's destructive rebuild instead of failing loud.
echo "Running the open-serialization guard unit tests..."
GUARD_OK=1
if ! cargo test --quiet --lib --manifest-path "$REPO_ROOT/Cargo.toml" -- \
    cognitive_memory::open_guard::tests; then
  GUARD_OK=0
fi

echo ""
echo "Running the lock-contention-never-wipes-records regression test..."
REGRESSION_OK=1
if ! cargo test --quiet --lib --manifest-path "$REPO_ROOT/Cargo.toml" -- \
    cognitive_memory::tests_library_parity::lock_contention_no_wipe; then
  REGRESSION_OK=0
fi

# ── 3. Optional: confirm the outside-in qa scenario asset is present ───────────
QA_OK=1
if [[ "$MODE" == "--full" ]]; then
  echo ""
  echo "Confirming the outside-in qa scenario asset (--full)..."
  if [[ -f "$QA_SCENARIO" ]]; then
    echo "  → present: $(basename "$QA_SCENARIO")"
  else
    echo "  ⚠️  missing qa scenario: $QA_SCENARIO"
    QA_OK=0
  fi
fi

# ── Verdict ───────────────────────────────────────────────────────────────────
echo ""
if [[ "$GUARD_OK" == "1" && "$REGRESSION_OK" == "1" && "$QA_OK" == "1" ]]; then
  echo "✅ DONE — a contended open fails loud and never wipes records; the"
  echo "   lbug lock-contention-as-corruption failure can no longer happen. The goal"
  echo "   stop-lbug-lock-contention-from-being-mistaken-f-0ebf1bc7 is certified."
  exit 0
fi

echo "⏳ Not done yet:"
[[ "$GUARD_OK" == "1" ]] && echo "   • open_guard tests: PASS" \
                         || echo "   • open_guard tests: FAILING (see above)"
[[ "$REGRESSION_OK" == "1" ]] && echo "   • lock-contention-no-wipe regression: PASS" \
                              || echo "   • lock-contention-no-wipe regression: FAILING (see above)"
if [[ "$MODE" == "--full" ]]; then
  [[ "$QA_OK" == "1" ]] && echo "   • qa scenario asset: present" \
                        || echo "   • qa scenario asset: missing (see above)"
fi
exit 1
