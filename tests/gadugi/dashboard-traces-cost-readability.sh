#!/usr/bin/env bash
# dashboard-traces-cost-readability.sh — outside-in check for issue #1682.
#
# The Traces tab used to render every cost-ledger entry as three indent-padded
# lines: the literal token "[cost]", a raw ISO-8601 timestamp, and the bare
# adapter brand ("copilot") — no cost amount, no model context, no token
# counts, and no per-call attribution. An operator triaging cost burn-rate
# learned nothing from the list.
#
# This script boots the real dashboard binary, logs in, and asserts — against
# the served HTML and the live /api/traces response — that cost rows now route
# through a dedicated readable renderer that surfaces When (relative + absolute
# time), What (call type / model / tokens / dollar cost), and Who (call context
# + session id), and that the old opaque rendering is gone.
set -euo pipefail

PORT="${DASH_PORT:-8141}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-traces.XXXXXX.log)"
CJ="$(mktemp -t dash-traces-cookies.XXXXXX)"

echo "[traces] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[traces] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[traces] starting dashboard on :$PORT"
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
  echo "[traces] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[traces] FAIL: login rejected" >&2
  exit 1
fi

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"
TRACES="$(curl -s -b "$CJ" "http://localhost:$PORT/api/traces")"

fail() { echo "[traces] FAIL: $1" >&2; exit 1; }

# ── Served HTML: cost rows route through the dedicated readable renderer ──────
grep -qF "s.source==='cost'?renderCostTrace(s.data):renderGenericTrace(s)" <<<"$HTML" \
  || fail "fetchTraces must dispatch cost spans to renderCostTrace (#1682)"
grep -qF 'function renderCostTrace(data)' <<<"$HTML" \
  || fail "renderCostTrace(data) helper must exist"

# ── When: relative + absolute time via the shared helpers (PR #1677) ─────────
grep -qF 'timeAgo(data.timestamp)' <<<"$HTML" \
  || fail "cost rows must render relative time via timeAgo (#1682)"
grep -qF 'formatTime(data.timestamp)' <<<"$HTML" \
  || fail "cost rows must expose the absolute timestamp via formatTime (#1682)"

# ── What: cost amount, model label, and token counts ─────────────────────────
grep -qF 'fmtCostUsd(data.cost_usd_est)' <<<"$HTML" \
  || fail "cost rows must show the estimated dollar cost (#1682)"
grep -qF 'costModelLabel(model)' <<<"$HTML" \
  || fail "cost rows must map the model token to a plain-language label (#1682)"
grep -qF "'copilot':'Copilot SDK call'" <<<"$HTML" \
  || fail "costModelLabel must humanise the bare 'copilot' adapter brand (#1682)"
grep -qF 'prompt_tokens_est' <<<"$HTML" \
  || fail "cost rows must show prompt token counts (#1682)"
grep -qF 'completion_tokens_est' <<<"$HTML" \
  || fail "cost rows must show completion token counts (#1682)"

# ── Who: per-call attribution (context + session id) ─────────────────────────
grep -qF 'data.context' <<<"$HTML" \
  || fail "cost rows must surface the call context for attribution (#1682)"
grep -qF 'shortSession(data.session_id)' <<<"$HTML" \
  || fail "cost rows must surface a shortened session id for attribution (#1682)"

# ── Live API: the source of truth the renderer reads ─────────────────────────
grep -q '"span_count"' <<<"$TRACES" \
  || fail "/api/traces must expose span_count"
if grep -q '"source":"cost"' <<<"$TRACES"; then
  grep -q '"cost_usd_est"' <<<"$TRACES" \
    || fail "cost-source spans must carry cost_usd_est for the renderer (#1682)"
  echo "[traces] live cost spans present and carry cost_usd_est"
else
  echo "[traces] note: no cost spans in the live ledger right now — HTML contract still asserted"
fi

echo "[traces] PASS: Traces-tab cost rows are human-readable (#1682)"
