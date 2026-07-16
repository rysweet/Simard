#!/usr/bin/env bash
# qa-brain-parse-consistency.sh
#
# Regression gate for issue #4187: the dashboard Brain-Failures tab's LIFETIME
# parse-failure count must be self-consistent with the recent-window count.
#
# Root cause (fixed): `count_brain_parse_failure_metrics` (which feeds both
# `lifetime.parse_failure_count` and `summary.metrics_parse_failure_count`)
# counted metric lines with a raw substring match on "brain_parse_failure".
# That has two defects:
#   1. It DROPS genuine `brain_parse_error` entries (the current recipe-brain
#      metric name), so after the metric-name transition the lifetime total can
#      read 0 while the recent window — which already reads BOTH names via
#      `BRAIN_PARSE_METRIC_NAMES` — shows real failures. The lifetime total then
#      contradicts the recent window.
#   2. It counts FALSE POSITIVES: any unrelated metric whose `context` field
#      merely mentions the string is miscounted.
#
# The fix keys the lifetime count on the parsed `metric_name` field against the
# same `BRAIN_PARSE_METRIC_NAMES` set the recent-window path uses, mirroring
# `recent_parse_failures_from_metrics` minus the time filter.
#
# This script is a hermetic, network-free pass/fail gate. It:
#   1. Runs the deterministic unit regressions that prove the lifetime counter
#      keys on metric_name, counts BOTH metric names, and rejects context-only
#      false positives.
#   2. Structurally guards the production wiring so a future edit cannot quietly
#      revert the lifetime counter to the fragile raw-substring match.
#
# A non-zero exit on any failed assertion is treated by the gadugi `cli` agent
# runner as a step failure, so this is a real gate (not a cosmetic assertion).
set -uo pipefail

fail() {
  echo "QA-BRAIN-PARSE: FAIL - $1"
  exit 1
}

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || fail "cannot cd to repo root"

SRC="src/operator_commands_dashboard/brain_failures.rs"

# 1. Deterministic unit regressions (no network, no daemon). These fail loudly
#    if the lifetime counter stops keying on metric_name, drops the
#    brain_parse_error name, or re-admits context-only false positives.
TESTS=(
  "operator_commands_dashboard::brain_failures::tests::count_brain_parse_failure_metrics_counts_matching_lines"
  "operator_commands_dashboard::brain_failures::tests::count_brain_parse_failure_metrics_counts_both_metric_names"
  "operator_commands_dashboard::brain_failures::tests::count_brain_parse_failure_metrics_ignores_context_false_positive"
)
echo "QA-BRAIN-PARSE: running ${#TESTS[@]} deterministic regressions…"
if ! cargo test --quiet --lib -- --exact "${TESTS[@]}"; then
  fail "brain-failures lifetime parse-count regression tests failed"
fi

# 2. Structural guards on the production source.
#    a. The lifetime counter must NOT revert to the fragile raw line-substring
#       match that dropped brain_parse_error and admitted context false
#       positives.
if grep -q 'line.contains("brain_parse_failure")' "$SRC"; then
  fail "lifetime counter reverted to fragile raw-substring match in $SRC"
fi
#    b. The lifetime counter must key on the shared metric-name set.
grep -q "BRAIN_PARSE_METRIC_NAMES.contains" "$SRC" \
  || fail "lifetime counter no longer keys on BRAIN_PARSE_METRIC_NAMES in $SRC"
#    c. The shared metric-name set must include BOTH names so lifetime and
#       recent stay consistent across the transition.
grep -q 'BRAIN_PARSE_METRIC_NAMES.*=.*&\[.*"brain_parse_error".*"brain_parse_failure"' "$SRC" \
  || fail "BRAIN_PARSE_METRIC_NAMES no longer covers both parse metric names in $SRC"

echo "QA-BRAIN-PARSE: PASS - Brain-Failures lifetime parse count is self-consistent (#4187)"
