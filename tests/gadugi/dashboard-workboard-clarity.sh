#!/usr/bin/env bash
# dashboard-workboard-clarity.sh — outside-in qa-team check for the Workboard
# de-jargon pass (PR #2552 findings #4 + #5).
#
# A live Playwright audit of the Workboard tab flagged two machine-jargon
# offenders that leaked onto the page:
#   #4  "Task Memory" rendered raw goal-board JSON blobs, e.g.
#       {"active":[{"id":…,"status":{"InProgress":{"percent":5}}}]} — exposing
#       the raw GoalProgress enum ("InProgress").
#   #5  "Recent Actions" showed the raw daemon result string, e.g.
#       `brain: continue_skipping (recipe-engineer-lifecycle-brain: no decision
#       keyword found…)`.
#
# This script boots the real dashboard binary, logs in with the dashkey, and
# asserts — against the served HTML — that the plain-English humanizers are
# wired in and the raw-render code paths are gone, while the stable structural
# hooks (the #wb-actions / #wb-facts-list slots) and the escAttr()-hardened
# title= tooltips that carry the raw machine values for power users stay intact
# (so the structural e2e contract keeps passing and nobody loses information).
set -euo pipefail

PORT="${DASH_PORT:-8142}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-workboard.XXXXXX.log)"
CJ="$(mktemp -t dash-workboard-cookies.XXXXXX)"

echo "[workboard] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[workboard] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[workboard] starting dashboard on :$PORT"
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
  echo "[workboard] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[workboard] FAIL: login rejected" >&2
  exit 1
fi

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"

fail() { echo "[workboard] FAIL: $1" >&2; exit 1; }

# ── #4 Task Memory: plain-English humanizers are wired in ────────────────────
grep -qF 'function humanizeTaskMemory(' <<<"$HTML" \
  || fail "humanizeTaskMemory must parse goal-board JSON blobs into plain text"
grep -qF 'function humanizeGoalProgress(' <<<"$HTML" \
  || fail "humanizeGoalProgress must map the raw GoalProgress enum to plain text"
# The raw InProgress struct-variant enum must become a plain phrase.
grep -qF "'In progress — '+status.InProgress.percent+'%'" <<<"$HTML" \
  || fail "the raw InProgress enum must render as 'In progress — N%'"
# Task Memory content must be humanized then escaped (escape-last).
grep -qF 'const humanizedContent=humanizeTaskMemory(rawContent)' <<<"$HTML" \
  || fail "Task Memory must pipe the fact content through humanizeTaskMemory"
grep -qF 'esc(humanizedContent.substring(0,200))' <<<"$HTML" \
  || fail "Task Memory must esc() the humanized content as the terminal op"
# The old raw-JSON render path must be gone.
grep -qF "esc((f.content||'').substring(0,200))" <<<"$HTML" \
  && fail "Task Memory must no longer render the raw fact content directly"

# ── #5 Recent Actions: raw brain result is humanized ─────────────────────────
grep -qF 'renderActionDetail(humanizeActionDetail(a.result))' <<<"$HTML" \
  || fail "Workboard Recent Actions must humanize a.result via humanizeActionDetail"
grep -qF '<span style="flex:1">${renderActionDetail(a.result)}</span>' <<<"$HTML" \
  && fail "Workboard Recent Actions must no longer render the raw a.result string"

# ── Raw machine values survive as escAttr()-hardened title= tooltips ─────────
grep -qF 'title="${escAttr(a.result||' <<<"$HTML" \
  || fail "raw a.result must survive as an escAttr() title= tooltip (power users)"
grep -qF "' title=\"'+escAttr(rawContent)+'\"'" <<<"$HTML" \
  || fail "raw Task Memory content must survive as an escAttr() title= tooltip"

# ── Stable structural hooks remain (structural e2e contract) ─────────────────
grep -qF "getElementById('wb-actions')" <<<"$HTML" \
  || fail "the #wb-actions slot must remain for the structural workboard spec"
grep -qF "getElementById('wb-facts-list')" <<<"$HTML" \
  || fail "the #wb-facts-list slot must remain for the structural workboard spec"

echo "[workboard] PASS: Workboard Task Memory + Recent Actions are jargon-free"
