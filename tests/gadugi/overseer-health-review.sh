#!/usr/bin/env bash
# Outside-in scenario for the Overseer's agentic HEALTH-REVIEW rail ([standing]).
#
# MOTIVATION (docs/concepts/agentic-recipes-first-principle.md): the Overseer
# once failed to self-heal a crash-loop of 7 goals that re-fired the SAME
# actor-binding failure 286+ times. The operator diagnosed it AGENTICALLY in a
# handful of reads — `journalctl --user -u simard-ooda`, `simard status`,
# `simard goal list` — then drove the fix. This rail gives the Overseer that same
# reflex on every tick, WITHOUT the retired anti-pattern of `record_step_failure`
# plumbing or an N-identical-failure THRESHOLD counter in Rust: the journal
# already contains every failure, and an agent reading it sees them all.
#
# THE DESIGN (validated here outside-in, no LLM): a THIN deterministic Rust rail
# (`src/overseer/health_review.rs`) invokes the `overseer-health-review` recipe
# each due tick, parses the agent's typed DECISION markers, and routes each into
# the SAME gated `LaunchRecipe` / `EscalateBlockedGoal` path every other action
# uses. All judgment (crash-loop? systemic-vs-per-goal? fix or escalate?) lives
# in the recipe; Rust only schedules and dispatches.
#
# This scenario:
#   (a) proves the deterministic recipe-runner SEAM the rail depends on: a
#       bash-step fixture emits the known health-review markers and json mode
#       surfaces the FINAL step output (what `SpawnHealthReviewRecipeRunner`
#       forwards to `parse_health_review_output`);
#   (b) validates the rail + every safety invariant via the in-tree parser,
#       config, and run_cycle wiring tests, asserting the critical test names
#       actually ran.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# ── (a) deterministic recipe-runner SEAM (no LLM) ────────────────────────────
# The production rail runs recipe-runner-rs in `--output-format json` and pops
# the FINAL step output (`extract_recipe_decision_output`) before parsing the
# typed markers. Prove that boundary carries the health-review markers intact
# using a deterministic bash step — a systemic LAUNCH_RECIPE, a per-goal
# ESCALATE_GOAL, and the REQUIRED terminal HEALTH_REVIEW_COMPLETE marker.
if command -v recipe-runner-rs >/dev/null 2>&1 && command -v jq >/dev/null 2>&1; then
  WORK="$(mktemp -d /tmp/simard-overseer-health-review.XXXXXX)"
  trap 'rm -rf "$WORK"' EXIT

  RECIPE="$WORK/overseer-health-review-fixture.yaml"
  cat > "$RECIPE" <<'EOF'
name: "overseer-health-review-fixture"
description: "deterministic health-review decision markers (no LLM)"
version: "1.0.0"
context: {}
steps:
  - id: "health-review"
    type: "bash"
    command: |
      echo 'LAUNCH_RECIPE={"task_description":"fix the actor-binding crash-loop re-firing 286x across 7 goals in src/typed_ooda (systemic root cause), additive, CI-green, merge-ready","target_repo":"rysweet/Simard","sequence_group":null}'
      echo 'ESCALATE_GOAL={"goal_id":"g-42","problem":"This goal has no measurable done-signal, so it can never complete.","next_step":"A human should define a concrete acceptance check for this goal.","why":"unmeasurable-done-gate","reason":"health-review:per-goal","link":null}'
      echo 'HEALTH_REVIEW_COMPLETE=1 systemic crash-loop launched, 1 goal escalated'
    output: "health_review_report"
EOF

  echo "== (a) recipe-runner json mode surfaces the health-review markers =="
  JSON_OUT="$(recipe-runner-rs "$RECIPE" --output-format json 2>/dev/null)"
  printf '%s' "$JSON_OUT" | jq -e '.success == true' >/dev/null \
    || { echo "FAIL: fixture recipe did not report success in json mode" >&2; exit 1; }
  STEP_OUTPUT="$(printf '%s' "$JSON_OUT" | jq -r '.step_results | last | .output')"
  echo "json-mode final step output:"
  printf '%s\n' "$STEP_OUTPUT"
  # The typed markers must survive intact across the seam.
  printf '%s' "$STEP_OUTPUT" | grep -q 'LAUNCH_RECIPE=' \
    || { echo "FAIL: LAUNCH_RECIPE marker lost across the seam" >&2; exit 1; }
  printf '%s' "$STEP_OUTPUT" | grep -q 'ESCALATE_GOAL=' \
    || { echo "FAIL: ESCALATE_GOAL marker lost across the seam" >&2; exit 1; }
  printf '%s' "$STEP_OUTPUT" | grep -q 'HEALTH_REVIEW_COMPLETE=' \
    || { echo "FAIL: required terminal HEALTH_REVIEW_COMPLETE marker missing" >&2; exit 1; }
  echo "OK: the recipe-runner seam forwards the health-review decision markers verbatim."
else
  echo "SKIP (a): recipe-runner-rs and/or jq not on PATH — seam proof skipped, in-tree tests still run" >&2
fi

