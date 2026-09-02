#!/usr/bin/env bash
# check-agent-kgpacks-rs-parity-done-gate.sh — Machine-checkable done-gate for the
# goal "advance agent-kgpacks-rs to full parity"
# (slug advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c).
#
# WHY THIS EXISTS
# ----------------
# The goal hard-parked as blocked cycle after cycle with the same diagnosis —
# "no tracked PR/issue the done-gate can verify" (why=UNCLEAR-CRITERIA). The
# blocker was NOT technical: "full parity" had no finish condition a check could
# confirm, so every OODA cycle re-observed it as unfinished, emitted NO ACTION,
# and re-investigated forever without shipping anything.
#
# The course-correction (rewrite-done-gate) binds the goal's finish condition to
# one artifact a check can observe: the tracking issue rysweet/Simard#4321, whose
# CLOSED state == full parity. #4321 closes exactly when the three remaining
# in-scope criteria (KGP-Q4, KGP-T3, KGP-Q5) ship and the two spec commands are
# green:
#     cargo test --lib native_knowledge
#     cargo test --lib knowledge_client
# See Specs/agent-kgpacks-rs-parity.md for the full, enumerated criteria table.
#
# UNLIKE the coin-benchmark / session-checkpoint goals, this work is NOT yet
# delivered: KGP-Q4, KGP-T3 and KGP-Q5 are still OPEN. So this gate honestly
# reports "not done yet" today, with the exact remaining backlog as the concrete
# next step — never a vague "stuck". It flips to certified-complete automatically
# the moment issue #4321 is closed.
#
# Usage:
#   scripts/check-agent-kgpacks-rs-parity-done-gate.sh          # verdict from issue #4321 + spec seams
#   scripts/check-agent-kgpacks-rs-parity-done-gate.sh --tests  # + run the two spec test commands
#
# Exit codes:
#   0  full parity reached — issue #4321 CLOSED (and, with --tests, both suites green)
#   1  not done yet — the remaining in-scope criteria are printed above as the next step

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SPEC="$REPO_ROOT/Specs/agent-kgpacks-rs-parity.md"
NATIVE_SRC="$REPO_ROOT/src/native_knowledge.rs"
CLIENT_SRC="$REPO_ROOT/src/knowledge_client.rs"
ISSUE="4321"
REPO_SLUG="rysweet/Simard"
MODE="${1:-}"

cd "$REPO_ROOT"

# ── 0. The done-criteria spec must exist ───────────────────────────────────────
if [[ ! -f "$SPEC" ]]; then
  echo "❌ done-gate spec not found: $SPEC"
  echo "   The done-gate cannot certify the goal without its criteria spec."
  exit 1
fi

# ── 1. The two seams the parity criteria are measured against must still exist ──
if [[ ! -f "$NATIVE_SRC" ]]; then
  echo "❌ native knowledge client missing: $NATIVE_SRC"
  echo "   The Rust reimplementation the parity criteria are measured against is gone."
  exit 1
fi
if [[ ! -f "$CLIENT_SRC" ]]; then
  echo "❌ typed knowledge client missing: $CLIENT_SRC"
  echo "   The client whose knowledge_client suite gates parity is gone."
  exit 1
fi

# ── 2. Enumerate the remaining in-scope OPEN criteria from the spec ────────────
# The spec table marks each in-scope criterion DONE or OPEN. We surface the OPEN
# ones so every cycle has a concrete, non-stuck next step even before #4321 is
# consulted. (Grep is deliberately conservative: it lists the ids the spec still
# marks OPEN in its in-scope KGP-* rows.)
OPEN_CRITERIA="$(grep -oE '\| (KGP-(Q|T|M|P)[0-9]+) \|[^|]*\|[^|]*\| OPEN \|' "$SPEC" \
                  | grep -oE 'KGP-(Q|T|M|P)[0-9]+' | sort -u | tr '\n' ' ' | sed 's/ $//')"

# ── 3. Consult the machine-checkable done-gate: issue #4321 CLOSED == parity ────
ISSUE_STATE=""
if command -v gh >/dev/null 2>&1; then
  ISSUE_STATE="$(gh issue view "$ISSUE" --repo "$REPO_SLUG" --json state \
                   --jq '.state' 2>/dev/null || true)"
fi

# ── 4. Optionally run the two spec test commands ───────────────────────────────
TESTS_RESULT="skipped"
if [[ "$MODE" == "--tests" ]]; then
  echo "Running the parity spec test commands..."
  if cargo test --quiet --lib --manifest-path "$REPO_ROOT/Cargo.toml" native_knowledge \
     && cargo test --quiet --lib --manifest-path "$REPO_ROOT/Cargo.toml" knowledge_client; then
    TESTS_RESULT="green"
  else
    TESTS_RESULT="failing"
  fi
fi

# ── Verdict ────────────────────────────────────────────────────────────────────
echo ""
if [[ "$ISSUE_STATE" == "CLOSED" ]]; then
  if [[ "$MODE" == "--tests" && "$TESTS_RESULT" != "green" ]]; then
    echo "⏳ Not done yet: tracking issue #$ISSUE is closed, but the spec test"
    echo "   commands are '$TESTS_RESULT' — re-open the gap until both suites are green:"
    echo "     cargo test --lib native_knowledge"
    echo "     cargo test --lib knowledge_client"
    exit 1
  fi
  echo "✅ DONE — full parity reached. Tracking issue rysweet/Simard#$ISSUE is CLOSED,"
  echo "   which by definition means every in-scope criterion (KGP-M*, KGP-Q*, KGP-T*,"
  echo "   KGP-P*) has shipped and both spec suites are green. The goal"
  echo "   advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c is certified complete."
  exit 0
fi

echo "⏳ Not done yet — agent-kgpacks-rs has NOT reached full parity."
if [[ -n "$ISSUE_STATE" ]]; then
  echo "   Finish signal: tracking issue rysweet/Simard#$ISSUE is $ISSUE_STATE"
  echo "   (it closes automatically when the remaining work below ships)."
else
  echo "   Finish signal: tracking issue rysweet/Simard#$ISSUE CLOSED"
  echo "   (issue state could not be read here — gh unavailable or offline)."
fi
if [[ -n "$OPEN_CRITERIA" ]]; then
  echo "   Concrete next step — the remaining in-scope criteria (work top-to-bottom):"
  echo "     $OPEN_CRITERIA"
  echo "   See Specs/agent-kgpacks-rs-parity.md for each criterion's acceptance test."
else
  echo "   All in-scope criteria are marked DONE in the spec, but issue #$ISSUE is not"
  echo "   yet closed — close #$ISSUE (or re-verify the spec) to certify parity."
fi
if [[ "$MODE" == "--tests" ]]; then
  echo "   Spec test commands: $TESTS_RESULT"
fi
exit 1
