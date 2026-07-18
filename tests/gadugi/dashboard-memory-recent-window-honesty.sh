#!/usr/bin/env bash
# dashboard-memory-recent-window-honesty.sh — outside-in check for issue #4318 (P1).
#
# The Memory tab shows a big "items remembered / in the last hour" number bound
# to /api/memory/recent's `last_hour_count`. That count is the net long-term
# growth since the most-recent memory-history snapshot at-or-before now−1h. When
# snapshots are SPARSE (a gap wider than an hour straddling the one-hour mark),
# the chosen baseline can be much older than an hour — observed 2.6h — so the
# number spans >1h while the caption still claims "in the last hour", overstating
# the true last-hour rate.
#
# The fix surfaces `last_hour_window_secs` (the ACTUAL elapsed time the count
# covers) from /api/memory/recent, and makes the caption honest: "in the last
# hour" only when the window is ~1h (±15 min), otherwise the real window.
#
# This script boots the real dashboard binary, logs in, and asserts both the
# live /api/memory/recent response shape and the served HTML renderer contract.
set -euo pipefail

PORT="${DASH_PORT:-8148}"
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

# ── The live endpoint must expose the honest window field ────────────────────
RECENT="$(curl -s -b "$CJ" "http://localhost:$PORT/api/memory/recent")"
echo "[mem-window] /api/memory/recent => $RECENT"
# `last_hour_window_secs` must be present: a number on the healthy path or null.
grep -qE '"last_hour_window_secs"[[:space:]]*:[[:space:]]*([0-9]+(\.[0-9]+)?|null)' <<<"$RECENT" \
  || fail "/api/memory/recent must surface last_hour_window_secs so the caption can be honest (#4318)"
# The count itself must remain present alongside the window.
grep -qE '"last_hour_count"[[:space:]]*:[[:space:]]*([0-9]+|null)' <<<"$RECENT" \
  || fail "/api/memory/recent must still report last_hour_count (#2679 regression guard)"

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"

# ── Renderer contract: the caption is derived from the real window ───────────
grep -qF 'id="mem-recent-window"' <<<"$HTML" \
  || fail "the 'in the last hour' caption must be a targetable element so it can be labeled honestly (#4318)"
grep -qF 'function formatRecentWindow' <<<"$HTML" \
  || fail "the renderer must format the caption from last_hour_window_secs (#4318)"
grep -qF 'd.last_hour_window_secs' <<<"$HTML" \
  || fail "fetchRecentMemories must read last_hour_window_secs to label the number (#4318)"
# When the window is ~1h the caption stays the canonical copy.
grep -qF 'in the last hour' <<<"$HTML" \
  || fail "the canonical ~1h caption 'in the last hour' must remain for genuine one-hour windows (#4318)"

echo "[mem-window] PASS: Memory tab labels the 'items remembered' number with the window it truly covers (#4318)"
