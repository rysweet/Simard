#!/usr/bin/env bash
# dashboard-memory-recent-last-hour-consistency.sh — outside-in check.
#
# The Memory tab shows a big headline number (#mem-recent-count) fed by
# /api/memory/recent's `last_hour_count`, labelled "items remembered in the
# last hour". Directly beside it the recent-memories list renders an
# empty-state string. On the library backend per-item listing is unavailable
# (`available:false`, `items:[]`) yet `last_hour_count` can be POSITIVE, so the
# pre-fix renderer — which branched only on the aggregate `total` — printed
# "No new memories in the last hour" next to a headline of, e.g., 313. That is
# a self-contradiction that misreports live memory health to a human operator.
#
# The fix branches the empty-items copy on `last_hour_count`:
#   * last_hour_count > 0  -> "N memories recorded in the last hour…" (agrees
#                             with the headline; per-item detail unavailable)
#   * last_hour_count == 0 && total > 0 -> "No new memories in the last hour — N total stored."
#   * total == 0           -> "No memories stored yet…"
#
# This script boots the real dashboard binary, logs in, and asserts the served
# HTML renderer contract plus the live /api/memory/recent response shape.
set -euo pipefail

PORT="${DASH_PORT:-8149}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-mem-consistency.XXXXXX.log)"
CJ="$(mktemp -t dash-mem-consistency-cookies.XXXXXX)"

echo "[mem-consistency] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[mem-consistency] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[mem-consistency] starting dashboard on :$PORT"
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
  echo "[mem-consistency] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[mem-consistency] FAIL: login rejected" >&2
  exit 1
fi

fail() { echo "[mem-consistency] FAIL: $1" >&2; exit 1; }

# ── The live endpoint exposes a numeric last-hour count ──────────────────────
RECENT="$(curl -s -b "$CJ" "http://localhost:$PORT/api/memory/recent")"
echo "[mem-consistency] /api/memory/recent => $RECENT"
grep -qE '"last_hour_count"[[:space:]]*:[[:space:]]*[0-9]+' <<<"$RECENT" \
  || fail "/api/memory/recent must report a numeric 'last_hour_count'"

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"

# ── Renderer contract: empty-state branches on the last-hour count ───────────
grep -qF 'const lastHour=d.last_hour_count||0' <<<"$HTML" \
  || fail "fetchRecentMemories must derive the last-hour count for its empty-state branch"
grep -qF 'recorded in the last hour' <<<"$HTML" \
  || fail "when last_hour_count>0 the panel must say memories WERE recorded in the last hour (consistent with the #mem-recent-count headline), never 'No new memories'"

# ── The zero-window and empty-store copies must remain (regression guard) ────
grep -qF 'No new memories in the last hour' <<<"$HTML" \
  || fail "the zero-last-hour branch must still say 'No new memories in the last hour' (#2358)"
grep -qF 'No memories stored yet' <<<"$HTML" \
  || fail "the empty-store branch must still fall back to 'No memories stored yet' (#2358)"

echo "[mem-consistency] PASS: Memory recent-memories copy is consistent with the last-hour headline count"
