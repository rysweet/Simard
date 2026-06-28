#!/usr/bin/env bash
# Outside-in scenario for issue #2421 — the decide + orient RecipeBrain phases
# share the #2419 text-vs-json parse bug (orient ACTIVELY corrupts urgency).
#
# Root cause: RecipeBrain::judge_decision / judge_orientation invoke
# recipe-runner-rs in its DEFAULT `text` output mode, which prints only a
# summary banner ("Recipe: <name> ... SUCCESS (0.0s) ...") to stdout. The
# agent's real decision text is exposed ONLY via `--output-format json`
# (step_results[].output). Consequences:
#   - decide: the banner's first word is always "Recipe:", which matches no
#     action keyword, so judge_decision silently returns AdvanceGoal every
#     cycle (the LLM is ignored).
#   - orient: WORSE — parse_orient_from_text scans the banner for the first
#     in-range decimal, and the timing string "(0.0s)" yields 0.0, so the
#     daemon silently DEMOTES the goal's urgency to a value scraped from the
#     banner rather than the LLM's judgment.
#
# This scenario validates the contract at the recipe-runner boundary WITHOUT an
# LLM: deterministic bash-step recipes emit a known action word / urgency
# decimal, and we assert that
#   (a) text mode hides them behind the banner (the bug), and
#   (b) json mode exposes them as the final step output (the fix).
#
# It also exercises the in-tree Rust parser path via `cargo test`
# (issue_2421_tests): banner-misparse pins + JSON-envelope recovery for both
# decide and orient, plus the orient timing-isolation guard.
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

WORK="$(mktemp -d /tmp/simard-decide-orient-2421.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

# ---------------------------------------------------------------------------
# decide: a real action word the text-mode banner hides
# ---------------------------------------------------------------------------
DECIDE_RECIPE="$WORK/decide-fixture.yaml"
ACTION="consolidate_memory context overhead is high this cycle"
cat > "$DECIDE_RECIPE" <<EOF
name: "decide-fixture-2421"
description: "deterministic decide action fixture (no LLM)"
version: "1.0.0"
context: {}
steps:
  - id: "decide-action"
    type: "bash"
    command: |
      echo '${ACTION}'
    output: "decide_result"
EOF

echo "== decide (a) text mode: first word is the banner, not the action =="
DTEXT="$(recipe-runner-rs "$DECIDE_RECIPE" 2>/dev/null)"
DTEXT_FIRST="$(printf '%s\n' "$DTEXT" | awk 'NF{print $1; exit}')"
echo "decide text-mode first word: ${DTEXT_FIRST}"
[ "$DTEXT_FIRST" = "consolidate_memory" ] \
  && { echo "FAIL: text mode unexpectedly exposed the action as the first word" >&2; exit 1; }
printf '%s\n' "$DTEXT" | grep -qE '^Recipe:' \
  || { echo "FAIL: expected a 'Recipe:' banner in decide text mode" >&2; exit 1; }

echo "== decide (b) json mode: final step output first word IS the action =="
DJSON="$(recipe-runner-rs "$DECIDE_RECIPE" --output-format json 2>/dev/null)"
DSTEP="$(printf '%s' "$DJSON" | jq -r '.step_results | last | .output')"
DJSON_FIRST="$(printf '%s\n' "$DSTEP" | awk 'NF{print $1; exit}')"
echo "decide json-mode first word: ${DJSON_FIRST}"
[ "$DJSON_FIRST" = "consolidate_memory" ] \
  || { echo "FAIL: decide json-mode first word is not the expected action" >&2; exit 1; }
echo "OK: decide — text hides the action behind the banner; json exposes it."

# ---------------------------------------------------------------------------
# orient: a real urgency decimal the banner timing "(0.0s)" would corrupt
# ---------------------------------------------------------------------------
ORIENT_RECIPE="$WORK/orient-fixture.yaml"
URGENCY="0.65 goal remains high urgency despite one transient failure"
cat > "$ORIENT_RECIPE" <<EOF
name: "orient-fixture-2421"
description: "deterministic orient urgency fixture (no LLM)"
version: "1.0.0"
context: {}
steps:
  - id: "orient-decision"
    type: "bash"
    command: |
      echo '${URGENCY}'
    output: "orient_result"
EOF

echo "== orient (a) text mode: banner timing '(0.0s)' is the corruption source =="
OTEXT="$(recipe-runner-rs "$ORIENT_RECIPE" 2>/dev/null)"
printf '%s\n' "$OTEXT" | grep -qE '\(0\.0s\)' \
  || { echo "FAIL: expected a '(0.0s)' timing string in the orient banner" >&2; exit 1; }
# The real urgency 0.65 must NOT be the first in-range decimal on the banner —
# the timing 0.0 precedes it, which is exactly what silently demotes urgency.
if printf '%s' "$OTEXT" | grep -qF '0.65'; then
  echo "FAIL: text-mode banner unexpectedly contains the real urgency 0.65" >&2
  exit 1
fi

echo "== orient (b) json mode: final step output IS the real urgency decimal =="
OJSON="$(recipe-runner-rs "$ORIENT_RECIPE" --output-format json 2>/dev/null)"
OSTEP="$(printf '%s' "$OJSON" | jq -r '.step_results | last | .output')"
echo "orient json-mode final step output: ${OSTEP}"
printf '%s' "$OSTEP" | grep -qF '0.65' \
  || { echo "FAIL: orient json-mode output is missing the real urgency 0.65" >&2; exit 1; }
echo "OK: orient — text scrapes 0.0 from timing; json exposes the real 0.65."

# ---------------------------------------------------------------------------
# (c) in-tree Rust parser path: banner pins + JSON-envelope recovery
# ---------------------------------------------------------------------------
echo "== (c) in-tree decide/orient parser unit tests (issue_2421_tests) =="
TEST_LOG="$WORK/issue-2421-cargo-test.log"
cargo test --lib --locked issue_2421_tests -- --nocapture >"$TEST_LOG" 2>&1
PASSED="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | grep -oE '[0-9]+' | head -1)"
PASSED="${PASSED:-0}"
echo "issue_2421 tests passed: ${PASSED}"
[ "$PASSED" -ge 1 ] \
  || { echo "FAIL: issue_2421_tests filter matched zero tests (module renamed?)" >&2; cat "$TEST_LOG" >&2; exit 1; }
grep -qF 'issue_2421_tests::orient_banner_timing_actively_corrupts_urgency' "$TEST_LOG" \
  || { echo "FAIL: the orient urgency-corruption pin did not run" >&2; exit 1; }
grep -qF 'issue_2421_tests::decide_json_envelope_recovers_real_action' "$TEST_LOG" \
  || { echo "FAIL: the decide JSON-envelope recovery test did not run" >&2; exit 1; }
echo "OK: decide/orient banner pins + JSON-envelope recovery pass (${PASSED} tests)."

echo "PASS: decide-orient-brain-parse scenario (issue #2421)"
