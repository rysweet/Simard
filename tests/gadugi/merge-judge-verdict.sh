#!/usr/bin/env bash
# Outside-in scenario for the merge-judge verdict-parse cluster
# (#2428 / #2430 / #2435 / #2462 / #2463) — `simard merge-pr` never surfaces a
# verdict because the recipe-runner-backed judge parses the TEXT-MODE banner.
#
# Root cause (identical to #2419, different surface): RecipeMergeJudge::judge
# invokes recipe-runner-rs in its DEFAULT `text` output mode, which prints only
# a summary banner ("Recipe: merge-readiness-judge ... SUCCESS ...") to stdout.
# The agent's actual {"verdict": "ready"|"not_ready"|"unclear"} JSON is exposed
# ONLY via `--output-format json` (step_results[].output). The keyword/JSON
# scan over the banner therefore finds NO verdict and every gated merge is
# blocked with: "no verdict keyword (ready/not_ready/unclear) found".
#
# This scenario validates the contract at the recipe-runner boundary WITHOUT an
# LLM: a deterministic bash-step recipe emits a known JSON verdict, and we
# assert that
#   (a) text mode hides it behind the banner (the bug — no verdict on stdout),
#   (b) json mode exposes it as the final step output (the fix), parseable as a
#       structured {"verdict": ...} object.
#
# It also exercises the in-tree Rust parse-composition + fail-closed unit tests
# via `cargo test` (issue_2428_tests) so the JSON-envelope verdict extraction,
# prose keyword fallback, and the empty-output fail-closed contract are all
# validated end-to-end.
#
# Steps (d)/(e) extend this to the REAL agent wire format reported in
# #2501 / #2555 (duplicates of the above): the GitHub Copilot CLI launcher
# prints a preamble line (`ℹ … NODE_OPTIONS=… (saved preference)…`) to stdout
# BEFORE the agent answer, so recipe-runner captures it into
# step_results[].output PREPENDED to the verdict JSON. (d) reproduces that exact
# shape deterministically (no LLM) and proves json mode still surfaces a
# parseable `ready`; (e) runs the in-tree production-path regression module
# (issue_2501_2555_real_envelope_tests) that drives the real
# `extract_recipe_decision_output` on the same preamble-bearing envelope.
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

WORK="$(mktemp -d /tmp/simard-merge-judge-verdict.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

RECIPE="$WORK/merge-judge-fixture.yaml"
VERDICT_JSON='{"verdict": "ready", "rationale": "all six skill sections present and substantive"}'

cat > "$RECIPE" <<EOF
name: "merge-judge-fixture-2428"
description: "deterministic merge-readiness verdict fixture (no LLM)"
version: "1.0.0"
context: {}
steps:
  - id: "judge-merge-readiness"
    type: "bash"
    command: |
      echo '${VERDICT_JSON}'
    output: "judge_result"
EOF

# --- (a) text mode reproduces the bug: the banner hides the verdict ----------
echo "== (a) text mode: verdict is hidden behind the SUCCESS banner =="
TEXT_OUT="$(recipe-runner-rs "$RECIPE" 2>/dev/null)"
printf '%s\n' "$TEXT_OUT"
printf '%s\n' "$TEXT_OUT" | grep -qE '^Recipe:' \
  || { echo "FAIL: expected a 'Recipe:' summary banner in text mode" >&2; exit 1; }
# The verdict JSON must NOT appear on text-mode stdout — that is the #2462 bug.
if printf '%s' "$TEXT_OUT" | grep -qF '"verdict"'; then
  echo "FAIL: text mode unexpectedly exposed the verdict JSON" >&2
  exit 1
fi
# And the banner carries no bare verdict keyword (only 'readiness', not 'ready').
if printf '%s' "$TEXT_OUT" | grep -qiwE 'ready|not_ready|unclear'; then
  echo "FAIL: text-mode banner unexpectedly contains a verdict keyword" >&2
  exit 1
fi
echo "OK: text mode surfaces no verdict (reproduces 'no verdict keyword' bug)."

# --- (b) json mode exposes the real verdict as the final step output (fix) ----
echo "== (b) json mode: verdict surfaced as the final step output =="
JSON_OUT="$(recipe-runner-rs "$RECIPE" --output-format json 2>/dev/null)"
printf '%s' "$JSON_OUT" | jq -e '.success == true' >/dev/null \
  || { echo "FAIL: recipe did not report success in json mode" >&2; exit 1; }

STEP_OUTPUT="$(printf '%s' "$JSON_OUT" | jq -r '.step_results | last | .output')"
echo "json-mode final step output: ${STEP_OUTPUT}"
printf '%s' "$STEP_OUTPUT" | grep -qF '"verdict"' \
  || { echo "FAIL: json-mode final step output is missing the verdict JSON" >&2; exit 1; }
VERDICT="$(printf '%s' "$STEP_OUTPUT" | jq -r '.verdict')"
echo "parsed verdict: ${VERDICT}"
[ "$VERDICT" = "ready" ] \
  || { echo "FAIL: json-mode verdict is not the expected 'ready'" >&2; exit 1; }
echo "OK: json mode exposes a parseable structured verdict."

