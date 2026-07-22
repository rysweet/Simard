#!/usr/bin/env bash
# ============================================================================
# scripts/verify-escalation-triage.sh
#
# TDD acceptance gate (RED-first) for issue #4455 —
#   "Triage and course-correct a blocked goal before escalating to a human."
#
# GOAL UNDER TRIAGE:
#   move-the-governed-repo-roster-out-of-framework-a8f57a50
#
# WHAT THIS HARNESS IS
# --------------------
# Issue #4455 is a ONE-TIME AGENTIC triage action, not a code feature. Its
# locked requirements add ZERO source code (design spec: files_to_change=[],
# new_files=[], test_files=[]). The framework seam that *triggers* the triage
# (`overseer::act_escalate_blocked_goal`) and the playbook asset
# (`prompt_assets/simard/overseer/escalation_triage.md`) are already covered by
# `src/overseer/tests_escalation_triage.rs`. Those are intentionally NOT
# re-tested here.
#
# What has NO test yet — and what this task must actually produce — is the
# OBSERVABLE EVIDENCE of the triage RUN itself, per the escalation_triage.md
# OUTPUT contract and the locked A1–A4 requirements:
#
#   1. the playbook OUTPUT JSON (problem/next_step/root_cause/decision/
#      action_taken/escalate) in plain English with ZERO raw markers;
#   2. exactly ONE jargon-free Signal message per triage stage (3 total);
#   3. the delimited, machine-checkable done-criteria block the rewrite writes
#      into the goal's GitHub tracking issue body;
#   4. verify-then-decide ORDERING: a read-only `gh` merged-PR probe that runs
#      BEFORE any write (the A1 determinism gate + command-injection guard);
#   5. the hard scope boundary: the migration seams are NOT touched and the
#      roster->identity migration itself is NOT implemented by this task.
#
# So the agentic triage run MUST emit a structured evidence artifact that this
# harness verifies. This is the contract for that artifact.
#
# EVIDENCE ARTIFACT CONTRACT (the "implementation" that makes this go GREEN)
# -------------------------------------------------------------------------
#   $TRIAGE_ARTIFACT_DIR   (default: target/triage-artifacts/<goal_id>)
#     ├── output.json        playbook OUTPUT JSON (the 6-field contract object)
#     ├── signal-messages.jsonl
#     │                      one JSON object per line, one per triage stage:
#     │                        {"stage":"restate|verify-decide|act","message":"..."}
#     ├── criteria-block.md  the EXACT delimited block written to the issue body,
#     │                      fenced by:
#     │                        <!-- SIMARD:done-criteria:start -->
#     │                        ... four machine-checkable criteria ...
#     │                        <!-- SIMARD:done-criteria:end -->
#     └── action-log.jsonl   ordered gh action log, one JSON object per line:
#                              {"seq":N,"mode":"read|write","argv":["gh",...],
#                               "target_issue":"<n>","validated":true,"ts":"..."}
#
# `target/` is git-ignored, so the artifact never gets committed. Override the
# location with TRIAGE_ARTIFACT_DIR for a hermetic/CI run.
#
# RED-FIRST EXPECTATION
# ---------------------
# Before the triage runs, the artifact does not exist, so every artifact-backed
# test (Groups A–E) FAILS. The documentation-deliverable tests (Group F) and the
# scope-boundary tests (Group G) reflect work already staged on this branch and
# may PASS immediately — that is fine and intended; they lock the contract.
#
# The harness runs ALL tests (never stops at the first failure) and exits
# non-zero if any test fails, so CI and humans see the full RED/GREEN picture.
#
# USAGE
#   scripts/verify-escalation-triage.sh
#   TRIAGE_ARTIFACT_DIR=/tmp/run scripts/verify-escalation-triage.sh
#   EXPECTED_DECISION=complete-delivered-goal scripts/verify-escalation-triage.sh
#   ISSUE_NUMBER=4455 BASE_REF=origin/main scripts/verify-escalation-triage.sh
#   CHECK_LIVE_ISSUE=1 scripts/verify-escalation-triage.sh   # also cross-check via gh
#
# ENV
#   GOAL_ID              default: move-the-governed-repo-roster-out-of-framework-a8f57a50
#   ISSUE_NUMBER         the goal's tracking-issue number (default: 4455)
#   TRIAGE_ARTIFACT_DIR  evidence dir (default: target/triage-artifacts/<goal_id>)
#   EXPECTED_DECISION    default: rewrite-done-gate (the expected branch here)
#   BASE_REF             base ref for scope diff (default: origin/main)
#   CHECK_LIVE_ISSUE     set to 1 to also assert the block is live on the issue via gh
# ============================================================================

