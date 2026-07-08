#!/usr/bin/env bash
# goal-board-recall-durability.sh — outside-in regression for issues #2320 / #2316.
#
# After the cognitive-memory de-fork, the dashboard goal board became flaky:
# `seed_goals()` writes 3 active goals, then a write+read round-trip
# intermittently read back an EMPTY board (`full_goal_lifecycle_crud` failing
# with `left: 0, right: 3`). Root cause: every goal write/read opened a fresh
# library store handle, so the open->write->reopen->read cycle raced (fact
# ordering metadata was intermittently absent on reopen), collapsing durable
# recall.
#
# This boots the REAL dashboard binary standalone (no OODA daemon — the tier-2
# direct-open path the fix hardens), seeds the board, then performs many
# write->read round-trips over the HTTP API (status updates on the seeded active
# goals + unique backlog appends). After every write it asserts the board is
# durably recalled: the 3 seeded active goals are always present (never an empty
# or stale board) and every appended backlog item is read back. Before the fix
# this fails within a few iterations; with the shared-store fix it is stable.
#
# NOTE: this scenario deliberately exercises the *append/update* recall path and
# does not assert goal *removal*, which is governed by a separate merge-on-write
# concern (issues #1923 / #1925) outside the scope of the #2320 durability fix.
set -euo pipefail

PORT="${DASH_PORT:-8141}"
ITERATIONS="${RECALL_ITERATIONS:-15}"
TOKEN="recall-durability-$$-$RANDOM"
LOG="$(mktemp -t goal-recall.XXXXXX.log)"
STATE_DIR="$(mktemp -d -t goal-recall-state.XXXXXX)"

# Isolate from the operator's live store so the scenario is hermetic.
export SIMARD_STATE_ROOT="$STATE_DIR"
export SIMARD_DASHBOARD_TOKEN="$TOKEN"

echo "[recall] state root: $STATE_DIR"
echo "[recall] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[recall] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[recall] starting dashboard on :$PORT"
"$BIN" dashboard serve --port="$PORT" >"$LOG" 2>&1 &
DASH_PID=$!
cleanup() { kill "$DASH_PID" 2>/dev/null || true; rm -rf "$STATE_DIR" "$LOG"; }
trap cleanup EXIT

# Wait for the server to accept connections.
up=0
for _ in $(seq 1 30); do
  if curl -s -o /dev/null "http://localhost:$PORT/login"; then up=1; break; fi
  sleep 1
done
if [[ "$up" -ne 1 ]]; then
  echo "[recall] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

AUTH=(-H "Authorization: Bearer $TOKEN")
BASE="http://localhost:$PORT"
fail() { echo "[recall] FAIL: $1" >&2; cat "$LOG" >&2; exit 1; }

# Extract a JSON field with jq (robust vs. grep on JSON).
jget() { jq -r --arg k "$1" '.[$k] // ""'; }
# Report whether an active/backlog goal with the given id is present.
has_id() {
  jq -r --arg w "$1" \
    '([.active[]?.id] + [.backlog[]?.id]) | if index($w) then "yes" else "no" end'
}

# ── Seed the 3 baseline active goals (idempotent) ────────────────────────────
SEED="$(curl -s "${AUTH[@]}" -X POST "$BASE/api/goals/seed")"
status="$(jget status <<<"$SEED")"
[[ "$status" == "ok" || "$status" == "already_seeded" ]] \
  || fail "seed returned unexpected status: $SEED"

BOARD="$(curl -s "${AUTH[@]}" "$BASE/api/goals")"
[[ "$(jget active_count <<<"$BOARD")" == "3" ]] \
  || fail "after seed, expected 3 active goals, got $(jget active_count <<<"$BOARD") ($BOARD)"
for sid in self-improvement knowledge-growth operational-health; do
  [[ "$(has_id "$sid" <<<"$BOARD")" == "yes" ]] || fail "seeded goal $sid missing after seed"
done
echo "[recall] seeded: 3 active goals confirmed"

# ── Stress the write->read durable-recall path ───────────────────────────────
# Each iteration performs two writes (an active-goal status update and a unique
# backlog append) and then re-reads the board, asserting the seeded goals are
# always recalled and the appended item round-trips. This is the exact
# open->write->reopen->read sequence that raced before the shared-store fix.
declare -a toggle=(in-progress blocked paused not-started)
for i in $(seq 1 "$ITERATIONS"); do
  st="${toggle[$(( (i - 1) % ${#toggle[@]} ))]}"
  curl -s "${AUTH[@]}" -X PUT -H 'Content-Type: application/json' \
    -d "{\"status\":\"$st\"}" "$BASE/api/goals/self-improvement/status" | grep -q '"status":"ok"' \
    || fail "iter $i: status update -> $st failed"

  bid="recall-probe-iteration-$i-unique-backlog-idea"
  ADD="$(curl -s "${AUTH[@]}" -X POST -H 'Content-Type: application/json' \
        -d "{\"description\":\"recall probe iteration $i unique backlog idea\",\"type\":\"backlog\"}" \
        "$BASE/api/goals")"
  [[ "$(jget status <<<"$ADD")" == "ok" ]] || fail "iter $i: backlog add failed: $ADD"

  # Durable-recall assertions (#2320): the board must reflect both writes on the
  # very next read — never an empty or stale snapshot.
  BOARD="$(curl -s "${AUTH[@]}" "$BASE/api/goals")"
  ac="$(jget active_count <<<"$BOARD")"
  [[ "$ac" == "3" ]] \
    || fail "iter $i: durable-recall regression — expected 3 seeded active goals, got $ac ($BOARD)"
  for sid in self-improvement knowledge-growth operational-health; do
    [[ "$(has_id "$sid" <<<"$BOARD")" == "yes" ]] \
      || fail "iter $i: seeded goal $sid lost from board (stale/empty read) ($BOARD)"
  done
  [[ "$(has_id "$bid" <<<"$BOARD")" == "yes" ]] \
    || fail "iter $i: just-appended backlog item $bid not recalled on next read ($BOARD)"
done

echo "[recall] PASS: goal-board durable recall stable across $ITERATIONS write->read rounds (#2320/#2316)"