# ── (b) in-tree parser + config + wiring tests (the rail + every invariant) ───
echo "== (b) in-tree overseer health-review tests =="
TEST_LOG="$(mktemp /tmp/simard-overseer-health-review-tests.XXXXXX.log)"
# The parser/reviewer unit tests, the config opt-out tests, and the run_cycle
# wiring tests all share the `health_review` substring.
cargo test --lib --locked health_review -- --nocapture >"$TEST_LOG" 2>&1 \
  || { echo "FAIL: overseer health-review tests did not pass" >&2; cat "$TEST_LOG" >&2; exit 1; }

PASSED="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | grep -oE '[0-9]+' | paste -sd+ - | bc 2>/dev/null || echo 0)"
echo "health-review tests passed: ${PASSED}"
[ "${PASSED:-0}" -ge 1 ] \
  || { echo "FAIL: the health_review filter matched zero tests (module renamed?)" >&2; cat "$TEST_LOG" >&2; exit 1; }

# Assert the CRITICAL invariants actually ran (not just "some tests passed"):
require_test() {
  grep -qF "$1" "$TEST_LOG" \
    || { echo "FAIL: required invariant test did not run: $1" >&2; cat "$TEST_LOG" >&2; exit 1; }
}
# The parser maps typed markers to the EXISTING capabilities.
require_test 'parse_launch_recipe_decision'
require_test 'parse_escalate_goal_decision'
# A HEALTHY verdict fabricates nothing.
require_test 'parse_healthy_pass_yields_no_interventions'
# Fail-closed: the REQUIRED terminal marker gates the whole pass.
require_test 'parse_missing_terminal_marker_is_error'
# Fail-closed: malformed / missing-field decisions are dropped, never fabricated.
require_test 'parse_skips_malformed_json_but_keeps_valid_decisions'
require_test 'parse_skips_escalation_missing_plain_english_fields'
# The rail degrades safely on a runner error / a missing terminal marker.
require_test 'review_degrades_to_empty_on_runner_error'
require_test 'review_degrades_to_empty_on_missing_terminal_marker'
# Degraded-pass recovery: the bounded escalation ladder (shared brain ladder
# primitives) recovers a truncated pass, exhausts fail-closed, honors its
# disable knob, stops on a rung fault, and never fires on a clean/base-fault pass.
require_test 'review_recovers_on_the_schema_repair_rung'
require_test 'review_recovers_on_the_high_effort_rung'
require_test 'review_exhausts_ladder_and_takes_no_remediation'
require_test 'review_disabled_ladder_makes_no_retry'
require_test 'review_stops_ladder_when_a_rung_faults'
require_test 'review_healthy_base_never_enters_the_ladder'
require_test 'review_base_runner_error_never_enters_the_ladder'
# The rail forwards ONLY bounded context vars to the recipe seam (no context files).
require_test 'review_forwards_bounded_context_vars_to_the_seam'
# END-TO-END wiring: both decisions flow through the SAME gate as every action.
require_test 'health_review_routes_launch_and_escalate_into_gated_plan'
# OBSERVABILITY: the pass's HEALTH_REVIEW_COMPLETE verdict is SURFACED on the
# observed state (never discarded) — a HEALTHY pass is an observable
# `Reviewed { 0 }` (not a silent no-op), a fault is a LOUD `Degraded`, and an
# unwired/off-cadence tick stays `NotRun`. Same "no silent OFF" discipline as
# merge-queue reasoning (#4097).
require_test 'health_review_healthy_verdict_surfaces_reviewed_with_zero_decisions'
require_test 'health_review_failure_surfaces_degraded_status'
require_test 'health_review_ok_without_verdict_surfaces_degraded_status'
require_test 'health_review_unwired_leaves_status_not_run'
require_test 'health_review_off_cadence_leaves_status_not_run'
# OPERATOR-FEED surfacing: the verdict is not only on the struct field — it is
# rendered into the operator-visible `observed:` feed (simard status / TUI /
# dashboard via `humanize_tick_details`). A Reviewed pass leaves a
# `health-review: <summary>` breadcrumb (a HEALTHY pass included), a Degraded
# pass is LOUD, and a NotRun tick stays quiet.
require_test 'health_review_verdict_surfaces_in_the_operator_feed'
require_test 'health_review_degraded_surfaces_loud_in_the_operator_feed'
require_test 'health_review_not_run_stays_quiet_in_the_operator_feed'
# The shared gap-scan throttle AND the dedicated opt-out each disable the rail.
require_test 'health_review_skipped_when_gap_scan_disabled'
require_test 'health_review_skipped_when_dedicated_flag_disabled'
# Cadence: the rail honors its every-N knob.
require_test 'health_review_respects_every_n_cadence'
# An unwired rail is a pure no-op (bare constructor / tests behave as before).
require_test 'health_review_unwired_is_a_noop'
# An escalation is HELD (not fabricated) when goal-board health is opted out.
require_test 'health_review_escalation_held_when_goal_health_disabled'
# Config opt-out: default-ON with the acting Overseer; explicit falsey disables.
require_test 'health_review_enabled_by_default_with_the_acting_overseer'
require_test 'health_review_disabled_on_explicit_falsey_values'
require_test 'health_review_forced_off_when_the_acting_overseer_is_disabled'
echo "OK: rail + all safety invariants validated in-tree (${PASSED} tests)."

rm -f "$TEST_LOG"
echo "PASS: overseer-health-review scenario ([standing])"
