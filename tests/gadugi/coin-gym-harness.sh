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
#   6. `leaderboard` ranks the saved arms LOCALLY and reports that the
#      multi-agent TEAM climbs above the single-model BASELINE (equal reach,
#      higher precision) — the objective's done-gate view, LOCAL-ONLY.
#   7. `contract` prints the real `coin evaluate`/`coin verify` wiring (snapshot
#      argv, the /answer/ submission contract, and the LOCAL-ONLY guardrail)
#      without running anything, and never leaks the fictional --target/--input
#      flags (issue #3001).
#   8. An unknown command exits non-zero with usage.
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

# --- 6. leaderboard: LOCAL standings show the team climb over the baseline -----
# Pooled across profiles: the baseline arm lives in `opus`, the team arm in
# `opus-team`. The team must rank first (equal reach, higher precision) and the
# best-of-arm verdict must report the climb. LOCAL-ONLY, offline-scaffold.
LEADERBOARD="$(run_gym leaderboard)"
printf '%s\n' "$LEADERBOARD"
printf '%s\n' "$LEADERBOARD" | grep -F "LOCAL leaderboard (all profiles)" >/dev/null
printf '%s\n' "$LEADERBOARD" | grep -F "LOCAL-ONLY:" >/dev/null
# Team ranks #1; baseline #2.
printf '%s\n' "$LEADERBOARD" | grep -E "^ +1 +team +60\.0% +100\.0%" >/dev/null
printf '%s\n' "$LEADERBOARD" | grep -E "^ +2 +baseline +60\.0% +60\.0%" >/dev/null
printf '%s\n' "$LEADERBOARD" | grep -F "precision 60.0% → 100.0% (+40.0 pts)" >/dev/null
printf '%s\n' "$LEADERBOARD" | grep -F "multi-agent team CLIMBS ABOVE the single-model baseline" >/dev/null
printf '%s\n' "$LEADERBOARD" | grep -F "OFFLINE SCAFFOLD" >/dev/null

# Scoped to one profile (baseline only): no team arm ⇒ nothing to compare.
LB_SCOPED="$(run_gym leaderboard --profile opus)"
printf '%s\n' "$LB_SCOPED"
printf '%s\n' "$LB_SCOPED" | grep -F "profile 'opus'" >/dev/null
printf '%s\n' "$LB_SCOPED" | grep -F "need at least one \`baseline\` run AND one \`team\` run" >/dev/null

# --- 7. contract: the real coin evaluate/verify wiring (issue #3001) -----------
# `contract` prints exactly how the harness drives COIN's own oracle, without
# running anything. Asserts the real snapshot-mode argv (no fictional
# --target/--input), the verify step, the /answer/ submission contract, and the
# LOCAL-ONLY guardrail.
CONTRACT="$(run_gym contract --split codeql_only --project cups --source image)"
printf '%s\n' "$CONTRACT"
printf '%s\n' "$CONTRACT" | grep -F "LOCAL-ONLY: true" >/dev/null
printf '%s\n' "$CONTRACT" | \
  grep -F "coin evaluate --dataset COIN-Bench/coin --revision v2026-07 --split codeql_only --project cups --source image" >/dev/null
printf '%s\n' "$CONTRACT" | grep -F "coin verify --experiment" >/dev/null
printf '%s\n' "$CONTRACT" | grep -F "/answer/blob.bin + /answer/blob.harness" >/dev/null
printf '%s\n' "$CONTRACT" | grep -F "/answer/UNREACHABLE.md" >/dev/null
printf '%s\n' "$CONTRACT" | grep -F "read \`reached\` from each result.json" >/dev/null
# The fictional per-input flags must never appear.
if printf '%s\n' "$CONTRACT" | grep -Eq -- "--target|--input"; then
  echo "ERROR: contract argv leaked fictional --target/--input flags" >&2
  exit 1
fi
# An unknown --source is rejected.
if run_gym contract --source bogus >/tmp/coin-gym-src.out 2>&1; then
  echo "ERROR: unknown --source should have failed" >&2
  exit 1
fi
grep -F "unknown --source" /tmp/coin-gym-src.out >/dev/null
rm -f /tmp/coin-gym-src.out

# --- 8. unknown command exits non-zero with usage -----------------------------
if run_gym frobnicate >/tmp/coin-gym-bogus.out 2>&1; then
  echo "ERROR: unknown command should have failed" >&2
  exit 1
fi
grep -F "unknown command" /tmp/coin-gym-bogus.out >/dev/null
rm -f /tmp/coin-gym-bogus.out

echo "coin-gym-harness gadugi scenario: PASS"
