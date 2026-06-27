#!/usr/bin/env bash
# dashboard-traces-cost-title-escaping.sh — outside-in check for issue #2351.
#
# The Traces tab's cost rows render an absolute timestamp into a *double-quoted*
# HTML `title` attribute:  '<span title="'+esc(abs)+'" …>'. The shared esc()
# helper escapes &<> (element-content safe) but NOT the double-quote character,
# so a quote-bearing `abs` would break out of the attribute. `formatTime` only
# returns its raw input on a parse failure, so the attribute is only safe if
# `abs` is guarded by a successful parse — exactly the guard renderGenericTrace
# already uses. PR #2345 left renderCostTrace assigning `abs` directly from
# formatTime on any truthy timestamp; #2351 hardens it to mirror the guard.
#
# This script boots the real dashboard binary, logs in, and asserts against the
# served HTML that renderCostTrace:
#   * normalises the timestamp up front via parseTs, and
#   * only calls formatTime when the parse succeeded (parsed ? … : ''), so the
#     raw-passthrough branch can never reach the double-quoted title attribute,
# while the readable cost-row contract from #1682/#2345 stays intact.
set -euo pipefail

PORT="${DASH_PORT:-8142}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-title-esc.XXXXXX.log)"
CJ="$(mktemp -t dash-title-esc-cookies.XXXXXX)"

echo "[title-esc] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[title-esc] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[title-esc] starting dashboard on :$PORT"
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
  echo "[title-esc] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[title-esc] FAIL: login rejected" >&2
  exit 1
fi

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"

fail() { echo "[title-esc] FAIL: $1" >&2; exit 1; }

# ── The cost renderer must exist and still feed `abs` into a quoted title ─────
grep -qF 'function renderCostTrace(data)' <<<"$HTML" \
  || fail "renderCostTrace(data) helper must exist"
grep -qF "'<span title=\"'+esc(abs)+'\"" <<<"$HTML" \
  || fail "cost rows must render the absolute time into a double-quoted title attribute fed by abs"

# ── #2351 guard: `abs` is parse-guarded, never assigned directly from a ───────
#    truthy-but-unparsed timestamp (which could carry a literal double-quote).
grep -qF 'const parsed=parseTs(data.timestamp)' <<<"$HTML" \
  || fail "renderCostTrace must normalise the timestamp via parseTs before computing abs (#2351)"
grep -qF "const abs=parsed?formatTime(data.timestamp):''" <<<"$HTML" \
  || fail "renderCostTrace must guard abs with the parse result so formatTime's raw-input passthrough cannot reach the title attribute (#2351)"
if grep -qF "const abs=data.timestamp?formatTime(data.timestamp)" <<<"$HTML"; then
  fail "renderCostTrace must NOT assign abs directly from formatTime on a truthy-but-unparsed timestamp — that is the #2351 attribute-injection gap"
fi

# ── Regression guard: the readable cost-row contract (#1682/#2345) is intact ──
grep -qF 'timeAgo(data.timestamp)' <<<"$HTML" \
  || fail "cost rows must still render relative time via timeAgo (#1682)"
grep -qF 'fmtCostUsd(data.cost_usd_est)' <<<"$HTML" \
  || fail "cost rows must still show the estimated dollar cost (#1682)"

echo "[title-esc] PASS: renderCostTrace title attribute is parse-guarded (#2351)"
