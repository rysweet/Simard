#!/usr/bin/env bash
# qa-dashboard-goal-persistence.sh
#
# End-to-end check for the standalone dashboard goal-board persistence fix:
# `simard dashboard serve` must persist goals across multiple HTTP requests.
#
# End-to-end persistence regression for the goal-board: a multi-request
# seed -> add -> promote -> read flow against a standalone dashboard must never
# lose a goal. The silent data-loss class (#1590 / #2320) came from per-request
# fresh `LibraryCognitiveMemory` opens racing the lbug store's exclusive
# per-handle lock: a reopen could read an empty board and the next mutating
# request would persist that empty board, dropping every goal. That race is now
# prevented by the launcher's tier-2 store cache (#2334) and by `serve()`
# registering one shared tier-0 handle (mirroring the OODA daemon and
# bootstrap); this script guards the end-to-end persistence contract across
# separate, hermetic HTTP requests.
set -uo pipefail

PORT="${QA_DASHBOARD_PORT:-18842}"
KEY="qa-dashkey-$$"
ROOT="$(mktemp -d "${TMPDIR:-/tmp}/qa-dash-XXXXXX")"
LOG="$ROOT/serve.log"
SERVE_PID=""

cleanup() {
  if [[ -n "$SERVE_PID" ]]; then
    kill "$SERVE_PID" 2>/dev/null || true
  fi
  rm -rf "$ROOT" 2>/dev/null || true
}
trap cleanup EXIT

fail() {
  echo "QA-DASHBOARD-GOALS: FAIL - $1"
  [[ -f "$LOG" ]] && { echo "--- serve log tail ---"; tail -20 "$LOG"; }
  exit 1
}

# Locate the freshly-built binary via cargo's reported target directory so the
# script works regardless of CARGO_TARGET_DIR.
cargo build --quiet --bin simard || fail "cargo build failed"
TARGET_DIR="$(cargo metadata --no-deps --format-version 1 \
  | tr ',' '\n' | grep -o '"target_directory":"[^"]*"' | head -1 | cut -d'"' -f4)"
BIN="$TARGET_DIR/debug/simard"
[[ -x "$BIN" ]] || fail "simard binary not found at $BIN"

# Start a standalone dashboard against an isolated, hermetic state root.
# HOME is pinned into the temp root so the login-code file the dashboard writes
# (~/.simard/.dashkey) stays hermetic and never touches the operator's real
# $HOME. SIMARD_DASHBOARD_TOKEN is the API bearer token used by the curl calls.
HOME="$ROOT" SIMARD_DASHBOARD_TOKEN="$KEY" SIMARD_STATE_ROOT="$ROOT" \
  "$BIN" dashboard serve --port="$PORT" >"$LOG" 2>&1 &
SERVE_PID=$!

# Wait for the server to accept connections (the public /login page).
ready=""
for _ in $(seq 1 60); do
  if curl -fsS -o /dev/null "http://127.0.0.1:$PORT/login" 2>/dev/null; then
    ready="1"
    break
  fi
  kill -0 "$SERVE_PID" 2>/dev/null || fail "server exited before becoming ready"
  sleep 0.5
done
[[ -n "$ready" ]] || fail "server not ready on port $PORT"

AUTH=(-H "Authorization: Bearer $KEY" -H "Content-Type: application/json")
api() { curl -fsS "${AUTH[@]}" "$@"; }

# 1. Seed the canonical board (3 active, 2 backlog) through the HTTP handler.
api -X POST "http://127.0.0.1:$PORT/api/goals/seed" >/dev/null \
  || fail "seed request failed"

# 2. Add a backlog item (a separate mutating request).
add_resp="$(api -X POST "http://127.0.0.1:$PORT/api/goals" \
  -d '{"description":"QA end-to-end persistence probe","type":"backlog"}')" \
  || fail "add request failed"
id="$(printf '%s' "$add_resp" | grep -o '"id":"[^"]*"' | head -1 | cut -d'"' -f4)"
[[ -n "$id" ]] || fail "add did not return an id (response: $add_resp)"

# 3. Promote the new backlog item to active (another mutating request).
api -X POST "http://127.0.0.1:$PORT/api/goals/promote/$id" >/dev/null \
  || fail "promote request failed"

# 4. Final read on a SEPARATE request: the 3 seeded goals plus the promoted one
#    must all still be present. A racing fresh-open read used to return an empty
#    board here, after which a mutating handler persisted it and dropped goals.
final="$(api "http://127.0.0.1:$PORT/api/goals")" || fail "final read failed"
active_count="$(printf '%s' "$final" | grep -o '"active_count":[0-9]*' | head -1 | cut -d: -f2)"
[[ "$active_count" == "4" ]] \
  || fail "expected 4 active goals (3 seeded + 1 promoted), got '${active_count:-<none>}' (response: $final)"

echo "QA-DASHBOARD-GOALS: PASS - 4 active goals persisted across 4 HTTP requests"
