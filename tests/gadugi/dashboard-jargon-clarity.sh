#!/usr/bin/env bash
# dashboard-jargon-clarity.sh — outside-in check for issue #2358 (P1/P2).
#
# The live Playwright audit found machine jargon and contradictory copy across
# the Memory, Costs, Overview, Goals and Thinking tabs. This script boots the
# real dashboard binary, logs in, and asserts — against the served HTML and the
# live /api/memory/recent response — that the P1/P2 fixes are present:
#
#   * Costs lede no longer claims "real provider invoices rather than estimates"
#     while labeling the metric "Estimated Cost".
#   * The cycle/summary humanizer (humanizeCycleSummary) and the token
#     humanizers (humanizeGoalId, humanizePeriod) are wired in, and the
#     BANNED_JARGON list is injected so the ban extends to rendered summaries.
#   * The Memory "What Simard Remembers" counter reflects total stored memory
#     and /api/memory/recent reports a real total instead of a hard-coded 0.
#   * The Goals "Current Activity" raw "[raw]" marker is gone.
set -euo pipefail

PORT="${DASH_PORT:-8139}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-jargon.XXXXXX.log)"
CJ="$(mktemp -t dash-jargon-cookies.XXXXXX)"

echo "[jargon] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[jargon] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[jargon] starting dashboard on :$PORT"
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
  echo "[jargon] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[jargon] FAIL: login rejected" >&2
  exit 1
fi

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"
RECENT="$(curl -s -b "$CJ" "http://localhost:$PORT/api/memory/recent")"

fail() { echo "[jargon] FAIL: $1" >&2; exit 1; }

# ── P1: Costs label vs lede consistency ──────────────────────────────────────
grep -qF 'real provider invoices rather than estimates' <<<"$HTML" \
  && fail "Costs lede still claims invoice-derived figures while labeling them Estimated"
grep -qF 'the dollar figures are estimates based on' <<<"$HTML" \
  || fail "Costs lede must state the figures are estimates"

# ── P2: cycle/summary + token humanizers are wired in ────────────────────────
grep -qF 'function humanizeCycleSummary(' <<<"$HTML" \
  || fail "humanizeCycleSummary must be defined so OODA/key=value never leak"
grep -qF 'humanizeCycleSummary(rpt.summary)' <<<"$HTML" \
  || fail "Thinking tab must humanize the cycle summary"
grep -qF 'function humanizeGoalId(' <<<"$HTML" \
  || fail "humanizeGoalId must map sentinel ids like __memory__"
grep -qF 'humanizeGoalId(top.goal_id)' <<<"$HTML" \
  || fail "Overview top priority must humanize the goal id"
grep -qF 'function humanizePeriod(' <<<"$HTML" \
  || fail "humanizePeriod must humanize daily:/weekly: period keys"
grep -qF 'const BANNED_JARGON=' <<<"$HTML" \
  || fail "BANNED_JARGON must be injected so the ban extends to rendered summaries"

# ── P2: Goals 'Current Activity' raw marker is gone ──────────────────────────
grep -qF "'[raw]'" <<<"$HTML" \
  && fail "Goals Current Activity must not render the literal [raw] marker"

# ── P1: Memory panel surfaces the live stored total, never claims empty ──────
grep -qF "(d.total||0).toLocaleString()+' total'" <<<"$HTML" \
  || fail "Memory panel must surface the humanized stored total (#2358)"
grep -qF 'No new memories in the last hour' <<<"$HTML" \
  || fail "Memory empty-state must distinguish last-hour from total stored"
grep -q '"total"' <<<"$RECENT" \
  || fail "/api/memory/recent must expose a total stored count"

# ── P2 item 3: Overview raw brain-action-detail is humanized ─────────────────
grep -qF 'function humanizeActionDetail(' <<<"$HTML" \
  || fail "humanizeActionDetail must be defined so Overview action details stop leaking raw brain strings"
grep -qF 'esc(humanizeActionDetail(o.detail).substring(0,120))' <<<"$HTML" \
  || fail "Last Cycle Actions must humanize o.detail before the terminal esc() (escape-last)"
grep -qF 'humanizeActionDetail(a.detail' <<<"$HTML" \
  || fail "Recent actions must humanize a.detail before renderActionDetail()"
grep -qF 'esc(o.detail.substring(0,120))' <<<"$HTML" \
  && fail "Last Cycle Actions still renders the raw, un-humanized detail string"
grep -qF 'renderActionDetail(esc(' <<<"$HTML" \
  && fail "Recent actions must not double-escape before renderActionDetail()"

echo "[jargon] PASS: dashboard jargon/clarity fixes hold (#2358 P1/P2)"
