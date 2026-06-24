#!/usr/bin/env bash
# goal-crud-serial-isolation.sh — QA-team evidence for #2360 / PR #2365.
#
# The previously-flaky surface was
# `operator_commands_dashboard::tests_goals_crud::full_goal_lifecycle_crud`
# (and its `tests_goals_crud` / `tests_goal_records_migration` siblings), which
# panicked ~15% of multi-threaded `cargo test` runs because a concurrent
# process-global env mutation (SIMARD_STATE_ROOT / HOME / SIMARD_LLM_PROVIDER)
# outside the `cognitive_memory` serial group tore a dashboard goal-handler's
# env read, sending the write to the wrong state root and leaving an empty
# board (`board.active.len() == 3` tripped).
#
# This script re-runs that exact surface under a high-concurrency runner,
# together with the syn-based regression-guard meta-test, asserting it is
# deterministically green. It is the runnable proof that the goal-CRUD board
# lifecycle now persists and reads back correctly under load.
set -euo pipefail

# Assert that a cargo-test invocation both exits 0 and reports a passing
# libtest summary line ("test result: ok"). A bare exit-0 is not enough —
# a filter that matches zero tests also exits 0, so we require the summary.
run_and_assert_ok() {
  local label="$1"
  shift
  local out
  echo "[goal-crud] >>> ${label}"
  echo "[goal-crud]     ${*}"
  if ! out="$("$@" 2>&1)"; then
    echo "${out}"
    echo "[goal-crud] FAIL: ${label} — non-zero exit" >&2
    exit 1
  fi
  if ! grep -q "test result: ok" <<<"${out}"; then
    echo "${out}"
    echo "[goal-crud] FAIL: ${label} — no passing 'test result: ok' summary" >&2
    exit 1
  fi
  if grep -qE "test result: FAILED|\b0 passed\b" <<<"${out}"; then
    echo "${out}"
    echo "[goal-crud] FAIL: ${label} — failing or empty result" >&2
    exit 1
  fi
  grep -E "test result:" <<<"${out}" | sed 's/^/[goal-crud]     /'
  echo "[goal-crud]     PASS: ${label}"
}

# 1. Regression guard: the syn-based meta-test fails the build if any
#    state-root/provider env-mutating test escapes the cognitive_memory group.
run_and_assert_ok "serial_guard meta-test" \
  cargo test --lib every_env_mutating_test_is_serialized

# 2. The previously-flaky goal-CRUD + records-migration suites, run together
#    under a 16-thread runner to maximise the chance of an env-tear. With the
#    cognitive_memory keying in place this must be deterministically green.
run_and_assert_ok "goal-CRUD + records-migration @ 16 threads" \
  cargo test --lib operator_commands_dashboard::tests_goal -- --test-threads=16

# 3. The provider-reader the follow-up commit made hermetic — confirms the
#    dashboard agent-session provider resolution is deterministic.
run_and_assert_ok "dashboard provider reader (hermetic)" \
  cargo test --lib open_agent_session_returns_none_without_provider_config -- --test-threads=16

echo "[goal-crud] ALL CHECKS PASSED — #2360 goal-CRUD flake surface is green under concurrency"
