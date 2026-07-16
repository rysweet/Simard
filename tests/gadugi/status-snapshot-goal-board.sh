#!/usr/bin/env bash
# status-snapshot-goal-board.sh — outside-in qa-team check for the unified
# Status snapshot goal-board wiring (issue #4196).
#
# BUG: the unified Status snapshot — surfaced on the dashboard Overview tab's
# "System Status" (`GET /api/status/snapshot`), the `simard status` CLI, and the
# TUI Status tab — hard-coded its goal-board section to
# `availability: unavailable / note: "goal board read deferred …"`, EVEN THOUGH
# the live goal board is fully readable (the `/api/goals` panel returns the
# active goals at the same instant). An operator reading the single consolidated
# Status snapshot could never see goal-board state (active / blocked + why /
# not-started) from that surface.
#
# FIX: `assemble_goals(state_root)` reads the durable `goal-board:snapshot`
# through the SAME process-agnostic reader client (`open_reader_client` +
# `load_goal_board`) that backs `/api/goals` and the TUI, and maps each active
# goal into the snapshot's GoalItem (short_id, p{priority}, status Display with
# the blocked reason, first-line-capped summary). Fail-visible: a reader/read
# fault degrades ONLY this section to `error`; a readable-but-empty board reads
# back present + live (empty list), distinct from `unavailable`.
#
# This script boots the real dashboard binary, logs in with the dashkey, reads
# BOTH `/api/status/snapshot` and `/api/goals`, and asserts the snapshot goal
# board is now `ok`/`live` (not the retired "deferred" placeholder) and — when
# the live `/api/goals` board is non-empty — that the snapshot surfaces the same
# active goals with their status. Also asserts the terminal `rendered` block
# contains a GOAL BOARD section and no longer carries the deferred note.
set -euo pipefail

PORT="${DASH_PORT:-8156}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-status-goals.XXXXXX.log)"
CJ="$(mktemp -t dash-status-goals-cookies.XXXXXX)"

echo "[status-goals] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[status-goals] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[status-goals] starting dashboard on :$PORT"
"$BIN" dashboard serve --port="$PORT" >"$LOG" 2>&1 &
DASH_PID=$!
cleanup() { kill "$DASH_PID" 2>/dev/null || true; rm -f "$CJ" "$LOG" "${SNAP_FILE:-}" "${GOALS_FILE:-}"; }
trap cleanup EXIT

up=0
for _ in $(seq 1 30); do
  if curl -s -o /dev/null "http://localhost:$PORT/login"; then up=1; break; fi
  sleep 1
done
if [[ "$up" -ne 1 ]]; then
  echo "[status-goals] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[status-goals] FAIL: login rejected" >&2
  exit 1
fi

SNAP_FILE="$(mktemp -t dash-status-snap.XXXXXX.json)"
GOALS_FILE="$(mktemp -t dash-status-goals.XXXXXX.json)"
curl -s -b "$CJ" "http://localhost:$PORT/api/status/snapshot" >"$SNAP_FILE"
curl -s -b "$CJ" "http://localhost:$PORT/api/goals" >"$GOALS_FILE"

fail() { echo "[status-goals] FAIL: $1" >&2; exit 1; }

# The whole assertion suite runs inside python for robust JSON handling. The
# live goal board is authoritative: whatever `/api/goals` shows, the unified
# snapshot must agree that the board is readable (ok/live) and surface the same
# active goals. The payloads are passed by FILE PATH (not env/argv) because the
# snapshot's `rendered` block makes the JSON far larger than ARG_MAX — the exact
# E2BIG class this project guards against.
SNAP_FILE="$SNAP_FILE" GOALS_FILE="$GOALS_FILE" python3 - <<'PY' || fail "status snapshot goal-board assertions failed"
import json, os, sys

with open(os.environ["SNAP_FILE"]) as fh:
    snap = json.load(fh)
with open(os.environ["GOALS_FILE"]) as fh:
    goals = json.load(fh)

data = snap.get("data", {})
sec = data.get("goals", {})

def bail(msg):
    print(f"[status-goals] FAIL: {msg}", file=sys.stderr)
    sys.exit(1)

# 1. The section must no longer be the retired "deferred / unavailable" stub.
if sec.get("availability") != "ok":
    bail(f"snapshot goals availability must be 'ok', got {sec.get('availability')!r} (note={sec.get('note')!r})")
if sec.get("freshness") != "live":
    bail(f"snapshot goals freshness must be 'live', got {sec.get('freshness')!r}")
if sec.get("note"):
    bail(f"a healthy snapshot goals section must carry no error/deferred note, got {sec.get('note')!r}")

board = sec.get("data") or {}
active = board.get("active")
if not isinstance(active, list):
    bail(f"snapshot goals.data.active must be a list, got {type(active).__name__}")

# 2. Each surfaced goal carries the mapped shape.
for g in active:
    for field in ("short_id", "priority", "status", "summary"):
        if field not in g:
            bail(f"snapshot goal item missing '{field}': {g}")
    if not g["priority"].startswith("p"):
        bail(f"snapshot goal priority must be a p<N> label, got {g['priority']!r}")

# 3. Parity with the live /api/goals board: the snapshot's active-goal count
#    must match the live active board (both read the same durable snapshot).
live_active = goals.get("active", [])
if isinstance(live_active, list) and live_active:
    if not active:
        bail("live /api/goals has active goals but the snapshot board is empty — the wiring regressed")
    # A blocked live goal must surface its blocked status in the snapshot too.
    live_blocked = [x for x in live_active if str(x.get("status", "")).startswith("blocked")]
    if live_blocked:
        snap_blocked = [x for x in active if x["status"].startswith("blocked")]
        if not snap_blocked:
            bail("live board has blocked goals but the snapshot surfaces none of their blocked status/why")

# 4. The terminal rendering must include a GOAL BOARD section and NOT the
#    retired deferred placeholder.
rendered = snap.get("rendered", "")
if "GOAL BOARD" not in rendered:
    bail("terminal rendering must contain a GOAL BOARD section")
if "goal board read deferred" in rendered:
    bail("terminal rendering must not carry the retired 'goal board read deferred' placeholder")

print(f"[status-goals] snapshot goal board OK: availability=ok freshness=live active={len(active)}")
PY

echo "[status-goals] PASS: unified Status snapshot surfaces the live goal board (issue #4196)"
