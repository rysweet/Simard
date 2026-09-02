#!/usr/bin/env bash
# dashboard-goals-active-breakdown.sh — outside-in qa-team check for the Goals
# tab active-lifecycle breakdown.
#
# BUG: an operator asking "what is Simard actually working on right now?" read
# the Goals tab and saw only "N active goal(s)". But the active board mixes
# genuinely in-progress goals with ones that are blocked, paused, not-started,
# or already `Completed` yet not-yet-archived off the board. On a live host,
# 9 of 20 "active" goals were `Completed` — so the single `active_count` badly
# overstated in-flight work and hid the block/complete split.
#
# FIX: `/api/goals` additively exposes `active_status_breakdown` — a faithful
# per-`GoalProgress`-variant count (proposed, not_started, in_progress, blocked,
# paused, completed) — and the Goals tab appends the nonzero buckets to the
# count line via a `goalBreakdownText()` helper.
#
# This script boots the real dashboard binary, logs in with the dashkey, and
# asserts BOTH the served HTML (helper + wiring present) AND the live
# `/api/goals` JSON (breakdown object with all six keys, counts consistent).
set -euo pipefail

PORT="${DASH_PORT:-8144}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-goals-breakdown.XXXXXX.log)"
CJ="$(mktemp -t dash-goals-breakdown-cookies.XXXXXX)"

echo "[goals-breakdown] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[goals-breakdown] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[goals-breakdown] starting dashboard on :$PORT"
"$BIN" dashboard serve --port="$PORT" >"$LOG" 2>&1 &
DASH_PID=$!
cleanup() { kill "$DASH_PID" 2>/dev/null || true; rm -f "$CJ"; }
trap cleanup EXIT

up=0
for _ in $(seq 1 30); do
  if curl -s -o /dev/null "http://localhost:$PORT/login"; then up=1; break; fi
  sleep 1
done
if [[ "$up" -ne 1 ]]; then
  echo "[goals-breakdown] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[goals-breakdown] FAIL: login rejected" >&2
  exit 1
fi

fail() { echo "[goals-breakdown] FAIL: $1" >&2; exit 1; }

# ── Served HTML wires in the breakdown helper + the count-line call ──────────
HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"
grep -qF 'function goalBreakdownText(' <<<"$HTML" \
  || fail "the goalBreakdownText() helper must be present in the served HTML"
grep -qF 'goalBreakdownText(d.active_status_breakdown)' <<<"$HTML" \
  || fail "the active-count line must append goalBreakdownText(d.active_status_breakdown)"

# ── Live /api/goals JSON carries the additive breakdown with all six keys ────
GOALS="$(curl -s -b "$CJ" "http://localhost:$PORT/api/goals")"
python3 - "$GOALS" <<'PY'
import json, sys
d = json.loads(sys.argv[1])
bd = d.get("active_status_breakdown")
assert isinstance(bd, dict), f"active_status_breakdown must be an object, got {type(bd)}"
keys = ["proposed", "not_started", "in_progress", "blocked", "paused", "completed"]
for k in keys:
    assert k in bd, f"breakdown missing key {k!r}"
    assert isinstance(bd[k], int) and bd[k] >= 0, f"breakdown[{k!r}] must be a non-negative int, got {bd[k]!r}"
# The buckets must sum to the number of active goals (faithful partition).
total = sum(bd[k] for k in keys)
assert total == d.get("active_count"), (
    f"breakdown buckets sum to {total} but active_count is {d.get('active_count')}"
)
# active_count is preserved (back-compat) alongside the additive field.
assert "active_count" in d, "active_count must remain present (back-compat)"
print(f"[goals-breakdown] breakdown OK: {bd} (active_count={d.get('active_count')})")
PY

echo "[goals-breakdown] PASS: Goals tab exposes and renders the active lifecycle breakdown"
