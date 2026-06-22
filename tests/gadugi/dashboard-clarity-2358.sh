#!/usr/bin/env bash
# dashboard-clarity-2358.sh — outside-in check for the P1 clarity fixes in
# issue #2358.
#
# Three operator-facing confusions are asserted away here, against the served
# dashboard HTML/JS of the real binary:
#
#   1. Memory tab "nothing remembered" lie — the headline used to render the
#      recent-window count (a bare 0 on the library backend) while tens of
#      thousands of memories were stored, and the empty-state said "No memories
#      stored yet". The headline must now reflect the TOTAL, and the empty-state
#      must read "No new memories in the last hour — N total stored".
#   2. Growth badge vs rate sign — the "↑ Growing" badge could render next to a
#      negative "long-term mem/hr" rate. The badge must be derived from the same
#      displayed long-term rate so the two can never contradict.
#   3. Cost honesty — the Costs lede claimed figures were "computed from real
#      provider invoices rather than estimates" while the metric is labeled
#      "Estimated Cost". The lede must no longer make the invoice claim.
set -euo pipefail

PORT="${DASH_PORT:-8139}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-clarity.XXXXXX.log)"
CJ="$(mktemp -t dash-clarity-cookies.XXXXXX)"

echo "[clarity] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[clarity] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[clarity] starting dashboard on :$PORT"
"$BIN" dashboard serve --port="$PORT" >"$LOG" 2>&1 &
DASH_PID=$!
cleanup() { kill "$DASH_PID" 2>/dev/null || true; rm -f "$CJ"; }
trap cleanup EXIT

# Wait for the server to accept connections.
up=0
for _ in $(seq 1 30); do
  if curl -s -o /dev/null "http://localhost:$PORT/login"; then up=1; break; fi
  sleep 1
done
if [[ "$up" -ne 1 ]]; then
  echo "[clarity] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[clarity] FAIL: login rejected" >&2
  exit 1
fi

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"

fail() { echo "[clarity] FAIL: $1" >&2; exit 1; }

# ── 1. Memory headline reflects total stored, not the recent-window count ────
grep -q 'countEl.textContent=memTotal' <<<"$HTML" \
  || fail "memory headline must render the total stored count (memTotal)"
grep -q 'memories<br>stored' <<<"$HTML" \
  || fail "memory headline label must read 'memories stored'"
grep -q 'No new memories in the last hour' <<<"$HTML" \
  || fail "empty-state must explain the recent window without claiming nothing is stored"
grep -q "Recent-memory list isn" <<<"$HTML" \
  || fail "empty-state must stay honest when the recent window is unavailable"
grep -q 'total stored' <<<"$HTML" \
  || fail "empty-state must surface the total stored count"
grep -q 'items remembered' <<<"$HTML" \
  && fail "stale 'items remembered / in the last hour' headline label still present"

# ── 2. Growth badge is derived from the displayed long-term rate ─────────────
grep -q "ltRateDisp>0?'growing'" <<<"$HTML" \
  || fail "trend badge must be derived from the long-term rate so it can't contradict the rate sign"

# ── 3. Cost lede honesty matches the 'Estimated Cost' metric label ───────────
grep -q 'Estimated Cost' <<<"$HTML" \
  || fail "cost metric must keep its honest 'Estimated Cost' label"
grep -q 'real provider invoices rather than estimates' <<<"$HTML" \
  && fail "cost lede still falsely claims figures come from real provider invoices"
grep -q 'estimates derived from token usage' <<<"$HTML" \
  || fail "cost lede must state the figures are estimates derived from token usage"

echo "[clarity] PASS: memory totals, growth-trend agreement, and cost honesty hold (#2358)"
