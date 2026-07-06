#!/usr/bin/env bash
# dashboard-goals-lifecycle.sh — outside-in qa-team check for the Goals tab
# lifecycle-status fix (issue #20).
#
# BUG: the operator ran `simard goal list` (ground truth) and saw the 20 active
# goals in MIXED states — several `blocked` (with an OODA-safeguard "needs human
# review" reason), many `not-started`, and several `completed`. But the
# dashboard "Goals" tab rendered EVERY goal as failed/blocked, because the
# Status column dumped the raw free-form status string which — paired with the
# prominent red activity chip in the Current Activity column — read as "failed"
# for every row.
#
# FIX: `/api/goals` additively exposes a serialized `GoalProgress` enum
# (`status_progress`) per active goal, and the Status column renders a
# distinctly-colored lifecycle badge via `humanizeGoalProgress(g.status_progress)`
# keyed off a `goalLifecycleKey()` variant classifier and a hardcoded
# `GOAL_STATUS_COLORS` allowlist. Blocked uses amber (#d29922), deliberately
# distinct from the activity-Failed red (#f85149), and a blocked goal surfaces
# its reason ("Blocked — <reason>").
#
# This script boots the real dashboard binary, logs in with the dashkey, and
# asserts — against the served HTML — that the lifecycle-badge machinery is
# wired in and the old raw status-string dump is gone.
set -euo pipefail

PORT="${DASH_PORT:-8143}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-goals-lifecycle.XXXXXX.log)"
CJ="$(mktemp -t dash-goals-lifecycle-cookies.XXXXXX)"

echo "[goals-lifecycle] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[goals-lifecycle] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[goals-lifecycle] starting dashboard on :$PORT"
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
  echo "[goals-lifecycle] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[goals-lifecycle] FAIL: login rejected" >&2
  exit 1
fi

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"

fail() { echo "[goals-lifecycle] FAIL: $1" >&2; exit 1; }

# ── The Status column renders a lifecycle badge from the serialized enum ──────
grep -qF 'humanizeGoalProgress(g.status_progress)' <<<"$HTML" \
  || fail "the Status column must render humanizeGoalProgress(g.status_progress), \
not a uniform raw status dump"
# escape-last: humanize the RAW enum, never already-escaped text.
grep -qF 'humanizeGoalProgress(esc(' <<<"$HTML" \
  && fail "humanizeGoalProgress must run on the raw enum, not escaped text (escape-last)"
# The old raw-status cell that made every goal look failed/blocked must be gone.
grep -qF '<td>${esc(g.status)}</td>' <<<"$HTML" \
  && fail "the Status column must no longer dump the raw free-form g.status string"

# ── Variant classifier + hardcoded color allowlist (G3, no style injection) ──
grep -qF 'function goalLifecycleKey(' <<<"$HTML" \
  || fail "goalLifecycleKey() must classify the GoalProgress enum by variant"
grep -qF 'GOAL_STATUS_COLORS' <<<"$HTML" \
  || fail "a hardcoded GOAL_STATUS_COLORS allowlist must drive the badge color"

# ── Blocked lifecycle badge is amber, DISTINCT from the activity-Failed red ──
grep -qF '#d29922' <<<"$HTML" \
  || fail "the blocked lifecycle badge must use amber #d29922"
grep -qE "[Bb]locked:'#f85149'" <<<"$HTML" \
  && fail "the blocked lifecycle badge must NOT reuse the activity-Failed red #f85149"

# ── A blocked goal surfaces its REASON (not a bare 'failed') ─────────────────
grep -qF "'Blocked — '+r" <<<"$HTML" \
  || fail "a blocked goal must render its reason as 'Blocked — <reason>'"

echo "[goals-lifecycle] PASS: Goals tab renders each goal's real lifecycle status"
