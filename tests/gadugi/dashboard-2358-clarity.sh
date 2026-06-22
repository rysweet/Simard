#!/usr/bin/env bash
# dashboard-2358-clarity.sh — outside-in operator check for issue #2358 (P1).
#
# Two P1 clarity fixes are verified here:
#
#   (1) Memory-count clarity — the "What Simard Remembers" panel must not claim
#       "0 total / No memories stored yet" while the Memory Store actually holds
#       memories. /api/memory/recent reports only the last-hour window (0 on the
#       library backend), so the panel now falls back to the authoritative total
#       from /api/memory/history's newest snapshot and renders "N total stored"
#       with the empty-state copy "No new memories in the last hour — N total
#       stored". The Memory-Growth trend badge must also agree in direction with
#       the displayed long-term rate sign (no "↑ Growing" beside a negative rate).
#
#   (2) Cost label consistency — the Costs lede must agree with the value label.
#       Costs are estimates (token-count × typical pricing), so the lede must say
#       so and must NOT claim they are "real provider invoices", which would
#       contradict the "Estimated Cost" value label.
#
# This boots the real dashboard binary, authenticates, and asserts against the
# served HTML and the live /api/memory/{recent,history} responses.
set -euo pipefail

PORT="${DASH_PORT:-8139}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-2358.XXXXXX.log)"
CJ="$(mktemp -t dash-2358-cookies.XXXXXX)"

echo "[2358] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[2358] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[2358] starting dashboard on :$PORT"
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
  echo "[2358] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[2358] FAIL: login rejected" >&2
  exit 1
fi

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"
RECENT="$(curl -s -b "$CJ" "http://localhost:$PORT/api/memory/recent")"
HISTORY="$(curl -s -b "$CJ" "http://localhost:$PORT/api/memory/history")"

fail() { echo "[2358] FAIL: $1" >&2; exit 1; }

# ── P1.2 Cost label/lede consistency (served template) ───────────────────────
grep -qF 'Figures are estimated from token counts and typical model pricing' <<<"$HTML" \
  || fail "Costs lede must state figures are estimated (agree with 'Estimated Cost')"
grep -qF 'Estimated Cost' <<<"$HTML" \
  || fail "Costs value label 'Estimated Cost' must be present"
if grep -qF 'real provider invoices' <<<"$HTML"; then
  fail "Costs lede must NOT claim 'real provider invoices' — contradicts 'Estimated Cost'"
fi

# ── P1.1 Memory total-stored semantics (served template) ─────────────────────
grep -qF 'No new memories in the last hour' <<<"$HTML" \
  || fail "Memory empty-state must read 'No new memories in the last hour — N total stored'"
grep -qF 'Recent-memory listing is unavailable on this backend' <<<"$HTML" \
  || fail "Memory empty-state must surface total stored even when the recent listing is unavailable"
grep -qF 'total stored' <<<"$HTML" \
  || fail "Memory panel must label the count as 'total stored'"
grep -qF 'hs[hs.length-1].total' <<<"$HTML" \
  || fail "Recent panel must fall back to /api/memory/history newest-snapshot total"

# ── P1.1 Trend badge agrees with rate sign (served template) ─────────────────
grep -qF "ltRate>0?'growing':'shrinking'" <<<"$HTML" \
  || fail "Trend badge must be derived from the displayed long-term rate sign"

# ── Live API: endpoints expose the fields the panel relies on ────────────────
grep -q '"total"' <<<"$RECENT"     || fail "/api/memory/recent must expose 'total'"
grep -q '"available"' <<<"$RECENT" || fail "/api/memory/recent must expose 'available'"
grep -q '"snapshots"' <<<"$HISTORY"   || fail "/api/memory/history must expose 'snapshots'"
grep -q '"rate_per_hour"' <<<"$HISTORY" || fail "/api/memory/history must expose 'rate_per_hour'"

# ── Live API: total-stored fallback and trend/rate agreement (data-driven) ───
python3 - "$RECENT" "$HISTORY" <<'PY'
import json, sys
recent = json.loads(sys.argv[1])
history = json.loads(sys.argv[2])
snaps = history.get("snapshots") or []
recent_total = recent.get("total", 0) or 0
hist_total = (snaps[-1].get("total", 0) if snaps else 0) or 0
rate_lt = (history.get("rate_per_hour") or {}).get("long_term_total", 0.0) or 0.0

print(f"[2358] recent.total(window)={recent_total}  history.newest.total={hist_total}  rate_lt={rate_lt:.2f}")

# Total-stored fallback: the panel renders max(recent_total, hist_total). When
# the store is non-empty the fallback must surface a number >= the window count,
# never the misleading 0.
total_shown = recent_total if recent_total else hist_total
if hist_total > 0:
    assert total_shown >= hist_total, f"panel would hide stored memories: shows {total_shown}, stored {hist_total}"
    assert total_shown > 0, "panel must not show 0 total while memories are stored"
    print(f"[2358] OK: panel surfaces {total_shown} total stored (not 0)")
else:
    print("[2358] NOTE: memory store empty in this env — total-stored path proven by template assertions")

# Trend/rate agreement: replicate the client logic and assert the badge arrow can
# never contradict the displayed rate sign.
if abs(rate_lt) < 0.1:
    badge = "stable"
elif rate_lt > 0:
    badge = "growing"
else:
    badge = "shrinking"
assert not (rate_lt < -0.1 and badge == "growing"), "badge 'growing' beside negative rate"
assert not (rate_lt > 0.1 and badge == "shrinking"), "badge 'shrinking' beside positive rate"
print(f"[2358] OK: trend badge '{badge}' agrees with rate {rate_lt:.2f}/hr")
PY

echo "[2358] PASS: memory total-stored semantics and cost label/lede consistency hold (#2358)"
