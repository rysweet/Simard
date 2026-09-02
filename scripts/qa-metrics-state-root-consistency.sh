#!/usr/bin/env bash
# qa-metrics-state-root-consistency.sh
#
# Regression gate: the self-improvement metrics WRITER must resolve its
# directory through the shared state-root ladder so it agrees with the
# dashboard READER.
#
# Root cause (fixed): `self_metrics::metrics_dir()` hardcoded
# `$HOME/.simard/metrics`, ignoring `SIMARD_STATE_ROOT`. The dashboard reader
# (`/api/brain-failures`, `/api/costs`, `/api/metrics`) resolves
# `metrics/metrics.jsonl` under `crate::state_root::simard_state_root()`. The
# writer/reader divergence had two concrete symptoms:
#   1. Operators who relocated their state root via `SIMARD_STATE_ROOT` had
#      metrics written to `$HOME/.simard/metrics` while the dashboard read from
#      `$SIMARD_STATE_ROOT/metrics` — so the costs / brain-failures / metrics
#      tabs showed stale or empty data.
#   2. Hermetic tests (which set `SIMARD_STATE_ROOT` to a temp dir) leaked
#      fixture metrics into the operator's real `~/.simard/metrics/metrics.jsonl`,
#      permanently polluting the live dashboard's lifetime counters with
#      unit-test noise (e.g. thousands of fixture goal_ids like `g1`/`bad-goal`).
#
# The fix routes `metrics_dir()` through `simard_state_root()` so writes and
# reads agree and honor the documented precedence ladder. Production behavior is
# unchanged when `SIMARD_STATE_ROOT` is unset.
#
# This script is a hermetic, network-free pass/fail gate. A non-zero exit on any
# failed assertion is treated by the gadugi `cli` agent runner as a step
# failure, so this is a real gate (not a cosmetic assertion).
set -uo pipefail

fail() {
  echo "QA-METRICS-STATE-ROOT: FAIL - $1"
  exit 1
}

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || fail "cannot cd to repo root"

SRC="src/self_metrics/mod.rs"

# 1. Deterministic unit regression (no network, no daemon): proves the writer
#    follows SIMARD_STATE_ROOT and does not leak into $HOME/.simard/metrics.
TEST="self_metrics::tests::record_metric_follows_state_root_not_home"
echo "QA-METRICS-STATE-ROOT: running the writer-follows-state-root regression…"
if ! cargo test --quiet --lib -- --exact "$TEST"; then
  fail "metrics-writer state-root regression test failed"
fi

# 2. Structural guards on the production source.
#    a. The metrics dir must route through the shared state-root resolver.
grep -q "simard_state_root()" "$SRC" \
  || fail "metrics_dir() no longer routes through simard_state_root() in $SRC"
#    b. It must NOT revert to a hardcoded HOME-based metrics path.
if grep -Eq 'var_os\("HOME"\).*metrics|home\.join\(".simard"\)\.join\("metrics"\)' "$SRC"; then
  fail "metrics_dir() reverted to a hardcoded \$HOME/.simard/metrics path in $SRC"
fi

echo "QA-METRICS-STATE-ROOT: PASS - metrics writer resolves via state root; dashboard reader/writer agree"
