#!/usr/bin/env bash
#
# coverage-gate.sh — the deterministic done-gate for the recurring
# "audit Simard's test coverage and raise it to 70%" goal.
#
# Usage:
#   scripts/coverage-gate.sh [threshold]   # threshold defaults to 70
#
# It runs ONE measurement and answers ONE boolean question:
#
#     is the whole-repo aggregate line coverage >= <threshold>% ?
#
#   - prints the measured total and the verdict (DONE / NOT DONE + the gap)
#   - exits 0 when DONE (coverage >= threshold), 1 when NOT DONE
#   - exits 2 on a measurement/tooling error (could-not-verify)
#
# This is a boolean, not a judgement call: there is no steward-identity gate,
# no recursion guard, and no manual per-module audit charter standing between
# the measurement and the verdict. See Specs/COVERAGE_AUDIT.md.
#
# Requires: cargo-llvm-cov, jq.
set -euo pipefail

threshold="${1:-70}"

if ! command -v jq >/dev/null 2>&1; then
  echo "coverage-gate: jq is required but not installed" >&2
  exit 2
fi
if ! cargo llvm-cov --version >/dev/null 2>&1; then
  echo "coverage-gate: cargo-llvm-cov is required but not installed" >&2
  echo "  install with: cargo install cargo-llvm-cov" >&2
  exit 2
fi

echo "coverage-gate: measuring whole-repo line coverage (cargo llvm-cov)..." >&2

# Measure the same way CI does (.github/workflows/coverage.yml): library +
# binary unit tests, with test files excluded from the denominator. The slow,
# real-subprocess integration tests under tests/ are deliberately NOT run here
# — they take tens of minutes and are prone to being reaped under load, which
# is exactly what made a "just measure it" gate feel unreachable before.
summary_json="$(cargo llvm-cov --no-fail-fast --workspace --lib --bins \
  --ignore-filename-regex 'tests?/' --summary-only --json)" || {
  echo "coverage-gate: cargo llvm-cov failed — could not verify coverage" >&2
  exit 2
}

total="$(printf '%s' "$summary_json" | jq -r '.data[0].totals.lines.percent')"
if [[ -z "$total" || "$total" == "null" ]]; then
  echo "coverage-gate: could not read total line coverage from llvm-cov JSON" >&2
  exit 2
fi

# Integer-scaled comparison keeps the gate free of floating-point shell math.
# The `10#` prefixes force base-10: a scaled value below 1% carries a leading
# zero (e.g. 0.58% -> "05800"), which bash would otherwise read as octal —
# silently mis-comparing all-octal-digit values and erroring on any digit >= 8.
total_scaled="$(printf '%.4f' "$total" | tr -d '.')"
threshold_scaled="$(printf '%.4f' "$threshold" | tr -d '.')"

printf 'coverage-gate: total line coverage = %.2f%% (threshold %s%%)\n' "$total" "$threshold"

if (( 10#$total_scaled >= 10#$threshold_scaled )); then
  printf 'coverage-gate: DONE — %.2f%% >= %s%%. Close the goal (simard goal remove).\n' "$total" "$threshold"
  exit 0
fi

gap="$(printf '%s %s' "$threshold" "$total" | awk '{printf "%.2f", $1 - $2}')"
printf 'coverage-gate: NOT DONE — gap is %s pts. Add hermetic tests to the lowest-coverage src/ files and re-run.\n' "$gap"
exit 1
