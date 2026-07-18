#!/usr/bin/env bash
# check-engineer-session-checkpoint-resume-done-gate.sh — Machine-checkable
# done-gate for the goal "engineers must support session checkpoint and resume"
# (slug engineers-must-support-session-checkpoint-and-r-aad52503).
#
# WHY THIS EXISTS
# ----------------
# The goal stayed Blocked cycle after cycle with the same diagnosis — "no
# tracked PR/issue the done-gate can verify" (why=UNCLEAR-CRITERIA). The blocker
# was NOT technical: the feature already shipped in merged PR #4311
# ("resume interrupted sessions from checkpoint (idempotent)"). Engineers already
# checkpointed their session at each phase boundary but never RESUMED — a fresh
# process (after a crash, restart, or deploy binary-swap) restarted the goal from
# scratch, re-spawning the expensive agent session and risking a duplicate PR. PR
# #4311 wired resume-on-startup into run_local_engineer_loop so a completed agent
# session is never re-spawned (no double-PR, no duplicate work).
#
# The blocker was that the goal's finish condition had no definition a check
# could confirm. This script turns that finish condition into a single command
# the done-gate can run: it confirms the delivered resume seam still exists and
# re-asserts the exact tests that prove checkpoint-and-resume works idempotently.
# It is the machine-checkable artifact the goal's done-criteria points at
# (see Specs/engineer-session-checkpoint-resume-done-gate.md).
#
# Because the work is already delivered, this gate certifies the goal as
# COMPLETE. It exits 0 while the delivered behaviour is present and green, and
# turns red only if that behaviour is ever removed or regresses.
#
# Usage:
#   scripts/check-engineer-session-checkpoint-resume-done-gate.sh          # run the resume tests
#   scripts/check-engineer-session-checkpoint-resume-done-gate.sh --full   # + confirm the reference doc asset
#
# Exit codes:
#   0  the checkpoint-and-resume seam + its tests hold — the goal is certified complete
#   1  not done — a failing check is printed above

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SPEC="$REPO_ROOT/Specs/engineer-session-checkpoint-resume-done-gate.md"
LOOP_SRC="$REPO_ROOT/src/engineer_loop/mod.rs"
TYPES_SRC="$REPO_ROOT/src/engineer_loop/types.rs"
REF_DOC="$REPO_ROOT/docs/reference/engineer-session-checkpoint-resume.md"
MODE="${1:-}"

cd "$REPO_ROOT"

if [[ ! -f "$SPEC" ]]; then
  echo "❌ done-gate spec not found: $SPEC"
  echo "   The done-gate cannot certify the goal without its criteria spec."
  exit 1
fi

# ── 1. The resume seam delivered by #4311 must still exist ─────────────────────
if [[ ! -f "$LOOP_SRC" ]] || ! grep -q "fn should_resume" "$LOOP_SRC"; then
  echo "❌ resume decision seam missing: should_resume in $LOOP_SRC"
  echo "   The logic that resumes an interrupted session is gone."
  exit 1
fi

if [[ ! -f "$TYPES_SRC" ]] || ! grep -q "fn resumable_execution" "$TYPES_SRC"; then
  echo "❌ idempotency linchpin missing: resumable_execution in $TYPES_SRC"
  echo "   The guard that reuses a completed agent session (no double-PR) is gone."
  exit 1
fi

# ── 2. Re-assert the tests that prove the feature (the measurable gate) ────────
# These are the exact tests shipped by merged PR #4311. They fail if resume ever
# re-spawns a completed agent session (duplicate work / duplicate PR) or resumes
# a mismatched / terminal checkpoint.
echo "Running the checkpoint-and-resume tests..."
RESUME_OK=1
if ! cargo test --quiet --lib --manifest-path "$REPO_ROOT/Cargo.toml" -- \
    engineer_loop::tests_resume; then
  RESUME_OK=0
fi

# ── 3. Optional: confirm the reference doc asset is present ────────────────────
DOC_OK=1
if [[ "$MODE" == "--full" ]]; then
  echo ""
  echo "Confirming the checkpoint-and-resume reference doc (--full)..."
  if [[ -f "$REF_DOC" ]]; then
    echo "  → present: $(basename "$REF_DOC")"
  else
    echo "  ⚠️  missing reference doc: $REF_DOC"
    DOC_OK=0
  fi
fi

# ── Verdict ───────────────────────────────────────────────────────────────────
echo ""
if [[ "$RESUME_OK" == "1" && "$DOC_OK" == "1" ]]; then
  echo "✅ DONE — a fresh engineer process resumes an interrupted session from its"
  echo "   checkpoint and never re-spawns a completed agent session (no double-PR,"
  echo "   no duplicate work). The goal"
  echo "   engineers-must-support-session-checkpoint-and-r-aad52503 is certified complete."
  exit 0
fi

echo "⏳ Not done yet:"
[[ "$RESUME_OK" == "1" ]] && echo "   • checkpoint-and-resume tests: PASS" \
                          || echo "   • checkpoint-and-resume tests: FAILING (see above)"
if [[ "$MODE" == "--full" ]]; then
  [[ "$DOC_OK" == "1" ]] && echo "   • reference doc asset: present" \
                         || echo "   • reference doc asset: missing (see above)"
fi
exit 1
