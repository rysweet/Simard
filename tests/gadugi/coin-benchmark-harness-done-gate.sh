#!/usr/bin/env bash
# qa-team scenario — the coin-benchmark-harness blocked-goal triage is durable,
# jargon-free, and its done-gate helper is machine-checkable and fail-open.
#
# TDD contract (written before the verification helper exists). This is the
# outside-in specification for the Overseer blocked-goal triage of goal
# `build-a-local-coin-benchmark-harness-and-a-self-09e65e35`, whose decision was
# `complete-delivered-goal` (the deliverable had already merged: issue #2713
# CLOSED, PR #4171 MERGED). Two bricks are under test:
#
#   Brick A — docs/operations/coin-benchmark-harness-goal-signal-2026-07-18.md
#             the durable, operator-facing triage record. Must carry the
#             verified-state evidence, the four Signal messages verbatim, and a
#             six-field OUTPUT JSON (escalate:null, decision complete-delivered-goal),
#             with ZERO internal marker/lock tokens leaking to operators.
#
#   Brick B — scripts/check-coin-benchmark-harness-done-gate.sh
#             a read-only, fail-open helper that proves the finish condition:
#             issue #2713 CLOSED and PR #4171 MERGED (optionally coin-gym verify
#             exits 0). PASS => exit 0; a genuinely failed criterion => non-zero;
#             gh missing/unauthenticated/offline => WARN + exit 0 (never a silent
#             PASS). Every gh call pinned to rysweet/Simard; strict mode; no set -x;
#             no token leakage.
#
# The suite ACCUMULATES failures (no `set -e`) so the full contract is visible.
# It exits non-zero if any assertion fails. Until Brick B ships this suite fails
# by design — that is the point of writing the tests first.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT" || exit 2

SCRIPT="scripts/check-coin-benchmark-harness-done-gate.sh"
DOC="docs/operations/coin-benchmark-harness-goal-signal-2026-07-18.md"
INDEX="docs/operations/index.md"
GOAL_ID="build-a-local-coin-benchmark-harness-and-a-self-09e65e35"

# Internal diagnostic markers / lock tokens that must NEVER surface to operators.
FORBIDDEN_TOKENS=(
  'OODA-SAFEGUARD'
  'UNCLEAR-CRITERIA'
  'GENUINELY-STUCK'
  'no-action cycles'
  'why='
  'evidence=['
  $'\xf0\x9f\x94\x92'   # 🔒 lock emoji
)

FAILURES=0
pass() { printf 'OK:   %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; FAILURES=$((FAILURES + 1)); }

# ---------------------------------------------------------------------------
# Fake `gh` (and optional `coin-gym`) so behavior is deterministic and offline.
# ---------------------------------------------------------------------------
FAKEBIN="$(mktemp -d "${TMPDIR:-/tmp}/coin-donegate-XXXXXX")"
trap 'rm -rf "$FAKEBIN"' EXIT

cat > "$FAKEBIN/gh" <<'FAKE_GH'
#!/usr/bin/env bash
# Deterministic gh stand-in. Honors:
#   FAKE_GH_AUTH_OK  (1 => authenticated/online, else auth+view calls fail)
#   FAKE_ISSUE_STATE (default CLOSED)
#   FAKE_PR_STATE    (default MERGED)
# Emits the raw field value when --jq/-q is present, else a JSON object,
# mirroring real gh so the script may parse either way.
sub="${1:-}"; shift || true
has_jq=0
for a in "$@"; do
  case "$a" in --jq|-q) has_jq=1 ;; esac
done
emit_state() {
  local state="$1"
  if [[ "$has_jq" == "1" ]]; then printf '%s\n' "$state"; else printf '{"state":"%s"}\n' "$state"; fi
}
case "$sub" in
  --version) echo "gh version 2.0.0 (fake)"; exit 0 ;;
  auth)
    if [[ "${FAKE_GH_AUTH_OK:-1}" == "1" ]]; then echo "Logged in (fake)"; exit 0; fi
    echo "not logged into any GitHub hosts (fake)" >&2; exit 1 ;;
  issue)
    if [[ "${FAKE_GH_AUTH_OK:-1}" != "1" ]]; then echo "gh: could not reach GitHub (fake)" >&2; exit 1; fi
    emit_state "${FAKE_ISSUE_STATE:-CLOSED}"; exit 0 ;;
  pr)
    if [[ "${FAKE_GH_AUTH_OK:-1}" != "1" ]]; then echo "gh: could not reach GitHub (fake)" >&2; exit 1; fi
    emit_state "${FAKE_PR_STATE:-MERGED}"; exit 0 ;;
  *) echo "fake gh: unhandled subcommand: $sub" >&2; exit 2 ;;
esac
FAKE_GH
chmod +x "$FAKEBIN/gh"

cat > "$FAKEBIN/coin-gym" <<'FAKE_CG'
#!/usr/bin/env bash
# Optional finish-condition binary. PASS-shaped by default.
if [[ "${1:-}" == "verify" ]]; then
  echo "result: ${FAKE_COINGYM_RESULT:-7/7 criteria passed}"
  exit "${FAKE_COINGYM_RC:-0}"
