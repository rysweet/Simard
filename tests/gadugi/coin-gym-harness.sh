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
#   6. `contract` prints the real `coin evaluate`/`coin verify` wiring (snapshot
#      argv, the /answer/ submission contract, and the LOCAL-ONLY guardrail)
#      without running anything, and never leaks the fictional --target/--input
#      flags (issue #3001).
#   7. An unknown command exits non-zero with usage.
#   8. `coin-gym verify` runs the LOCAL harness acceptance self-check (the
#      measurable done-gate): every component criterion passes and it exits 0.
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

# --- 6. contract: the real coin evaluate/verify wiring (issue #3001) -----------
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

# --- 7. unknown command exits non-zero with usage -----------------------------
if run_gym frobnicate >/tmp/coin-gym-bogus.out 2>&1; then
  echo "ERROR: unknown command should have failed" >&2
  exit 1
fi
grep -F "unknown command" /tmp/coin-gym-bogus.out >/dev/null
rm -f /tmp/coin-gym-bogus.out

# --- 8. verify: the LOCAL harness acceptance self-check (the done-gate) --------
# `verify` is the machine-checkable done-criteria for the LOCAL goal: it
# exercises every harness component offline against the bundled sample snapshot
# and exits non-zero if any criterion fails. The CLI runner already enforces
# exit 0; assert each criterion is PASS and the summary line reports 7/7.
VERIFY="$(run_gym verify)"
printf '%s\n' "$VERIFY"
printf '%s\n' "$VERIFY" | grep -F "7/7 criteria passed" >/dev/null
for CRIT in target-loader baseline-runner team-runner scorer \
            leaderboard-comparator self-improvement-loop contract-wiring; do
  printf '%s\n' "$VERIFY" | grep -E "\[PASS\] +${CRIT}" >/dev/null
done
# Not a single criterion may be FAIL.
if printf '%s\n' "$VERIFY" | grep -F "[FAIL]" >/dev/null; then
  echo "ERROR: coin-gym verify reported a FAIL criterion" >&2
  exit 1
fi
# It must name Phase 3 as externally gated / out of scope for this gate.
printf '%s\n' "$VERIFY" | grep -F "Phase 3" >/dev/null
# `verify` rejects unexpected flags (exits non-zero with usage).
if run_gym verify --bogus x >/tmp/coin-gym-verify.out 2>&1; then
  echo "ERROR: verify should reject unknown flags" >&2
  exit 1
fi
grep -F "unknown flag" /tmp/coin-gym-verify.out >/dev/null
rm -f /tmp/coin-gym-verify.out

echo "coin-gym-harness gadugi scenario: PASS"