set -uo pipefail

# --- locate repo root --------------------------------------------------------
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || {
  echo "ERROR: not inside a git repository" >&2
  exit 2
}
cd "$REPO_ROOT" || exit 2

GOAL_ID="${GOAL_ID:-move-the-governed-repo-roster-out-of-framework-a8f57a50}"
ISSUE_NUMBER="${ISSUE_NUMBER:-4455}"
TRIAGE_ARTIFACT_DIR="${TRIAGE_ARTIFACT_DIR:-target/triage-artifacts/$GOAL_ID}"
EXPECTED_DECISION="${EXPECTED_DECISION:-rewrite-done-gate}"
BASE_REF="${BASE_REF:-origin/main}"
CHECK_LIVE_ISSUE="${CHECK_LIVE_ISSUE:-0}"

OUTPUT_JSON="$TRIAGE_ARTIFACT_DIR/output.json"
SIGNAL_LOG="$TRIAGE_ARTIFACT_DIR/signal-messages.jsonl"
CRITERIA_BLOCK="$TRIAGE_ARTIFACT_DIR/criteria-block.md"
ACTION_LOG="$TRIAGE_ARTIFACT_DIR/action-log.jsonl"

# Raw operator-facing markers that must NEVER leak. Kept byte-identical to
# JARGON_TOKENS in src/overseer/tests_escalation_triage.rs, extended with the
# specific diagnostic vocabulary and cycle ids from THIS goal's WHY string.
FORBIDDEN_MARKERS=(
  'OODA-SAFEGUARD'
  'UNCLEAR-CRITERIA'
  'GENUINELY-STUCK'
  'evidence=\['
  'why='
  $'\xF0\x9F\x94\x92'                              # the 🔒 lock marker
  'health-review:blocked-goal-unclear-criteria'   # the raw reason marker
  'no-progress breaker'
  'guided-retry'
  'no-worktree'
  'no-investigation'
  'cycles?[[:space:]]*[0-9]{4}'                    # cycle ids e.g. "cycles 2407"
  '2407|2410|2417|2420'                            # the exact cycle numbers
)

# Join all markers into ONE alternation so a scan is a single grep pass instead
# of one subprocess per pattern. OR-of-alternatives is identical to "any hit".
FORBIDDEN_RE="$(IFS='|'; printf '%s' "${FORBIDDEN_MARKERS[*]}")"

# --- result tracking ---------------------------------------------------------
FAILURES=0
PASSES=0
section() { printf '\n=== %s ===\n' "$1"; }
pass()    { PASSES=$((PASSES + 1));    printf '  PASS  %s\n' "$1"; }
fail()    { FAILURES=$((FAILURES + 1)); printf '  FAIL  %s\n' "$1"; }
info()    { printf '        %s\n' "$1"; }

have_jq() { command -v jq >/dev/null 2>&1; }

# Assert a blob of text is free of every forbidden operator-facing marker.
assert_no_markers() { # $1 text  $2 context
  local text="$1" ctx="$2" hit
  # Single grep pass over the combined alternation; -o reports the actual leaked
  # text (better diagnostic than echoing the pattern), -m1 stops at first hit.
  hit="$(printf '%s' "$text" | grep -m1 -oiE "$FORBIDDEN_RE")"
  if [ -z "$hit" ]; then
    pass "$ctx is free of raw diagnostic markers"
  else
    fail "$ctx leaked a raw diagnostic marker: /$hit/"
  fi
}

