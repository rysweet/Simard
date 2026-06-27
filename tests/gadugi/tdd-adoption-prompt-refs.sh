#!/usr/bin/env bash
# qa-team scenario for issue #2317 — TDD adoption Phase 1 prompt references.
#
# Outside-in verification that the in-repo prompt assets now encode the
# ratified TDD charter (Specs/TDD_ADOPTION.md):
#
#   1. prompt_assets/simard/meeting_system.md references Specs/TDD_ADOPTION.md
#      near the `simard goal set-priority adopt-tdd 1` example line, so the
#      meeting facilitator links the ratified spec instead of re-creating the
#      `adopt-tdd` goal from boilerplate (spec §5, last exit-criterion).
#   2. prompt_assets/simard/engineer_system.md PR-evidence heading list carries
#      the `tdd:` / `tdd-exempt:` self-attestation row with all three ratified
#      forms (spec §3 Layer 2).
#
# The validation surface for a prompt-asset edit is the prompt text itself
# (tests cannot meaningfully precede a prompt edit — spec §1.1), so this
# scenario asserts directly against the checked-in prompt files. It must run
# against the in-repo prompt_assets/, never the deployed ~/.simard copy.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

MEETING="prompt_assets/simard/meeting_system.md"
ENGINEER="prompt_assets/simard/engineer_system.md"
SPEC="Specs/TDD_ADOPTION.md"

fail() {
  echo "[gadugi] FAIL: $1" >&2
  exit 1
}

# Preconditions: all three files exist.
[ -f "$MEETING" ] || fail "$MEETING not found"
[ -f "$ENGINEER" ] || fail "$ENGINEER not found"
[ -f "$SPEC" ] || fail "$SPEC (ratified charter) not found"

# --- Deliverable 1: meeting prompt references the spec near the adopt-tdd line.
ANCHOR_LINE="$(grep -n 'simard goal set-priority adopt-tdd 1' "$MEETING" | head -1 | cut -d: -f1)"
[ -n "$ANCHOR_LINE" ] || fail "adopt-tdd example line missing from $MEETING"

# The spec reference must appear, and within a 20-line window of the example
# line, proving it sits "near" the boilerplate it is meant to suppress.
REF_LINE="$(grep -n "$SPEC" "$MEETING" | head -1 | cut -d: -f1)"
[ -n "$REF_LINE" ] || fail "$MEETING does not reference $SPEC"
DELTA=$(( REF_LINE - ANCHOR_LINE ))
[ "$DELTA" -lt 0 ] && DELTA=$(( -DELTA ))
[ "$DELTA" -le 20 ] || fail "$SPEC reference is $DELTA lines from the adopt-tdd example (want <=20)"

# The note must steer away from re-creating the goal from boilerplate.
grep -Eq 'do \*\*not\*\* re-create|do not re-create' "$MEETING" \
  || fail "$MEETING reference does not tell the facilitator to stop re-creating the goal"

echo "[gadugi] meeting prompt references $SPEC at line $REF_LINE (anchor line $ANCHOR_LINE, delta $DELTA) ... ok"

# --- Deliverable 2: engineer prompt carries the tdd: attestation row.
grep -Fq 'tdd: test-first ordering verified —' "$ENGINEER" \
  || fail "$ENGINEER missing the default 'tdd: test-first ordering verified' attestation form"
grep -Fq 'tdd-exempt: <reason' "$ENGINEER" \
  || fail "$ENGINEER missing the 'tdd-exempt:' attestation form"
grep -Fq 'tdd: not applicable — PR touches no in-scope paths' "$ENGINEER" \
  || fail "$ENGINEER missing the 'tdd: not applicable' attestation form"

# The attestation row must live inside the PR-evidence heading block.
grep -Fq 'TDD attestation' "$ENGINEER" \
  || fail "$ENGINEER attestation row missing its evidence heading"

echo "[gadugi] engineer prompt evidence headings include the tdd:/tdd-exempt: attestation row ... ok"

echo "[gadugi] TDD adoption Phase 1 prompt references (#2317): all behaviors verified"
