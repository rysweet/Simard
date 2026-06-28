#!/usr/bin/env bash
# dashboard-p3-units-and-scales.sh — outside-in check for issue #2358 (P3).
#
# The live Playwright audit found unfriendly units and unexplained raw numbers
# that survived the P1/P2 humanization landed by #2373:
#
#   * Memory growth interval rendered a bare minute count: "since prev sample
#     (624m)" instead of a human duration like "10h 24m".
#   * Bare 0-1 urgency floats with no scale: "urgency 0.50" (Overview) and
#     "(urgency: 0.50)" (Thinking priorities).
#   * Brain Failures surfaced machine shorthand: "deterministic-brain:
#     prefix-routed" and a bare "Decision: consolidate_memory (confidence: 50%)".
#
# This script boots the real dashboard binary, logs in, and asserts — against
# the served HTML and the live /api/brain-failures response — that the P3 fixes
# are present and the bare/jargon forms are gone.
set -euo pipefail

PORT="${DASH_PORT:-8147}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-p3.XXXXXX.log)"
CJ="$(mktemp -t dash-p3-cookies.XXXXXX)"

echo "[p3] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[p3] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[p3] starting dashboard on :$PORT"
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
  echo "[p3] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[p3] FAIL: login rejected" >&2
  exit 1
fi

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"
BRAIN="$(curl -s -b "$CJ" "http://localhost:$PORT/api/brain-failures")"

fail() { echo "[p3] FAIL: $1" >&2; exit 1; }

# ── P3: memory growth interval is a human duration, not a bare minute count ──
grep -qF 'function humanizeDuration(' <<<"$HTML" \
  || fail "humanizeDuration must be defined so durations render as e.g. '10h 24m'"
grep -qF 'humanizeDuration(intervalSecs)' <<<"$HTML" \
  || fail "Memory growth interval must render via humanizeDuration"
grep -qF 'intervalMin' <<<"$HTML" \
  && fail "Memory growth interval still renders a bare minute count (intervalMin)"

# ── P3: urgency floats carry a qualitative word and an explicit 0-1 scale ────
grep -qF 'function urgencyPhrase(' <<<"$HTML" \
  || fail "urgencyPhrase must be defined so urgency scores explain their scale"
grep -qF 'urgencyPhrase(top.urgency)' <<<"$HTML" \
  || fail "Overview current focus must render urgency via urgencyPhrase"
grep -qF 'urgencyPhrase(p.urgency)' <<<"$HTML" \
  || fail "Thinking priorities must render urgency via urgencyPhrase"
grep -qF 'urgency ${top.urgency.toFixed(2)}' <<<"$HTML" \
  && fail "Overview still renders a bare unexplained urgency float"
grep -qF '(urgency: ${p.urgency.toFixed(2)})' <<<"$HTML" \
  && fail "Thinking priorities still render a bare unexplained urgency float"

# ── P3: Brain Failures never leak machine shorthand to the operator ──────────
grep -qF 'deterministic-brain: prefix-routed' <<<"$BRAIN" \
  && fail "Brain Failures rationale still leaks 'deterministic-brain: prefix-routed'"
grep -qF 'fallback-brain: prefix-routed' <<<"$BRAIN" \
  && fail "Brain Failures rationale still leaks 'fallback-brain: prefix-routed'"

echo "[p3] PASS: dashboard P3 unit/number humanization holds (#2358 P3)"