# =============================================================================
# GROUP A — playbook OUTPUT JSON contract  (escalation_triage.md ## OUTPUT)
# =============================================================================
section "A  OUTPUT JSON contract ($OUTPUT_JSON)"
if [ ! -s "$OUTPUT_JSON" ]; then
  fail "A1 output.json missing — triage has not emitted its OUTPUT contract yet"
  info "expected at: $OUTPUT_JSON"
elif ! have_jq; then
  fail "A1 jq not available to validate output.json"
elif ! jq -e . "$OUTPUT_JSON" >/dev/null 2>&1; then
  fail "A1 output.json is not valid JSON"
else
  pass "A1 output.json exists and is valid JSON"

  # A2 — exactly the six contract keys, no more, no fewer.
  keys="$(jq -r 'keys_unsorted | sort | join(",")' "$OUTPUT_JSON")"
  want="action_taken,decision,escalate,next_step,problem,root_cause"
  if [ "$keys" = "$want" ]; then
    pass "A2 OUTPUT has exactly the 6 contract keys"
  else
    fail "A2 OUTPUT keys mismatch — got [$keys], want [$want]"
  fi

  # A3 — decision is one of the three legal branches AND matches the expected one.
  decision="$(jq -r '.decision // ""' "$OUTPUT_JSON")"
  case "$decision" in
    rewrite-done-gate|complete-delivered-goal|ask-operator-one-question)
      pass "A3a decision is a legal branch ($decision)" ;;
    *)
      fail "A3a decision is not a legal branch: '$decision'" ;;
  esac
  if [ "$decision" = "$EXPECTED_DECISION" ]; then
    pass "A3b decision matches the expected branch ($EXPECTED_DECISION)"
  else
    fail "A3b decision '$decision' != expected '$EXPECTED_DECISION'"
    info "override with EXPECTED_DECISION= if the probe legitimately found a merged PR"
  fi

  # A4 — escalate is null unless the decision is ask-operator-one-question.
  escalate="$(jq -r '.escalate' "$OUTPUT_JSON")"
  if [ "$decision" = "ask-operator-one-question" ]; then
    if [ "$escalate" != "null" ] && [ -n "$escalate" ]; then
      pass "A4 escalate carries the single operator question"
    else
      fail "A4 ask-operator branch must set escalate to one plain-English question"
    fi
  else
    if [ "$escalate" = "null" ]; then
      pass "A4 escalate is null (course-corrected without paging a human)"
    else
      fail "A4 escalate must be null for a self-course-corrected branch, got: '$escalate'"
    fi
  fi

  # A5 — every operator-facing string field is plain English (no raw markers).
  op_fields="$(jq -r '[.problem, .next_step, .root_cause, .action_taken, (.escalate // "")] | join("\n")' "$OUTPUT_JSON")"
  assert_no_markers "$op_fields" "A5 OUTPUT operator fields"

  # A5b — the plain-English fields are actually populated.
  empty=""
  for f in problem next_step root_cause action_taken; do
    v="$(jq -r ".$f // \"\"" "$OUTPUT_JSON")"
    [ -z "$v" ] && empty="$empty $f"
  done
  if [ -z "$empty" ]; then
    pass "A5b problem/next_step/root_cause/action_taken are all populated"
  else
    fail "A5b empty required OUTPUT field(s):$empty"
  fi
fi

# =============================================================================
# GROUP B — Signal cadence: exactly ONE jargon-free message per triage stage
# =============================================================================
section "B  Signal messages ($SIGNAL_LOG)"
if [ ! -s "$SIGNAL_LOG" ]; then
  fail "B1 signal-messages.jsonl missing — no per-stage Signal messages recorded"
  info "expected at: $SIGNAL_LOG"
elif ! have_jq; then
  fail "B1 jq not available to validate signal-messages.jsonl"
