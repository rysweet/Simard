#!/usr/bin/env bash
# dashboard-memory-recent-last-hour-window-honesty.sh — outside-in check (#4318).
#
# The Resources -> Memory sub-section leads with a big headline number
# (#mem-recent-count) fed by /api/memory/recent's `last_hour_count`, captioned
# "items remembered in the last hour". That count is the NET growth of
# long-term memory since a baseline snapshot. In steady state the baseline is
# ~1h old, but when memory_history.json has a gap wider than an hour straddling
# the 1h mark, the chosen baseline is arbitrarily older (e.g. 2.6h), so the
# count is net growth over 2.6h while the caption still claims "in the last
# hour" — it overstates the true last-hour count by ~2.6x under a one-hour
# label. That is a dishonest window shown to a human operator.
#
# The fix surfaces the ACTUAL covered window as `last_hour_window_secs` on
# /api/memory/recent and labels the caption honestly:
#   * window null / within +/-15min of 3600s -> "in the last hour"
#   * window materially != 1h                 -> the true span ("in the last 2.6h")
#
# This script boots the real dashboard binary, logs in, and asserts BOTH the
# live /api/memory/recent response shape (numeric last_hour_window_secs) and the
# served HTML renderer contract (addressable caption element + the
# formatWindowCaption honesty rule wired from the live field).
set -euo pipefail

PORT="${DASH_PORT:-8151}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-mem-window.XXXXXX.log)"
CJ="$(mktemp -t dash-mem-window-cookies.XXXXXX)"

echo "[mem-window] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[mem-window] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[mem-window] starting dashboard on :$PORT"
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
  echo "[mem-window] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[mem-window] FAIL: login rejected" >&2
  exit 1
fi

fail() { echo "[mem-window] FAIL: $1" >&2; exit 1; }

# ── The live endpoint discloses the ACTUAL covered window ────────────────────
RECENT="$(curl -s -b "$CJ" "http://localhost:$PORT/api/memory/recent")"
echo "[mem-window] /api/memory/recent => $RECENT"
# On a healthy live read the window is a number (null only on a fail-closed
# error payload). The key MUST always be present so the caption can be honest.
grep -qE '"last_hour_window_secs"[[:space:]]*:[[:space:]]*([0-9]+(\.[0-9]+)?|null)' <<<"$RECENT" \
  || fail "/api/memory/recent must expose 'last_hour_window_secs' (the true window the last-hour count covers)"

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"

# ── The caption is an addressable element, not a hardcoded literal ───────────
grep -qF 'id="mem-recent-window"' <<<"$HTML" \
  || fail "the last-hour caption must live in an addressable #mem-recent-window element (#4318)"

# ── The renderer implements the honest-window rule and wires the live field ──
grep -qF 'formatWindowCaption(d.last_hour_window_secs)' <<<"$HTML" \
  || fail "fetchRecentMemories must label the caption from the live last_hour_window_secs (#4318)"
grep -qF 'function formatWindowCaption(' <<<"$HTML" \
  || fail "the honest-window helper formatWindowCaption must be defined"
grep -qF 'in the last hour' <<<"$HTML" \
  || fail "formatWindowCaption must keep the plain 'in the last hour' copy for the ~1h/unknown case"
grep -qF '900' <<<"$HTML" \
  || fail "formatWindowCaption must apply a +/-15min (900s) tolerance before claiming 'in the last hour'"

echo "[mem-window] PASS: the last-hour caption honestly reflects the true covered window"
