#!/usr/bin/env bash
# Outside-in scenario for issue #2432 — the engineer-lifecycle brain's
# confidence-gated escalation ladder (schema-repair + tier bump) that replaces
# the zero-retry parse→default introduced for the #2419 family.
#
# What this proves, without an LLM, at the recipe-runner boundary:
#
#   (a) The production recipe `ooda-engineer-lifecycle.yaml` exposes the
#       `{{escalation_note}}` placeholder the brain injects on each rung, and
#       documents the "empty on the base attempt" contract.
#
#   (b) recipe-runner-rs context-var substitution honours that contract:
#       - base attempt (escalation_note empty)  → the note text is ABSENT, so
#         the rendered prompt is the original base prompt (byte-identical
#         behaviour to before the change);
#       - escalation rung (escalation_note set) → the pinned schema-repair /
#         high-effort instruction is injected ahead of the ROLE section.
#       This is exactly the seam `RecipeBrain` drives one rung at a time.
#
#   (c) The in-tree Rust ladder is exercised end-to-end via `cargo test`
#       (issue_2432_tests): schema-repair recovery, escalation to the second
#       rung, bounded-cap exhaustion → deterministic default, disabled config,
#       invoke-error fallback, prior-output feed-through, content-pinned note
#       wording, and config clamp/bound. The #2419 metric/outcome tests
#       (issue_2419_tests) are re-run to prove the parse-failure signal is
#       preserved and not regressed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if ! command -v recipe-runner-rs >/dev/null 2>&1; then
  echo "SKIP: recipe-runner-rs not on PATH (required for this scenario)" >&2
  exit 0
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "SKIP: jq not on PATH (required for this scenario)" >&2
  exit 0
fi

RECIPE="prompt_assets/simard/recipes/ooda-engineer-lifecycle.yaml"

# --- (a) production recipe exposes the placeholder + documents the contract ---
echo "== (a) recipe exposes {{escalation_note}} placeholder + base contract =="
grep -qF '{{escalation_note}}' "$RECIPE" \
  || { echo "FAIL: $RECIPE is missing the {{escalation_note}} placeholder" >&2; exit 1; }
# The base attempt must render the note to nothing — documented in the recipe.
grep -qiE 'escalation_note.*empty on the base attempt|empty on the base attempt' "$RECIPE" \
  || { echo "FAIL: $RECIPE does not document the empty-on-base escalation_note contract" >&2; exit 1; }
echo "OK: placeholder present and base contract documented."

WORK="$(mktemp -d /tmp/simard-brain-escalation-2432.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

# --- (b) deterministic substitution proof (no LLM) ---------------------------
# A bash-step fixture that embeds the SAME placeholder the production prompt
# uses. We render it twice through recipe-runner-rs and assert the contract.
echo "== (b) {{escalation_note}} substitution contract (base vs escalation) =="
FIX="$WORK/escalation-note-fixture.yaml"
cat > "$FIX" <<'EOF'
name: "escalation-note-fixture-2432"
description: "deterministic escalation_note substitution proof (no LLM)"
version: "1.0.0"
context:
  escalation_note: ""
steps:
  - id: "render-prompt"
    type: "bash"
    command: |
      cat <<'BODY'
      {{escalation_note}}
      ## ROLE
      You are the brain of Simard's OODA daemon.
      BODY
    output: "rendered"
EOF

SENTINEL="SCHEMA REPAIR (retry)"

# base attempt: production passes an explicit empty `-c escalation_note=` on the
# base rung, so mirror that here (not just the fixture default) to prove the
# placeholder substitution path itself renders the base prompt cleanly.
BASE_OUT="$(recipe-runner-rs "$FIX" -c escalation_note= --output-format json 2>/dev/null \
  | jq -r '.step_results | last | .output')"
printf 'base render:\n%s\n' "$BASE_OUT"
if printf '%s' "$BASE_OUT" | grep -qF "$SENTINEL"; then
  echo "FAIL: base attempt unexpectedly injected an escalation note" >&2
  exit 1