else
  # B1 — exactly three messages (one per RESTATE / VERIFY-DECIDE / ACT stage).
  n="$(grep -c . "$SIGNAL_LOG")"
  if [ "$n" -eq 3 ]; then
    pass "B1 exactly 3 Signal messages (one per triage stage)"
  else
    fail "B1 expected 3 Signal messages (one per stage), got $n"
  fi

  # B2 — the three stages are present and distinct.
  stages="$(jq -r '.stage' "$SIGNAL_LOG" 2>/dev/null | sort -u | paste -sd, -)"
  if [ "$stages" = "act,restate,verify-decide" ]; then
    pass "B2 all three stages present (restate, verify-decide, act)"
  else
    fail "B2 stages must be exactly restate/verify-decide/act, got [$stages]"
  fi

  # B3 — every message is non-empty plain English, free of raw markers.
  all_msgs="$(jq -r '.message // ""' "$SIGNAL_LOG" 2>/dev/null)"
  if [ -n "$(printf '%s' "$all_msgs" | tr -d '[:space:]')" ]; then
    pass "B3a every Signal message is non-empty"
  else
    fail "B3a Signal messages are empty"
  fi
  assert_no_markers "$all_msgs" "B3b Signal messages"
fi

# =============================================================================
# GROUP C — machine-checkable done-criteria block written into the issue body
# =============================================================================
section "C  Done-criteria block ($CRITERIA_BLOCK)"
START='<!-- SIMARD:done-criteria:start -->'
END='<!-- SIMARD:done-criteria:end -->'
if [ ! -s "$CRITERIA_BLOCK" ]; then
  fail "C1 criteria-block.md missing — the rewrite has not produced a criteria block"
  info "expected at: $CRITERIA_BLOCK"
else
  block="$(cat "$CRITERIA_BLOCK")"

  # C1 — stable HTML-comment delimiters so re-runs overwrite idempotently.
  if printf '%s' "$block" | grep -qF "$START" && printf '%s' "$block" | grep -qF "$END"; then
    pass "C1 block is fenced by stable SIMARD:done-criteria delimiters"
  else
    fail "C1 block missing the SIMARD:done-criteria start/end delimiters"
  fi

  # C2 — all FOUR machine-checkable criteria are present (A3 of the locked reqs).
  declare -a CRIT_LABEL CRIT_PAT
  CRIT_LABEL=(
    "roster seeded from the identity file"
    "roster persisted as self-deploy-safe identity state"
    "committed ecosystem_repos.toml wiring removed"
    "certified by exactly one merged PR"
  )
  CRIT_PAT=(
    'identit(y|ies).*(seed|file)|seed.*identit'
    'self[- ]deploy.*(preserve|not overwrite|does not overwrite|safe)|identity state'
    'ecosystem_repos\.toml'
    'merged[- ]PR|PR[[:space:]]*#?[0-9].*MERGED|exactly one merged|one merged PR'
  )
  for i in "${!CRIT_PAT[@]}"; do
    if printf '%s' "$block" | grep -qiE "${CRIT_PAT[$i]}"; then
      pass "C2.$((i+1)) criterion present: ${CRIT_LABEL[$i]}"
    else
      fail "C2.$((i+1)) criterion MISSING: ${CRIT_LABEL[$i]}"
    fi
  done

  # C3 — the criteria are machine-checkable (name an observable: PR/issue state,
  #      a file, or a command), not prose-only.
  if printf '%s' "$block" | grep -qiE 'MERGED|CLOSED|\.toml|\.rs|file|command|grep|absent|present'; then
    pass "C3 criteria reference observable machine-checkable signals"
  else
    fail "C3 criteria are prose-only — no machine-checkable observable named"
  fi

  # C4 — the block itself must not leak raw diagnostic markers.
  assert_no_markers "$block" "C4 criteria block"
fi

# Optional live cross-check: the block is actually on the tracking issue body.
if [ "$CHECK_LIVE_ISSUE" = "1" ]; then
  section "C-live  Criteria block is live on issue #$ISSUE_NUMBER"
  if ! command -v gh >/dev/null 2>&1; then
    fail "C-live gh not available"
  elif ! gh auth status >/dev/null 2>&1; then
    fail "C-live gh not authenticated"
  else
    body="$(gh issue view "$ISSUE_NUMBER" --json body --jq .body 2>/dev/null)"
    if printf '%s' "$body" | grep -qF "$START"; then
      pass "C-live issue #$ISSUE_NUMBER body carries the SIMARD:done-criteria block"
    else
      fail "C-live issue #$ISSUE_NUMBER body does NOT carry the criteria block yet"
    fi
  fi
fi

