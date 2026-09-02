#!/usr/bin/env bash
# dashboard-workboard-blocked-reason.sh — outside-in qa-team check for the
# Workboard 'Blocked' column fix (issue #4178).
#
# BUG: the Workboard kanban card (`wbGoalCard`) colored a lifecycle-BLOCKED
# goal's progress bar with `var(--red)` (#f85149) — the SAME activity-failure
# red that issue #20 deliberately reserved for failures — so a blocked goal on
# the Workboard read as *failed*. The card also never surfaced WHY a goal was
# blocked: the reason is carried in the additive `block_reason` field and, for
# back-compat, jammed into the legacy `status` Display string, but the frontend
# dropped it.
#
# FIX: the blocked bar uses amber `var(--yellow)` (#d29922), matching the
# Goals-tab GOAL_STATUS_COLORS decision (blocked != failed), and the card
# renders `Blocked — <reason>` from `g.block_reason` (falling back to the
# legacy prefix-stripped `status`). `/api/workboard` additively emits a clean,
# prefix-free `block_reason` for blocked goals.
#
# This script boots the real dashboard binary, logs in with the dashkey, and
# asserts — against the served HTML — that the Workboard blocked branch no
# longer reuses the failure red, uses amber instead, and surfaces the reason.
set -euo pipefail

PORT="${DASH_PORT:-8144}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-wb-blocked.XXXXXX.log)"
CJ="$(mktemp -t dash-wb-blocked-cookies.XXXXXX)"

echo "[wb-blocked] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[wb-blocked] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[wb-blocked] starting dashboard on :$PORT"
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
  echo "[wb-blocked] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[wb-blocked] FAIL: login rejected" >&2
  exit 1
fi

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"

fail() { echo "[wb-blocked] FAIL: $1" >&2; exit 1; }

# ── The Workboard blocked bar must NOT reuse the activity-failure red ────────
grep -qF "g.status.startsWith('blocked')?'var(--red)'" <<<"$HTML" \
  && fail "the Workboard blocked progress bar must not reuse the activity-failure red var(--red)"

# ── It must use amber var(--yellow) instead (blocked != failed, per issue #20) ─
grep -qF "isBlocked?'var(--yellow)'" <<<"$HTML" \
  || fail "the Workboard blocked progress bar must use amber var(--yellow)"

# ── The card must surface WHY blocked from the additive clean field ──────────
grep -qF 'g.block_reason' <<<"$HTML" \
  || fail "the Workboard card must surface a blocked goal's reason from g.block_reason"
grep -qF '<strong>Blocked — </strong>${esc(reason)}' <<<"$HTML" \
  || fail "the Workboard blocked card must render an escaped 'Blocked — <reason>' row"

echo "[wb-blocked] PASS: Workboard renders blocked goals amber (not failed) with a visible reason"