fi
# Guard against a false pass where substitution silently no-ops and leaves the
# literal `{{escalation_note}}` token in the prompt: that must NEVER appear.
if printf '%s' "$BASE_OUT" | grep -qF '{{escalation_note}}'; then
  echo "FAIL: base render left the literal {{escalation_note}} placeholder (substitution broken)" >&2
  exit 1
fi
printf '%s' "$BASE_OUT" | grep -qF '## ROLE' \
  || { echo "FAIL: base render lost the ROLE section" >&2; exit 1; }
echo "OK: base attempt renders the original prompt with NO escalation note."

# escalation rung: escalation_note carries the pinned schema-repair instruction
NOTE="## ⚠️ ${SENTINEL} ## Your previous response could not be parsed: its FIRST WORD was not a valid decision variant. Respond again now. The VERY FIRST WORD of your reply MUST be exactly one of: continue_skipping, reclaim_and_redispatch, deprioritize, open_tracking_issue, mark_goal_blocked, consider_self_update."
ESC_OUT="$(recipe-runner-rs "$FIX" -c escalation_note="$NOTE" --output-format json 2>/dev/null \
  | jq -r '.step_results | last | .output')"
printf 'escalation render:\n%s\n' "$ESC_OUT"
printf '%s' "$ESC_OUT" | grep -qF "$SENTINEL" \
  || { echo "FAIL: escalation rung did not inject the schema-repair note" >&2; exit 1; }
printf '%s' "$ESC_OUT" | grep -qF 'continue_skipping' \
  || { echo "FAIL: escalation note is missing the variant allow-list" >&2; exit 1; }
# The injected note must come BEFORE the ROLE section (it re-frames the task).
NOTE_LINE="$(printf '%s\n' "$ESC_OUT" | grep -nF "$SENTINEL" | head -1 | cut -d: -f1)"
ROLE_LINE="$(printf '%s\n' "$ESC_OUT" | grep -nF '## ROLE'   | head -1 | cut -d: -f1)"
[ -n "$NOTE_LINE" ] && [ -n "$ROLE_LINE" ] && [ "$NOTE_LINE" -lt "$ROLE_LINE" ] \
  || { echo "FAIL: escalation note must be injected ahead of the ROLE section" >&2; exit 1; }
echo "OK: escalation rung injects the schema-repair instruction ahead of ROLE."

# --- (c) in-tree Rust ladder + preserved #2419 metric outcomes ---------------
echo "== (c) in-tree ladder unit tests (issue_2432_tests + issue_2419_tests) =="
# Capture the cargo test output so we can assert tests ACTUALLY ran. A cargo
# filter that matches zero tests still exits 0, so a future rename of the
# `issue_24*` modules must not silently turn this rung into a no-op.
TEST_LOG="$WORK/issue24-cargo-test.log"
cargo test --lib --locked issue_24 -- --nocapture >"$TEST_LOG" 2>&1
PASSED="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | grep -oE '[0-9]+' | head -1)"
PASSED="${PASSED:-0}"
echo "issue_24 tests passed: ${PASSED}"
[ "$PASSED" -ge 1 ] \
  || { echo "FAIL: issue_24 filter matched zero tests — module rename silently no-op'd this rung" >&2; cat "$TEST_LOG" >&2; exit 1; }
# Pin two representative tests by name so the ladder + the #2419 metric path are
# both genuinely covered (not just *some* unrelated issue_24* test).
grep -qF 'issue_2432_tests::ladder_recovers_via_schema_repair' "$TEST_LOG" \
  || { echo "FAIL: the escalation-ladder recovery test did not run" >&2; exit 1; }
grep -qF 'issue_2419_tests::' "$TEST_LOG" \
  || { echo "FAIL: the #2419 metric-outcome tests did not run (parse-failure signal coverage lost)" >&2; exit 1; }
echo "OK: escalation-ladder logic + preserved parse-failure metric outcomes pass (${PASSED} tests)."

echo "PASS: brain-escalation-ladder scenario (issue #2432)"