# =============================================================================
# GROUP D — verify-then-decide ORDERING + read-only-probe-before-write invariant
#           (the A1 determinism gate and the command-injection guard)
# =============================================================================
section "D  Verify-then-decide ordering ($ACTION_LOG)"
if [ ! -s "$ACTION_LOG" ]; then
  fail "D1 action-log.jsonl missing — no gh probe/ordering evidence recorded"
  info "expected at: $ACTION_LOG"
elif ! have_jq; then
  fail "D1 jq not available to validate action-log.jsonl"
else
  # D1 — a read-only merged-PR probe is recorded.
  if jq -e 'select(.mode=="read")' "$ACTION_LOG" >/dev/null 2>&1; then
    pass "D1 a read-only gh probe is recorded"
  else
    fail "D1 no read-only gh probe recorded (A1 gate never ran)"
  fi

  # D2 — every read precedes every write (probe-before-write ordering).
  first_write="$(jq -s '[.[] | select(.mode=="write") | .seq] | min // empty' "$ACTION_LOG" 2>/dev/null)"
  last_read="$(jq -s '[.[] | select(.mode=="read") | .seq] | max // empty' "$ACTION_LOG" 2>/dev/null)"
  if [ -z "$first_write" ]; then
    pass "D2 no write action recorded (read-only outcome) — ordering trivially holds"
  elif [ -n "$last_read" ] && [ "$last_read" -lt "$first_write" ]; then
    pass "D2 every read-only probe precedes the first write (probe-before-write)"
  else
    fail "D2 a write (seq=$first_write) is not preceded by the probe (last read seq=${last_read:-none})"
  fi

  # D3 — every gh invocation is an argv ARRAY (command-injection guard), never a
  #      shell string.
  if jq -e 'select((.argv | type) != "array")' "$ACTION_LOG" >/dev/null 2>&1; then
    fail "D3 an action has a non-array argv (shell-string injection risk)"
  else
    pass "D3 all gh invocations are argv arrays (no shell interpolation)"
  fi

  # D4 — goal id + issue number were allowlist-validated before any call.
  if jq -e 'select(.validated != true)' "$ACTION_LOG" >/dev/null 2>&1; then
    fail "D4 an action ran without allowlist validation of goal id / issue number"
  else
    pass "D4 every action validated inputs against the allowlist first"
  fi

  # D5 — any write targets ONLY the scope-bound tracking issue.
  bad_target="$(jq -r --arg n "$ISSUE_NUMBER" 'select(.mode=="write" and (.target_issue|tostring) != $n) | .target_issue' "$ACTION_LOG" 2>/dev/null | head -1)"
  if [ -z "$bad_target" ]; then
    pass "D5 writes are scope-bound to issue #$ISSUE_NUMBER only"
  else
    fail "D5 a write targeted a different issue (#$bad_target) than #$ISSUE_NUMBER"
  fi
fi

# =============================================================================
# GROUP E — no raw markers anywhere in the artifact (belt-and-suspenders leak scan)
# =============================================================================
section "E  Whole-artifact marker-leak scan ($TRIAGE_ARTIFACT_DIR)"
if [ ! -d "$TRIAGE_ARTIFACT_DIR" ]; then
  fail "E1 artifact dir missing — nothing to scan"
else
  scan="$(cat "$OUTPUT_JSON" "$SIGNAL_LOG" "$CRITERIA_BLOCK" 2>/dev/null)"
  assert_no_markers "$scan" "E1 combined operator-facing artifact"
fi

# =============================================================================
# GROUP F — documentation deliverable (already staged on this branch → GREEN)
# =============================================================================
section "F  Documentation deliverable"
REF_DOC="docs/reference/escalation-triage-decision-pipeline.md"
HOW_DOC="docs/howto/triage-a-blocked-goal-with-unclear-finish-criteria.md"

for d in "$REF_DOC" "$HOW_DOC"; do
  if [ -s "$d" ]; then pass "F1 doc present: $d"; else fail "F1 doc missing: $d"; fi
done