fi
exit 0
FAKE_CG
chmod +x "$FAKEBIN/coin-gym"

# run_script <env-assignments...> -- captures combined output + exit code into
# globals OUT and RC without aborting the suite.
run_script() {
  OUT="$(env "$@" PATH="$FAKEBIN:$PATH" bash "$SCRIPT" 2>&1)"
  RC=$?
}

# ===========================================================================
# Brick B — verification helper: static contract
# ===========================================================================
echo "== Brick B: script static contract =="

if [[ -x "$SCRIPT" ]]; then
  pass "$SCRIPT exists and is executable"
else
  fail "$SCRIPT is missing or not executable (Brick B not yet built)"
fi

if [[ -f "$SCRIPT" ]]; then
  if grep -Eq '^[[:space:]]*set -euo pipefail' "$SCRIPT"; then
    pass "script uses strict mode (set -euo pipefail)"
  else
    fail "script must declare 'set -euo pipefail'"
  fi

  if grep -Eq '^[[:space:]]*set -x' "$SCRIPT"; then
    fail "script must NOT enable 'set -x' (token/tracing leakage risk)"
  else
    pass "script does not enable shell tracing (no set -x)"
  fi

  if grep -q 'rysweet/Simard' "$SCRIPT"; then
    pass "script pins GitHub calls to rysweet/Simard"
  else
    fail "script must pin every gh call with --repo rysweet/Simard"
  fi

  # every gh issue/pr view line must carry an explicit --repo pin
  unpinned="$(grep -nE 'gh[[:space:]]+(issue|pr)[[:space:]]+view' "$SCRIPT" \
              | grep -v -- '--repo' || true)"
  if [[ -z "$unpinned" ]]; then
    pass "all gh issue/pr view calls carry an explicit --repo pin"
  else
    fail "gh view call(s) missing --repo pin: $unpinned"
  fi

  if grep -q 'command -v gh' "$SCRIPT"; then
    pass "script guards on gh availability (command -v gh)"
  else
    fail "script must guard on gh availability for the fail-open path"
  fi

  if grep -q '2713' "$SCRIPT" && grep -q '4171' "$SCRIPT"; then
    pass "script references issue #2713 and PR #4171"
  else
    fail "script must reference issue #2713 and PR #4171"
  fi

  if grep -q -- '--token' "$SCRIPT"; then
    fail "script must not pass tokens via --token (rely on ambient gh auth)"
  else
    pass "script relies on ambient gh auth (no --token)"
  fi

  # no internal markers / lock tokens embedded in the helper
  tok_hits=0
  for t in "${FORBIDDEN_TOKENS[@]}"; do
    if grep -Fq -- "$t" "$SCRIPT"; then fail "script leaks forbidden token: $t"; tok_hits=$((tok_hits + 1)); fi
  done
  [[ "$tok_hits" -eq 0 ]] && pass "script contains no forbidden marker/lock tokens"

  if command -v shellcheck >/dev/null 2>&1; then
    if shellcheck -x "$SCRIPT" >/tmp/coin-donegate-shellcheck.txt 2>&1; then
      pass "shellcheck is clean"
    else
      fail "shellcheck reported issues:"; cat /tmp/coin-donegate-shellcheck.txt >&2
    fi
  else
    echo "SKIP: shellcheck not installed"
  fi
else
  fail "cannot run static checks — $SCRIPT does not exist"
fi

# ===========================================================================
# Brick B — verification helper: behavior (deterministic, offline)
# ===========================================================================
echo "== Brick B: script behavior =="

if [[ -x "$SCRIPT" ]]; then
  # PASS: issue CLOSED, PR MERGED, gh online, coin-gym verify exits 0.
  run_script FAKE_GH_AUTH_OK=1 FAKE_ISSUE_STATE=CLOSED FAKE_PR_STATE=MERGED FAKE_COINGYM_RC=0
  if [[ "$RC" -eq 0 ]] && printf '%s' "$OUT" | grep -qiE 'PASS'; then
    pass "PASS path: CLOSED+MERGED => exit 0 and reports PASS"
  else
    fail "PASS path expected exit 0 + PASS; got rc=$RC out=<<$OUT>>"
  fi

  # FAIL: issue not CLOSED (still OPEN) => non-zero, names the issue criterion.
  run_script FAKE_GH_AUTH_OK=1 FAKE_ISSUE_STATE=OPEN FAKE_PR_STATE=MERGED
  if [[ "$RC" -ne 0 ]] && printf '%s' "$OUT" | grep -qiE 'issue|2713|FAIL'; then
    pass "FAIL path: issue not CLOSED => non-zero and names the failed criterion"
  else
    fail "issue-open path expected non-zero + failure detail; got rc=$RC out=<<$OUT>>"
  fi

  # FAIL: PR not MERGED (OPEN) => non-zero, names the PR criterion.
  run_script FAKE_GH_AUTH_OK=1 FAKE_ISSUE_STATE=CLOSED FAKE_PR_STATE=OPEN
  if [[ "$RC" -ne 0 ]] && printf '%s' "$OUT" | grep -qiE 'pull request|PR|4171|FAIL'; then
    pass "FAIL path: PR not MERGED => non-zero and names the failed criterion"
  else
    fail "pr-open path expected non-zero + failure detail; got rc=$RC out=<<$OUT>>"
  fi

  # WARN (fail-open): gh unauthenticated/offline => exit 0, WARN, never PASS.
  run_script FAKE_GH_AUTH_OK=0
  if [[ "$RC" -eq 0 ]] \
     && printf '%s' "$OUT" | grep -qiE 'WARN' \
     && ! printf '%s' "$OUT" | grep -qiE 'PASS'; then
    pass "WARN path: gh offline/unauth => exit 0, WARN, and NOT a silent PASS"
  else
    fail "offline path expected exit 0 + WARN + no PASS; got rc=$RC out=<<$OUT>>"
  fi