# --- (c) in-tree Rust parse-composition + fail-closed unit tests -------------
echo "== (c) in-tree merge-judge unit tests (issue_2428_tests) =="
TEST_LOG="$WORK/issue-2428-cargo-test.log"
cargo test --lib --locked issue_2428_tests -- --nocapture >"$TEST_LOG" 2>&1
PASSED="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | grep -oE '[0-9]+' | head -1)"
PASSED="${PASSED:-0}"
echo "issue_2428 tests passed: ${PASSED}"
[ "$PASSED" -ge 1 ] \
  || { echo "FAIL: issue_2428_tests filter matched zero tests (module renamed?)" >&2; cat "$TEST_LOG" >&2; exit 1; }
grep -qF 'issue_2428_tests::json_envelope_fenced_verdict_parses_ready' "$TEST_LOG" \
  || { echo "FAIL: the JSON-envelope verdict-extraction test did not run" >&2; exit 1; }
grep -qF 'issue_2428_tests::empty_final_step_output_fails_closed' "$TEST_LOG" \
  || { echo "FAIL: the fail-closed contract test did not run" >&2; exit 1; }
echo "OK: JSON-envelope verdict extraction + fail-closed contract pass (${PASSED} tests)."

# --- (d) REAL agent wire format (#2501/#2555): launcher preamble + verdict -----
# recipe-runner captures the Copilot launcher preamble into step_results[].output
# ahead of the verdict JSON. Reproduce that exact shape (no LLM) and prove the
# verdict is still surfaced and parses to `ready` — the preamble must not shadow
# the real answer, and the merge must NOT abort with "no verdict keyword".
echo "== (d) real agent wire format: launcher preamble + verdict JSON =="
PREAMBLE='ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: /home/azureuser/.amplihack/config'
RECIPE_D="$WORK/merge-judge-preamble.yaml"
cat > "$RECIPE_D" <<EOF
name: "merge-judge-preamble-2501"
description: "deterministic real-wire-format fixture: launcher preamble + verdict (no LLM)"
version: "1.0.0"
context: {}
steps:
  - id: "judge-merge-readiness"
    type: "bash"
    command: |
      printf '%s\n%s\n' '${PREAMBLE}' '${VERDICT_JSON}'
    output: "judge_result"
EOF
JSON_OUT_D="$(recipe-runner-rs "$RECIPE_D" --output-format json 2>/dev/null)"
printf '%s' "$JSON_OUT_D" | jq -e '.success == true' >/dev/null \
  || { echo "FAIL: preamble fixture did not report success in json mode" >&2; exit 1; }
STEP_OUTPUT_D="$(printf '%s' "$JSON_OUT_D" | jq -r '.step_results | last | .output')"
echo "json-mode final step output (with preamble):"
printf '%s\n' "$STEP_OUTPUT_D"
# The captured output must carry BOTH the launcher preamble AND the verdict JSON.
printf '%s' "$STEP_OUTPUT_D" | grep -qF 'NODE_OPTIONS=' \
  || { echo "FAIL: expected the launcher preamble in the captured step output" >&2; exit 1; }
printf '%s' "$STEP_OUTPUT_D" | grep -qF '"verdict"' \
  || { echo "FAIL: verdict JSON missing from the preamble-prefixed step output" >&2; exit 1; }
# The verdict (last line) must still parse to `ready` despite the leading preamble.
VERDICT_D="$(printf '%s\n' "$STEP_OUTPUT_D" | tail -1 | jq -r '.verdict')"
echo "parsed verdict (past preamble): ${VERDICT_D}"
[ "$VERDICT_D" = "ready" ] \
  || { echo "FAIL: verdict past the launcher preamble is not the expected 'ready'" >&2; exit 1; }
echo "OK: json mode surfaces a parseable verdict even behind the launcher preamble."

# --- (e) in-tree production-path regression on the real 0.3.6 envelope ---------
echo "== (e) production extractor on the real 0.3.6 envelope (#2501/#2555) =="
TEST_LOG_E="$WORK/issue-2501-cargo-test.log"
cargo test --lib --locked issue_2501_2555_real_envelope_tests -- --nocapture >"$TEST_LOG_E" 2>&1
PASSED_E="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG_E" | grep -oE '[0-9]+' | head -1)"
PASSED_E="${PASSED_E:-0}"
echo "issue_2501/2555 real-envelope tests passed: ${PASSED_E}"
[ "$PASSED_E" -ge 1 ] \
  || { echo "FAIL: issue_2501_2555_real_envelope_tests matched zero tests (module renamed?)" >&2; cat "$TEST_LOG_E" >&2; exit 1; }
grep -qF 'real_copilot_envelope_recovers_ready_verdict' "$TEST_LOG_E" \
  || { echo "FAIL: the real-envelope ready-verdict test did not run" >&2; exit 1; }
grep -qF 'real_copilot_envelope_preamble_only_fails_closed_never_ready' "$TEST_LOG_E" \
  || { echo "FAIL: the preamble-only fail-closed test did not run" >&2; exit 1; }
echo "OK: production extractor recovers a verdict from the real preamble-bearing envelope."

echo "PASS: merge-judge-verdict scenario (#2428/#2430/#2435/#2462/#2463, #2501/#2555)"
