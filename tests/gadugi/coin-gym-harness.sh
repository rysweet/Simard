#!/usr/bin/env bash
# Outside-in gadugi scenario for the LOCAL COIN Gym harness CLI (`coin-gym`),
# Phase 4 of issue #2713.
#
# GUARANTEES UNDER TEST (all offline; no VM, no Docker, no network):
#   1. `coin-gym run` drives the full pipeline (target-load → agent-run → grade
#      → score) against the bundled sample snapshot and a mock oracle, and
#      labels the run OFFLINE SCAFFOLD.
#   2. The multi-agent TEAM strategy lifts precision over the single-model
#      BASELINE at equal reach — the central baseline-vs-team design property
#      (baseline 3R/2W = 60% precision; team 3R/2A = 100% precision).
#   3. `score`, `compare`, and `improve` reload a saved run and report
#      reach/precision, leaderboard deltas, and gated tactic proposals.
#   4. The overfitting-reviewer gate ACCEPTs the analyst's general tactics, and
#      `improve` flags the live verify/rollback loop as Phase 5.
#   5. `profiles` lists isolated per-model run state.
#   6. An unknown command exits non-zero with usage.
#
# Hermetic: COIN_GYM_HOME is a throwaway temp dir; nothing touches the real
# ~/.simard state or the network.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

HOME_DIR="$(mktemp -d)"
cleanup() { rm -rf "$HOME_DIR"; }
trap cleanup EXIT
export COIN_GYM_HOME="$HOME_DIR"

run_gym() { cargo run --quiet --bin coin-gym -- "$@"; }

# --- 1. baseline run: full offline pipeline -----------------------------------
BASELINE="$(run_gym run claude-opus-4.6 --strategy baseline --profile opus)"
printf '%s\n' "$BASELINE"
printf '%s\n' "$BASELINE" | grep -F "OFFLINE SCAFFOLD" >/dev/null
printf '%s\n' "$BASELINE" | grep -F "reach: 60.0%" >/dev/null
printf '%s\n' "$BASELINE" | grep -F "precision: 60.0%" >/dev/null
printf '%s\n' "$BASELINE" | grep -F "R:3/W:2/A:0/T:0/N:0/E:0" >/dev/null

# --- 2. team run: abstention gate lifts precision at equal reach ---------------
TEAM="$(run_gym run claude-opus-4.6 --strategy team --profile opus-team)"
printf '%s\n' "$TEAM"
printf '%s\n' "$TEAM" | grep -F "reach: 60.0%" >/dev/null
printf '%s\n' "$TEAM" | grep -F "precision: 100.0%" >/dev/null
printf '%s\n' "$TEAM" | grep -F "R:3/W:0/A:2/T:0/N:0/E:0" >/dev/null

# --- 3. score / compare / improve reload the saved baseline run ---------------
RUN_ID="$(printf '%s\n' "$BASELINE" | awk '/^run-id:/ { print $2; exit }')"
test -n "$RUN_ID"

SCORE="$(run_gym score "$RUN_ID")"
printf '%s\n' "$SCORE"
printf '%s\n' "$SCORE" | grep -F "reach: 60.0%" >/dev/null
printf '%s\n' "$SCORE" | grep -F "frontier" >/dev/null
printf '%s\n' "$SCORE" | grep -F "non-trivial-reachable" >/dev/null
# Reloaded mock-oracle runs must still carry the OFFLINE SCAFFOLD warning.
printf '%s\n' "$SCORE" | grep -F "OFFLINE SCAFFOLD" >/dev/null

COMPARE="$(run_gym compare "$RUN_ID")"
printf '%s\n' "$COMPARE"
printf '%s\n' "$COMPARE" | grep -F "published: Claude Opus 4.6" >/dev/null
printf '%s\n' "$COMPARE" | grep -F "material-deviation:" >/dev/null
printf '%s\n' "$COMPARE" | grep -F "offline scaffold" >/dev/null

# --- 4. improve: general tactics ACCEPTed; live loop flagged Phase 5 -----------
IMPROVE="$(run_gym improve "$RUN_ID")"
printf '%s\n' "$IMPROVE"
printf '%s\n' "$IMPROVE" | grep -F "[ACCEPT]" >/dev/null
printf '%s\n' "$IMPROVE" | grep -F "Phase 5" >/dev/null
# The gate must not have rejected the analyst's own general tactics.
printf '%s\n' "$IMPROVE" | grep -F "rejected: 0" >/dev/null

# --- 5. profiles: isolated per-model run state --------------------------------
PROFILES="$(run_gym profiles)"
printf '%s\n' "$PROFILES"
printf '%s\n' "$PROFILES" | grep -F "opus" >/dev/null

# --- 6. unknown command exits non-zero with usage -----------------------------
if run_gym frobnicate >/tmp/coin-gym-bogus.out 2>&1; then
  echo "ERROR: unknown command should have failed" >&2
  exit 1
fi
grep -F "unknown command" /tmp/coin-gym-bogus.out >/dev/null
rm -f /tmp/coin-gym-bogus.out

echo "coin-gym-harness gadugi scenario: PASS"
