#!/usr/bin/env bash
#
# Done-gate for goal: build-a-local-coin-benchmark-harness-and-a-self-improvement-loop
# (goal id: build-a-local-coin-benchmark-harness-and-a-self-09e65e35)
#
# Purpose: give this one-shot goal a single, machine-checkable finish condition so
# the Overseer's no-progress safeguard can certify "done" instead of re-parking it
# every cycle. The harness and its self-improvement loop already shipped on main
# (src/coin_gym/, delivered via PRs #2740, #2763, #4171, #4208); this gate certifies
# that the shipped work is present AND green in this checkout.
#
# Finish condition (ALL must hold):
#   1. The LOCAL COIN Gym harness source is present under src/coin_gym/
#      (executor, target loader, scorer, leaderboard, agent runner) and the module
#      is wired into the crate.
#   2. The self-improvement loop is present: `run_self_improvement` in
#      src/coin_gym/improve_loop.rs.
#   3. The harness acceptance self-check certifies green:
#         cargo run --quiet --bin coin-gym -- verify   (added by PR #4171)
#      This exercises every criterion end-to-end, including the self-improvement loop.
#   4. The coin_gym test suite is green:
#         cargo test --lib coin_gym
#
# Exit codes:
#   0  => the harness + self-improvement loop are delivered and green (goal may be completed)
#   1  => not certified; the concrete failing criterion is printed by name
#
# The gate is truthful offline: it derives its verdict from files, symbols, the
# acceptance self-check, and the test suite — no network needed. If `gh` is
# available it additionally PRINTS (informational, non-gating) the merged state of
# the delivering PRs. No silent fallback — every path prints why it decided.

set -uo pipefail

REPO="rysweet/Simard"
DELIVERY_PRS=(2740 2763 4171 4208)
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GYM_DIR="${ROOT}/src/coin_gym"

cd "${ROOT}"

echo "[done-gate] goal: build a LOCAL COIN benchmark harness and a self-improvement loop"

fail() {
  echo "[done-gate] FAIL: $1" >&2
  exit 1
}

# 1) Harness source present and wired into the crate.
required_files=(
  "${GYM_DIR}/executor.rs"
  "${GYM_DIR}/target_loader.rs"
  "${GYM_DIR}/scorer.rs"
  "${GYM_DIR}/leaderboard.rs"
  "${GYM_DIR}/agent_runner.rs"
  "${GYM_DIR}/improve_loop.rs"
  "${GYM_DIR}/mod.rs"
)
missing=()
for f in "${required_files[@]}"; do
  [ -f "${f}" ] || missing+=("${f#"${ROOT}/"}")
done
if [ "${#missing[@]}" -gt 0 ]; then
  printf '[done-gate]   missing: %s\n' "${missing[@]}" >&2
  fail "harness source is incomplete; the files above are absent."
fi
grep -q "pub mod coin_gym;" "${ROOT}/src/lib.rs" \
  || fail "coin_gym module is not wired into src/lib.rs."
echo "[done-gate] [PASS] harness source present and wired (src/coin_gym/)."

# 2) Self-improvement loop present.
grep -q "fn run_self_improvement" "${GYM_DIR}/improve_loop.rs" \
  || fail "self-improvement loop (run_self_improvement) is absent from improve_loop.rs."
echo "[done-gate] [PASS] self-improvement loop present (run_self_improvement)."

# 3) Acceptance self-check must certify green.
echo "[done-gate] running harness acceptance self-check: coin-gym verify ..."
verify_log="$(mktemp)"
if cargo run --quiet --bin coin-gym -- verify >"${verify_log}" 2>&1; then
  echo "[done-gate] [PASS] acceptance self-check green:"
  sed 's/^/[done-gate]     /' "${verify_log}"
  rm -f "${verify_log}"
else
  echo "[done-gate] acceptance self-check output:" >&2
  sed 's/^/[done-gate]     /' "${verify_log}" >&2
  echo "[done-gate] verify log: ${verify_log}" >&2
  fail "coin-gym verify did not pass all LOCAL acceptance criteria."
fi

# 4) coin_gym test suite must be green.
echo "[done-gate] running acceptance suite: cargo test --lib coin_gym ..."
test_log="$(mktemp)"
if cargo test --lib coin_gym >"${test_log}" 2>&1; then
  echo "[done-gate] [PASS] coin_gym test suite green ($(grep -Eo '[0-9]+ passed' "${test_log}" | tail -1))."
  rm -f "${test_log}"
else
  echo "[done-gate] test log tail:" >&2
  tail -n 40 "${test_log}" | sed 's/^/[done-gate]     /' >&2
  echo "[done-gate] full test log: ${test_log}" >&2
  fail "coin_gym test suite is not green."
fi

# Informational (non-gating): merged state of the delivering PRs.
if command -v gh >/dev/null 2>&1; then
  for pr in "${DELIVERY_PRS[@]}"; do
    state="$(gh pr view "${pr}" --repo "${REPO}" --json state -q .state 2>/dev/null || true)"
    echo "[done-gate] (info) delivering PR #${pr}: ${state:-unknown}"
  done
fi

echo "[done-gate] CERTIFIED: LOCAL COIN benchmark harness + self-improvement loop delivered and green."
exit 0
