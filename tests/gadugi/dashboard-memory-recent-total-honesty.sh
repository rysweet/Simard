#!/usr/bin/env bash
# dashboard-memory-recent-total-honesty.sh — outside-in check for issue #2358 (P1).
#
# The Memory tab's recent-memories panel used to tell a human "No memories
# stored yet. Simard will remember things as it works." whenever the last-hour
# window was empty — even though the store actually held tens of thousands of
# memories. The root cause was twofold:
#   * the /api/memory/recent handler hardcoded `total: 0` (per-item listing is
#     unavailable on the library backend), so the front-end never saw the real
#     aggregate; and
#   * the renderer showed the "nothing stored, ever" copy for any empty window.
#
# This change surfaces the live aggregate `total` from the same get_statistics()
# path /api/memory/history uses, and branches the empty-state copy on it:
#   * total>0  -> "No new memories in the last hour — N total stored."
#   * total==0 -> the original "No memories stored yet…" copy.
#
# This script boots the real dashboard binary, logs in, and asserts both the
# live /api/memory/recent response shape and the served HTML renderer contract.
set -euo pipefail

PORT="${DASH_PORT:-8147}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-mem-honesty.XXXXXX.log)"
CJ="$(mktemp -t dash-mem-honesty-cookies.XXXXXX)"

echo "[mem-honesty] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[mem-honesty] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[mem-honesty] starting dashboard on :$PORT"
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
  echo "[mem-honesty] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[mem-honesty] FAIL: login rejected" >&2
  exit 1
fi

fail() { echo "[mem-honesty] FAIL: $1" >&2; exit 1; }

# ── The live endpoint must expose a numeric aggregate `total` ────────────────
RECENT="$(curl -s -b "$CJ" "http://localhost:$PORT/api/memory/recent")"
echo "[mem-honesty] /api/memory/recent => $RECENT"
grep -qE '"total"[[:space:]]*:[[:space:]]*[0-9]+' <<<"$RECENT" \
  || fail "/api/memory/recent must report a numeric aggregate 'total' (#2358)"
# The handler is explicit that per-item listing stays unavailable on this
# backend; only the aggregate count is surfaced here.
grep -q '"available":false' <<<"$RECENT" \
  || fail "/api/memory/recent must keep per-item listing marked unavailable (#2307)"

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"

# ── Renderer contract: empty-state branches on the aggregate total ───────────
grep -qF 'const total=d.total||0' <<<"$HTML" \
  || fail "fetchRecentMemories must read the aggregate total before choosing empty-state copy (#2358)"
grep -qF 'No new memories in the last hour' <<<"$HTML" \
  || fail "when total>0 the empty-state must say there are no NEW memories in the last hour, not that nothing is stored (#2358)"
grep -qF 'No memories stored yet' <<<"$HTML" \
  || fail "when total is zero the empty-state must still fall back to the truthful 'No memories stored yet' copy (#2358)"

# ── Stored total is humanized with thousands separators ──────────────────────
grep -qF "(d.total||0).toLocaleString()+' total'" <<<"$HTML" \
  || fail "the recent-memories stored total must be humanized via toLocaleString (#2358)"

echo "[mem-honesty] PASS: Memory tab surfaces the live stored total and no longer claims memory is empty when it is not (#2358)"
