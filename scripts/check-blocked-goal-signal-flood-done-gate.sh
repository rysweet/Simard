#!/usr/bin/env bash
# check-blocked-goal-signal-flood-done-gate.sh — Machine-checkable done-gate for
# the goal "stop the blocked-goal signal flood; make the Overseer course-correct
# before escalating" (slug stop-the-blocked-goal-signal-flood-make-oversee-17d6ca84).
#
# WHY THIS EXISTS
# ----------------
# The goal stayed Blocked cycle after cycle with the same diagnosis — "no tracked
# PR/issue the done-gate can verify". The blocker was NOT technical: the anti-flood
# behaviour the goal asks for already shipped. Two protections stop the flood and
# one makes the Overseer fix a block itself before ever paging a human:
#
#   1. A cadence rail — the Overseer's blocked-goal escalations go through a
#      back-off gate (WhisperGate::with_backoff), so a repeatedly-blocked goal is
#      re-surfaced on an exponentially widening interval instead of every tick.
#   2. Agentic course-correct-before-escalate — a genuinely blocked goal is handed
#      to the escalation-triage recipe (prompt_assets/simard/overseer/
#      escalation_triage.md, wired via act_escalate_blocked_goal), which restates
#      the block in plain English and repairs it, only paging a person when a human
#      decision is truly required.
#   3. Plain-English operator copy — the operator notification never renders raw
#      diagnostic markers.
#
# The goal only stayed on the "stuck" list because its finish condition had no
# definition a check could confirm. This script turns that finish condition into a
# single command the done-gate can run: it confirms the delivered anti-flood seams
# still exist and re-asserts the exact tests that prove they work. It is the
# machine-checkable artifact the goal's done-criteria points at
# (see Specs/blocked-goal-signal-flood-done-gate.md).
#
# Because the work is already delivered, this gate certifies the goal as COMPLETE.
# It exits 0 while the delivered behaviour is present and green, and turns red only
# if any of that behaviour is ever removed or regresses.
#
# Usage:
#   scripts/check-blocked-goal-signal-flood-done-gate.sh          # run the anti-flood tests
#   scripts/check-blocked-goal-signal-flood-done-gate.sh --full   # + confirm the operator doc asset
#
# Exit codes:
#   0  the cadence rail + agentic triage seam + their tests hold — goal certified complete
#   1  not done — a failing check is printed above

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SPEC="$REPO_ROOT/Specs/blocked-goal-signal-flood-done-gate.md"
OVERSEER_SRC="$REPO_ROOT/src/overseer/mod.rs"
TRIAGE_ASSET="$REPO_ROOT/prompt_assets/simard/overseer/escalation_triage.md"
OPS_DOC="$REPO_ROOT/docs/operations/blocked-goal-signal-flood-goal-signal-2026-07-18.md"
MODE="${1:-}"

cd "$REPO_ROOT"

if [[ ! -f "$SPEC" ]]; then
  echo "❌ done-gate spec not found: $SPEC"
  echo "   The done-gate cannot certify the goal without its criteria spec."
  exit 1
fi

# ── 1. The cadence rail that spaces out re-escalations must still exist ─────────
if [[ ! -f "$OVERSEER_SRC" ]] || ! grep -q "blocked_goal_gate: WhisperGate::with_backoff" "$OVERSEER_SRC"; then
  echo "❌ cadence rail missing: blocked_goal_gate: WhisperGate::with_backoff in $OVERSEER_SRC"
  echo "   The back-off gate that stops repeated blocked-goal escalations from"
  echo "   flooding every tick is gone."
  exit 1
fi

# ── 2. The agentic course-correct-before-escalate seam must still exist ─────────
if ! grep -q "fn act_escalate_blocked_goal" "$OVERSEER_SRC"; then
  echo "❌ escalation seam missing: act_escalate_blocked_goal in $OVERSEER_SRC"
  echo "   The path that hands a blocked goal to the triage recipe (course-correct"
  echo "   before paging a human) is gone."
  exit 1
fi

if [[ ! -f "$TRIAGE_ASSET" ]]; then
  echo "❌ triage asset missing: $TRIAGE_ASSET"
  echo "   The agentic reasoning contract the Overseer follows before escalating is gone."
  exit 1
fi

# ── 3. Re-assert the tests that prove the anti-flood behaviour (the gate) ───────
# These pin: the back-off gate widens re-escalation intervals exponentially and
# per-signature (whisper_backoff_tests); a blocked goal is triaged agentically and
# the operator is notified in plain English with no marker passthrough
# (tests_escalation_triage); and goal-board health emits its signal and escalates
# on both channels (tests_goal_health).
echo "Running the blocked-goal anti-flood tests..."
TESTS_OK=1
if ! cargo test --quiet --lib --manifest-path "$REPO_ROOT/Cargo.toml" -- \
    overseer::guardrails::whisper_backoff_tests \
    overseer::tests_escalation_triage \
    overseer::tests_goal_health; then
  TESTS_OK=0
fi

# ── 4. Optional: confirm the operator Signal doc asset is present ───────────────
DOC_OK=1
if [[ "$MODE" == "--full" ]]; then
  echo ""
  echo "Confirming the operator update doc (--full)..."
  if [[ -f "$OPS_DOC" ]]; then
    echo "  → present: $(basename "$OPS_DOC")"
  else
    echo "  ⚠️  missing operator doc: $OPS_DOC"
    DOC_OK=0
  fi
fi

# ── Verdict ─────────────────────────────────────────────────────────────────────
echo ""
if [[ "$TESTS_OK" == "1" && "$DOC_OK" == "1" ]]; then
  echo "✅ DONE — the Overseer no longer floods a human with raw blocked-goal"
  echo "   markers: repeated escalations are spaced out by a back-off gate, and a"
  echo "   genuinely blocked goal is course-corrected agentically (plain English)"
  echo "   before anyone is paged. The goal"
  echo "   stop-the-blocked-goal-signal-flood-make-oversee-17d6ca84 is certified complete."
  exit 0
fi

echo "⏳ Not done yet:"
[[ "$TESTS_OK" == "1" ]] && echo "   • anti-flood tests: PASS" \
                         || echo "   • anti-flood tests: FAILING (see above)"
if [[ "$MODE" == "--full" ]]; then
  [[ "$DOC_OK" == "1" ]] && echo "   • operator doc asset: present" \
                         || echo "   • operator doc asset: missing (see above)"
fi
exit 1