if [ -f mkdocs.yml ]; then
  for d in "$REF_DOC" "$HOW_DOC"; do
    rel="${d#docs/}"
    if grep -qF "$rel" mkdocs.yml; then
      pass "F2 nav references $rel"
    else
      fail "F2 nav missing entry for $rel"
    fi
  done
else
  fail "F2 mkdocs.yml not found"
fi

# F3 — the reference doc no longer OVERCLAIMS code-enforced determinism.
if [ -f "$REF_DOC" ]; then
  if grep -qiE 'binding contract for (the|a) deterministic (decision )?pipeline' "$REF_DOC"; then
    fail "F3 reference doc still overclaims a 'binding contract for the deterministic pipeline'"
  else
    pass "F3 reference doc dropped the code-enforced-determinism overclaim"
  fi
  if grep -qiE 'requirements-level contract' "$REF_DOC"; then
    pass "F3b reference doc frames itself as a requirements-level contract"
  else
    fail "F3b reference doc must frame itself as a requirements-level contract"
  fi
fi

# =============================================================================
# GROUP G — hard scope boundary (locked requirement #5)
# =============================================================================
section "G  Scope boundary vs $BASE_REF"
if ! git rev-parse --verify --quiet "$BASE_REF" >/dev/null; then
  git fetch --quiet origin main 2>/dev/null || true
fi
if git rev-parse --verify --quiet "$BASE_REF" >/dev/null; then
  CHANGED="$(git diff --name-only "$BASE_REF"...HEAD 2>/dev/null)"

  # G1 — the migration seams must NOT be touched by this task.
  SEAMS='src/overseer/ecosystem_observe.rs|src/overseer/wiring.rs|src/overseer/mod.rs'
  if printf '%s\n' "$CHANGED" | grep -qE "$SEAMS"; then
    # Touching a seam file is only a violation if the seam FUNCTIONS changed.
    if git diff "$BASE_REF"...HEAD -- src/overseer/ 2>/dev/null \
        | grep -qE '^[+-].*(resolve_ecosystem_roster_path|build_ecosystem_observer|act_escalate_blocked_goal)'; then
      fail "G1 a protected migration seam function was modified"
    else
      pass "G1 no protected seam function (resolve_ecosystem_roster_path/build_ecosystem_observer/act_escalate_blocked_goal) modified"
    fi
  else
    pass "G1 no protected seam file touched"
  fi

  # G2 — the roster->identity MIGRATION itself is NOT implemented by this task:
  #      the committed ecosystem_repos.toml must still be present (removing it is
  #      the migration's job, out of scope for this triage).
  if [ -f prompt_assets/simard/ecosystem_repos.toml ]; then
    pass "G2 ecosystem_repos.toml still present — migration not (wrongly) implemented here"
  else
    fail "G2 ecosystem_repos.toml was removed — this task must NOT implement the migration"
  fi

  # G3 — every changed path is within the additive allowlist for this triage.
  OUT_OF_SCOPE=()
  while IFS= read -r p; do
    [ -z "$p" ] && continue
    case "$p" in
      docs/*) ;;
      mkdocs.yml) ;;
      scripts/verify-escalation-triage.sh) ;;
      *) OUT_OF_SCOPE+=("$p") ;;
    esac
  done < <(printf '%s\n' "$CHANGED")
  if [ "${#OUT_OF_SCOPE[@]}" -eq 0 ]; then
    pass "G3 all changes are additive/in-scope (docs + this harness)"
  else
    fail "G3 out-of-scope change(s) — this triage adds no source code:"
    for p in "${OUT_OF_SCOPE[@]}"; do info "$p"; done
  fi
else
  info "SKIP G: base ref $BASE_REF unavailable"
fi

# =============================================================================
# Summary
# =============================================================================
section "Summary"
printf '  %d passed, %d failed\n' "$PASSES" "$FAILURES"
if [ "$FAILURES" -gt 0 ]; then
  printf '\nESCALATION-TRIAGE VERIFICATION FAILED (%d check[s]).\n' "$FAILURES"
  printf 'RED is expected until the triage run emits its evidence artifact under:\n  %s\n' "$TRIAGE_ARTIFACT_DIR"
  exit 1
fi
printf '\nESCALATION-TRIAGE VERIFICATION PASSED.\n'
exit 0
