#!/usr/bin/env bash
# Gym suite gadugi scenario (rysweet/Simard#2548).
#
# The `starter` suite is the deterministic self-test / self-update *health gate*.
# It must be genuinely green on a healthy binary and exit 0. The scenario
# pins that honest contract:
#
#   * `gym list` still exposes the FULL benchmark catalogue (unchanged) —
#     including the LLM-content-check scenarios that are graded by an external
#     reasoning backend.
#   * `gym run-suite starter` runs ONLY the deterministic, credential-free
#     session-quality scenarios, reports `Suite passed: true`, and exits 0.
#   * the LLM-content-check scenarios (e.g. `repo-exploration-local`) are NOT in
#     the gate — keeping them out is what makes self-test honest rather than a
#     false-green.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# ── list: the full catalogue is unchanged and still discoverable ──────────
LIST_OUTPUT="$(
  cargo run --quiet --bin simard-gym -- list
)"

printf '%s\n' "$LIST_OUTPUT"
printf '%s\n' "$LIST_OUTPUT" | grep -F "repo-exploration-local" >/dev/null
printf '%s\n' "$LIST_OUTPUT" | grep -F "docs-refresh-copilot" >/dev/null
printf '%s\n' "$LIST_OUTPUT" | grep -F "safe-code-change-rusty-clawd" >/dev/null
printf '%s\n' "$LIST_OUTPUT" | grep -F "composite-session-review" >/dev/null
printf '%s\n' "$LIST_OUTPUT" | grep -F "interactive-terminal-driving" >/dev/null

# ── run-suite: the health gate is deterministic and genuinely green ───────
# `set -e` also asserts the exit code is 0 for a passing suite.
SUITE_OUTPUT="$(
  cargo run --quiet --bin simard-gym -- run-suite starter
)"

printf '%s\n' "$SUITE_OUTPUT"
printf '%s\n' "$SUITE_OUTPUT" | grep -F "Suite: starter" >/dev/null
printf '%s\n' "$SUITE_OUTPUT" | grep -F "Suite passed: true" >/dev/null
printf '%s\n' "$SUITE_OUTPUT" | grep -F "composite-session-review: passed" >/dev/null
printf '%s\n' "$SUITE_OUTPUT" | grep -F "interactive-terminal-driving: passed" >/dev/null
printf '%s\n' "$SUITE_OUTPUT" | grep -F "session-quality-memory-export: passed" >/dev/null

# The LLM-content-check scenarios are benchmarks (run via `gym run <id>`),
# not health-gate checks, so they must NOT appear in the gate output.
if printf '%s\n' "$SUITE_OUTPUT" | grep -F "repo-exploration-local" >/dev/null; then
  echo "FAIL: LLM-content-check scenario leaked into the self-test gate" >&2
  exit 1
fi

# ── the gate must exit non-zero when the suite fails ──────────────────────
# An unknown suite is the deterministic, side-effect-free way to exercise the
# non-zero-exit path that makes self-test trustworthy.
if cargo run --quiet --bin simard-gym -- run-suite definitely-not-a-suite >/dev/null 2>&1; then
  echo "FAIL: run-suite returned 0 for an unknown suite (false-green)" >&2
  exit 1
fi

# ── the persisted suite artifact agrees with the operator-facing output ───
SUITE_REPORT="$(printf '%s\n' "$SUITE_OUTPUT" | sed -n 's/^Suite artifact report: //p')"
[ -n "$SUITE_REPORT" ]
[ -f "$SUITE_REPORT" ]

grep -F '"suite_id": "starter"' "$SUITE_REPORT" >/dev/null
grep -F '"passed": true' "$SUITE_REPORT" >/dev/null