else
  fail "cannot run behavior checks — $SCRIPT is not executable"
fi

# ===========================================================================
# Brick A — durable operations record
# ===========================================================================
echo "== Brick A: operations record =="

if [[ -f "$DOC" ]]; then
  pass "operations record exists"

  # Required operator-facing structure.
  required_headings=(
    '## At a glance'
    '## Verified state'
    '## Root cause'
    '## Decision'
    '## The finish condition'
    '## Operator updates'
    '## Machine-readable result'
  )
  for h in "${required_headings[@]}"; do
    if grep -Fq "$h" "$DOC"; then pass "record has section: $h"; else fail "record missing section: $h"; fi
  done

  if grep -Fq "$GOAL_ID" "$DOC"; then pass "record names the goal id"; else fail "record must name goal id $GOAL_ID"; fi

  # Verified-state evidence: issue CLOSED and PR MERGED must both appear.
  if grep -q '2713' "$DOC" && grep -qi 'CLOSED' "$DOC"; then
    pass "record documents issue #2713 CLOSED"
  else
    fail "record must document issue #2713 as CLOSED"
  fi
  if grep -q '4171' "$DOC" && grep -qi 'MERGED' "$DOC"; then
    pass "record documents PR #4171 MERGED"
  else
    fail "record must document PR #4171 as MERGED"
  fi

  # Does not perpetuate the duplicate-triage loop.
  if grep -q '4322' "$DOC" && grep -q '4326' "$DOC"; then
    pass "record references existing duplicate triage PRs #4322/#4326"
  else
    fail "record should reference existing duplicate PRs #4322/#4326 (not open a third)"
  fi

  # Four operator Signal messages, one per triage step.
  for n in 1 2 3 4; do
    if grep -Eq "^${n}\. \*\*" "$DOC"; then
      pass "record contains Signal message #$n"
    else
      fail "record missing numbered Signal message #$n"
    fi
  done

  # No internal marker/lock tokens surfaced to operators.
  tok_hits=0
  for t in "${FORBIDDEN_TOKENS[@]}"; do
    if grep -Fq -- "$t" "$DOC"; then fail "record leaks forbidden token: $t"; tok_hits=$((tok_hits + 1)); fi
  done
  [[ "$tok_hits" -eq 0 ]] && pass "record contains no forbidden marker/lock tokens"

  # Six-field OUTPUT JSON, machine-parseable, correct decision + null escalate.
  JSON="$(awk '/^```json$/{f=1;next} /^```$/{if(f){exit}} f{print}' "$DOC")"
  if [[ -n "$JSON" ]] && printf '%s' "$JSON" | jq -e . >/dev/null 2>&1; then
    pass "OUTPUT JSON block is valid JSON"

    if printf '%s' "$JSON" | jq -e '
        has("problem") and has("next_step") and has("root_cause")
        and has("decision") and has("action_taken") and has("escalate")' >/dev/null; then
      pass "OUTPUT JSON has all six required fields"
    else
      fail "OUTPUT JSON must have problem,next_step,root_cause,decision,action_taken,escalate"
    fi

    dec="$(printf '%s' "$JSON" | jq -r '.decision')"
    if [[ "$dec" == "complete-delivered-goal" ]]; then
      pass "OUTPUT JSON decision == complete-delivered-goal"
    else
      fail "OUTPUT JSON decision expected complete-delivered-goal, got '$dec'"
    fi

    if printf '%s' "$JSON" | jq -e '.escalate == null' >/dev/null; then
      pass "OUTPUT JSON escalate is null (no human escalation)"
    else
      fail "OUTPUT JSON escalate must be null"
    fi
  else
    fail "record must contain a valid fenced json OUTPUT block"
  fi
else
  fail "operations record $DOC does not exist"
fi

# Discoverability: linked from the operations index.
if [[ -f "$INDEX" ]] && grep -q 'coin-benchmark-harness-goal-signal-2026-07-18.md' "$INDEX"; then
  pass "record is linked from the operations index"
else
  fail "record must be linked from $INDEX"
fi

# ---------------------------------------------------------------------------
echo
if [[ "$FAILURES" -eq 0 ]]; then
  echo "PASS: coin-benchmark-harness done-gate triage contract satisfied"
  exit 0
fi
echo "FAILED: $FAILURES assertion(s) not satisfied (expected until Brick B ships)"
exit 1
