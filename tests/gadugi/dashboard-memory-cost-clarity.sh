#!/usr/bin/env bash
# dashboard-memory-cost-clarity.sh — outside-in check for issue #2358 (P1).
#
# Two P1 clarity fixes are asserted against the real dashboard binary's served
# HTML (the same HTML the operator's browser receives, JS included):
#
#   P1a — Memory tab false-empty. The "What Simard Remembers" headline must
#         reflect TOTAL stored memory (not just the recent-window count, which
#         is 0 on the library backend), the empty-recent-window copy must read
#         "No new memories in the last hour — N total stored" instead of
#         "nothing stored, ever", and the Memory-Growth trend arrow must be
#         derived from the displayed per-hour rate sign so "↑ Growing" never
#         renders next to a negative rate.
#
#   P1b — Cost label contradiction. The cost figure is an estimate (see
#         src/cost_tracking.rs: char/4 token heuristic + default per-token
#         rates, all *_est fields). The label stays "Estimated Cost" and the
#         Costs lede must agree — it must NOT claim "real provider invoices".
#
# The script boots the real binary, authenticates with ~/.simard/.dashkey, and
# greps the served HTML. It is hermetic: it starts its own daemon on an
# ephemeral port and tears it down on exit.
set -euo pipefail

PORT="${DASH_PORT:-8141}"
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
cleanup() { kill "$DASH_PID" 2>/dev/null || true; rm -f "$CJ" "$LOG"; }
trap cleanup EXIT

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

# ── P1a: Memory headline reflects total stored, not the recent window ────────
grep -qF 'items remembered<br>across all memory' <<<"$HTML" \
  || fail "memory headline caption must label the big counter as total stored (#2358)"
grep -qF 'new in the last hour' <<<"$HTML" \
  || fail "recent-window activity must be shown as a secondary count (#2358)"
grep -qF 'No new memories in the last hour' <<<"$HTML" \
  || fail "empty recent-window copy must say 'No new memories in the last hour' (#2358)"
grep -qF 'total stored' <<<"$HTML" \
  || fail "empty-state must report the stored total, not imply nothing is remembered (#2358)"
grep -qF "snaps[snaps.length-1].total" <<<"$HTML" \
  || fail "headline total must fall back to /api/memory/history when recent.total is 0 (#2358)"

# ── P1a: Memory-Growth trend arrow agrees with the displayed rate sign ───────
grep -qF "rph<=-0.1?'shrinking'" <<<"$HTML" \
  || fail "trend badge must be derived from the per-hour rate sign so it never disagrees (#2358)"

# ── P1b: Cost label and lede tell one consistent story (estimated) ───────────
grep -qF "'total_cost_usd':'Estimated Cost'" <<<"$HTML" \
  || fail "cost metric must keep the 'Estimated Cost' label (#2358)"
grep -qF 'estimates derived from token counts, not exact provider invoices' <<<"$HTML" \
  || fail "Costs lede must describe the figure as an estimate (#2358)"
grep -qF 'real provider invoices rather than estimates' <<<"$HTML" \
  && fail "Costs lede must not still claim figures come from real provider invoices (#2358)"

echo "[clarity] PASS: Memory headline + Cost label clarity holds (#2358 P1)"
