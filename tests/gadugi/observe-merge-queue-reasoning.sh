#!/usr/bin/env bash
# Outside-in scenario for the agentic observe/orient merge-queue + issue
# reasoning fix (#4097).
#
# ROOT CAUSE (verified on origin/main 923fb8db): the Overseer's observe/orient
# stage populated `ObservedState.ready_prs` from a single imperative allowlist
# sensor — `survey_ready_prs(&automerge_repos())`. With `SIMARD_AUTOMERGE_REPOS`
# / `SIMARD_AUTOMERGE_AUTHOR` UNSET in production, the allowlist was empty, the
# sensor returned nothing, and the Overseer reasoned about ZERO open PRs while a
# CI-green merge queue piled up. Unset SILENTLY meant OFF.
#
# THE FIX (validated here outside-in, no LLM): merge-queue + issue REASONING is
# an agentic recipe behind a THIN deterministic rail. Reasoning is default-ON
# over the governed roster even when SIMARD_AUTOMERGE_* are unset; merge
# AUTHORIZATION stays narrow behind objective gates + the agentic MergeJudge.
#
# This scenario:
#   (a) proves the deterministic recipe-runner SEAM the rail depends on: a
#       bash-step fixture emits a known merge-queue BRIEF and json mode surfaces
#       it as the final step output (what `SpawnMergeQueueRecipeRunner` forwards
#       verbatim to `parse_merge_queue_brief`);
#   (b) validates the fix + every safety invariant via the in-tree contract +
#       wiring unit tests, asserting the critical test names actually ran.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# ── (a) deterministic recipe-runner SEAM (no LLM) ────────────────────────────
# The production rail (`SpawnMergeQueueRecipeRunner`) runs recipe-runner-rs in
# `--output-format json` and forwards the final step output VERBATIM. Prove that
# boundary carries a merge-queue brief intact using a deterministic bash step.
if command -v recipe-runner-rs >/dev/null 2>&1 && command -v jq >/dev/null 2>&1; then
  WORK="$(mktemp -d /tmp/simard-observe-merge-queue.XXXXXX)"
  trap 'rm -rf "$WORK"' EXIT

  BRIEF_JSON='{"reasoned_prs":[{"repo":"rysweet/Simard","pr":4123,"disposition":"ready-for-merge","rationale":"CI green, MERGEABLE","duplicate_of":null},{"repo":"rysweet/Simard","pr":4200,"disposition":"duplicate","rationale":"same fix as #4123","duplicate_of":4123}],"triaged_issues":[{"repo":"rysweet/Simard","issue":4097,"priority":"high","readiness":"ready","next_action":"start a workstream"}]}'
  RECIPE="$WORK/observe-merge-queue-fixture.yaml"
  cat > "$RECIPE" <<EOF
name: "observe-merge-queue-fixture-4097"
description: "deterministic merge-queue brief fixture (no LLM)"
version: "1.0.0"
context: {}
steps:
  - id: "brief"
    type: "bash"
    command: |
      echo '${BRIEF_JSON}'
    output: "merge_queue_brief"
EOF

  echo "== (a) recipe-runner json mode surfaces the merge-queue brief =="
  JSON_OUT="$(recipe-runner-rs "$RECIPE" --output-format json 2>/dev/null)"
  printf '%s' "$JSON_OUT" | jq -e '.success == true' >/dev/null \
    || { echo "FAIL: fixture recipe did not report success in json mode" >&2; exit 1; }
  STEP_OUTPUT="$(printf '%s' "$JSON_OUT" | jq -r '.step_results | last | .output')"
  echo "json-mode final step output: ${STEP_OUTPUT}"
  # The brief must survive intact: a ready-for-merge PROPOSAL and a duplicate.
  printf '%s' "$STEP_OUTPUT" | jq -e '.reasoned_prs | length == 2' >/dev/null \
    || { echo "FAIL: brief lost its reasoned_prs across the seam" >&2; exit 1; }
  printf '%s' "$STEP_OUTPUT" | jq -e '.reasoned_prs[0].disposition == "ready-for-merge"' >/dev/null \
    || { echo "FAIL: ready-for-merge proposal missing from the forwarded brief" >&2; exit 1; }
  printf '%s' "$STEP_OUTPUT" | jq -e '.triaged_issues[0].priority == "high"' >/dev/null \
    || { echo "FAIL: triaged issue missing from the forwarded brief" >&2; exit 1; }
  echo "OK: the recipe-runner seam forwards a parseable merge-queue brief verbatim."
else
  echo "SKIP (a): recipe-runner-rs and/or jq not on PATH — seam proof skipped, in-tree tests still run" >&2
fi

# ── (b) in-tree contract + wiring tests (the fix + every safety invariant) ────
echo "== (b) in-tree observe-merge-queue reasoning tests =="
TEST_LOG="$(mktemp /tmp/simard-observe-merge-queue-tests.XXXXXX.log)"
# Both the contract module and the run_cycle wiring tests share the
# `merge_queue`/`tests_merge_queue_reasoning` substring.
cargo test --lib --locked merge_queue -- --nocapture >"$TEST_LOG" 2>&1 \
  || { echo "FAIL: observe-merge-queue reasoning tests did not pass" >&2; cat "$TEST_LOG" >&2; exit 1; }

PASSED="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | grep -oE '[0-9]+' | paste -sd+ - | bc 2>/dev/null || echo 0)"
echo "merge-queue reasoning tests passed: ${PASSED}"
[ "${PASSED:-0}" -ge 1 ] \
  || { echo "FAIL: the merge_queue filter matched zero tests (module renamed?)" >&2; cat "$TEST_LOG" >&2; exit 1; }

# Assert the CRITICAL invariants actually ran (not just "some tests passed"):
require_test() {
  grep -qF "$1" "$TEST_LOG" \
    || { echo "FAIL: required invariant test did not run: $1" >&2; cat "$TEST_LOG" >&2; exit 1; }
}
# ROOT-CAUSE fix: unset scope is default-ON over the governed roster, not OFF.
require_test 'scope_unset_defaults_on_to_roster'
# Explicit disable is LOUD (R3), never a silent hard-OFF.
require_test 'scope_off_and_falsey_values_disable_loudly'
# The rail fails closed: empty scope never spawns the recipe; a runner error degrades.
require_test 'rail_empty_scope_fails_closed_without_running_recipe'
require_test 'rail_runner_error_degrades_to_none'
# The roster is the trust boundary: off-roster entries are dropped.
require_test 'parse_drops_off_roster_repos_the_trust_boundary'
# A duplicate pointing at itself is incoherent and dropped (fail-closed).
require_test 'parse_drops_duplicate_pointing_at_itself'
# CORE SAFETY INVARIANT: ReadyForMerge reasoning ALONE never authorizes a merge.
require_test 'ready_for_merge_reasoning_alone_does_not_authorize_merge'
# The new interventions can never carry --admin / --no-verify.
require_test 'new_interventions_never_carry_admin_or_no_verify'
# The gap-scan resource opt-out surfaces a VISIBLE Disabled status, not a silent Unknown.
require_test 'merge_queue_reasoning_gap_scan_optout_is_visible_not_silent'
# Default-ON populates reasoned state even with SIMARD_AUTOMERGE_* unset (the dead-wire fix).
require_test 'merge_queue_reasoning_populates_observed_state_default_on_over_roster'
echo "OK: fix + all safety invariants validated in-tree (${PASSED} tests)."

rm -f "$TEST_LOG"
echo "PASS: observe-merge-queue-reasoning scenario (#4097)"
