#!/usr/bin/env bash
# Outside-in gadugi scenario for the COIN Gym **Phase-5 self-improvement loop**
# (`coin-gym improve --holdout fresh`, issue #2825).
#
# GUARANTEES UNDER TEST (all offline; no VM, no Docker, no network):
#   1. A `run` against the bundled improve-loop snapshot persists the offline
#      scaffold (mock oracle + script) that the live loop needs.
#   2. `improve --holdout fresh` runs a full cycle: failure-analyst →
#      overfitting-reviewer gate → apply → verify on held-out fresh targets →
#      keep-or-roll-back.
#   3. Tactics that GENERALISE to held-out fresh targets (a format-gated decoder
#      and a crypto state machine) are KEPT and banked; a tactic that lifts only
#      TRAINING reach (generic guard, no held-out member) is ROLLED BACK with an
#      overfitting-warning (train/held-out gap).
#   4. Kept tactics are persisted to durable memory keyed by GENERAL FAMILY
#      (never per project/target), and are REUSED on a second cycle (no
#      double-banking; held-out baseline already at 100%).
#   5. `--holdout` only accepts `fresh`.
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

FIXTURE="src/coin_gym/fixtures/improve_loop_snapshot.json"
run_gym() { cargo run --quiet --bin coin-gym -- "$@"; }

# --- 1. run persists the offline scaffold the loop verifies against -----------
RUN_OUT="$(run_gym run "Claude Opus 4.6" --targets "$FIXTURE" --profile loop)"
printf '%s\n' "$RUN_OUT"
printf '%s\n' "$RUN_OUT" | grep -F "OFFLINE SCAFFOLD" >/dev/null
RUN_ID="$(printf '%s\n' "$RUN_OUT" | awk '/^run-id:/ { print $2; exit }')"
test -n "$RUN_ID"
# The persisted run carries the mock oracle + script (needed for held-out grading).
test -f "$HOME_DIR/profiles/loop/runs/$RUN_ID.json"
grep -F '"offline"' "$HOME_DIR/profiles/loop/runs/$RUN_ID.json" >/dev/null

# --- 2/3. cycle 1: keep generalising tactics, roll back the overfit one --------
CYCLE1="$(run_gym improve "$RUN_ID" --profile loop --holdout fresh)"
printf '%s\n' "$CYCLE1"
printf '%s\n' "$CYCLE1" | grep -F "3 accepted" >/dev/null
printf '%s\n' "$CYCLE1" | grep -F "0 rejected" >/dev/null
printf '%s\n' "$CYCLE1" | grep -F "kept 2, rolled back 1, train/held-out-gap warnings 1" >/dev/null
printf '%s\n' "$CYCLE1" | grep -F "2 durable tactic(s)" >/dev/null
printf '%s\n' "$CYCLE1" | grep -F "[KEEP] dec-a (format-gated-decoder)" >/dev/null
printf '%s\n' "$CYCLE1" | grep -F "[KEEP] cry-a (crypto-state-machine)" >/dev/null
printf '%s\n' "$CYCLE1" | grep -F "[ROLLBACK] gen-a (generic)" >/dev/null
printf '%s\n' "$CYCLE1" | grep -F "train/held-out gap:" >/dev/null

# --- 4. durable memory: banked by general family, never per project/target -----
TACTICS="$HOME_DIR/profiles/loop/tactics.json"
test -f "$TACTICS"
COUNT="$(grep -c '"category"' "$TACTICS")"
test "$COUNT" -eq 2
grep -F '"format-gated-decoder"' "$TACTICS" >/dev/null
grep -F '"crypto-state-machine"' "$TACTICS" >/dev/null
# The general (rolled-back) tactic must NOT have been banked.
if grep -F '"generic"' "$TACTICS" >/dev/null; then
  echo "ERROR: rolled-back generic tactic was banked to durable memory" >&2
  exit 1
fi

# --- 4 (cont). cycle 2 reuses banked tactics without double-banking ------------
CYCLE2="$(run_gym improve "$RUN_ID" --profile loop --holdout fresh)"
printf '%s\n' "$CYCLE2"
printf '%s\n' "$CYCLE2" | grep -F "kept 0, rolled back 3" >/dev/null
printf '%s\n' "$CYCLE2" | grep -F "already in durable memory (reused)" >/dev/null
# Memory did not grow.
COUNT2="$(grep -c '"category"' "$TACTICS")"
test "$COUNT2" -eq 2

# --- 5. --holdout only accepts 'fresh' ----------------------------------------
if run_gym improve "$RUN_ID" --profile loop --holdout stale >/tmp/coin-gym-holdout.out 2>&1; then
  echo "ERROR: --holdout stale should have failed" >&2
  exit 1
fi
grep -F "only supports 'fresh'" /tmp/coin-gym-holdout.out >/dev/null
rm -f /tmp/coin-gym-holdout.out

echo "coin-gym-self-improve gadugi scenario: PASS"
