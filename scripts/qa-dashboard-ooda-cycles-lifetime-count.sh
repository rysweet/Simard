#!/usr/bin/env bash
# qa-dashboard-ooda-cycles-lifetime-count.sh
#
# End-to-end check for the dashboard Cycle History lifetime-count fix.
#
# The Cycle History tab (`GET /api/ooda-cycles`) renders a "cycles recorded"
# line. `total_cycles` is the number of cycle reports in the bounded scan
# window (capped at MAX_CYCLES = 50). Before the fix, that capped window size
# was rendered as the lifetime total, so an operator asking "how many OODA
# cycles have I run?" saw `50 cycles recorded` even when the daemon was on cycle
# #1800+ — contradicting System Status (`/api/status` → daemon_health.cycle_number),
# which the cycle_source single-source-of-truth module (#1680) governs.
#
# The fix adds `latest_cycle_number` — the authoritative cumulative cycle number
# (the highest persisted cycle-report index). This script seeds more cycle
# reports than the scan window against a standalone dashboard and asserts, over
# HTTP, that:
#   * total_cycles is capped at the 50-report window, AND
#   * latest_cycle_number reflects the true highest cycle index (60), AND
#   * latest_cycle_number > total_cycles (the lifetime count is never
#     undercounted to the capped window size).
set -uo pipefail

PORT="${QA_DASHBOARD_PORT:-18847}"
KEY="qa-dashkey-$$"
ROOT="$(mktemp -d "${TMPDIR:-/tmp}/qa-dash-cycles-XXXXXX")"
LOG="$ROOT/serve.log"
SERVE_PID=""
SEEDED_CYCLES=60
WINDOW_CAP=50

cleanup() {
  if [[ -n "$SERVE_PID" ]]; then
    kill "$SERVE_PID" 2>/dev/null || true
  fi
  rm -rf "$ROOT" 2>/dev/null || true
}
trap cleanup EXIT

fail() {
  echo "QA-DASHBOARD-CYCLES: FAIL - $1"
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

# Seed SEEDED_CYCLES cycle reports (cycle_1.json .. cycle_60.json), more than
# the 50-report scan window, so the capped window and the true lifetime count
# diverge. Each carries the minimal fields the endpoint reads.
python3 - "$ROOT/cycle_reports" "$SEEDED_CYCLES" <<'PY' || fail "seed cycle reports failed"
import json, os, sys
cdir, n = sys.argv[1], int(sys.argv[2])
os.makedirs(cdir, exist_ok=True)
for i in range(1, n + 1):
    body = {
        "cycle_number": i,
        "timestamp": f"2026-07-06T05:{i % 60:02d}:00Z",
        "duration_secs": 12.0,
        "summary": f"Cycle #{i} — 1 of 1 actions succeeded",
        "outcomes": [{
            "action_kind": "AdvanceGoal",
            "action_description": "opened PR",
            "success": True,
            "goal_id": f"g{i}",
        }],
    }
    with open(os.path.join(cdir, f"cycle_{i}.json"), "w") as f:
        json.dump(body, f)
PY

# Start a standalone dashboard against the hermetic state root. HOME is pinned
# into the temp root so the login-code file (~/.simard/.dashkey) stays hermetic.
# SIMARD_DASHBOARD_TOKEN is the API bearer token used by the curl calls.
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

resp="$(curl -fsS "${AUTH[@]}" "http://127.0.0.1:$PORT/api/ooda-cycles")" \
  || fail "GET /api/ooda-cycles failed"

read -r total latest <<<"$(printf '%s' "$resp" | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(d.get("total_cycles"), d.get("latest_cycle_number"))
' 2>/dev/null)" || fail "could not parse /api/ooda-cycles (response: $resp)"

[[ "$total" == "$WINDOW_CAP" ]] \
  || fail "expected total_cycles=$WINDOW_CAP (the capped scan window), got '${total:-<none>}'. Response: $resp"

[[ "$latest" == "$SEEDED_CYCLES" ]] \
  || fail "expected latest_cycle_number=$SEEDED_CYCLES (the highest persisted cycle index), got '${latest:-<none>}'. \
A value equal to total_cycles ($WINDOW_CAP) means the lifetime count was undercounted to the capped window. Response: $resp"

if ! [[ "$latest" -gt "$total" ]] 2>/dev/null; then
  fail "latest_cycle_number ($latest) must exceed total_cycles ($total) so the Cycle History tab reports the real lifetime count, not the capped window. Response: $resp"
fi

echo "QA-DASHBOARD-CYCLES: PASS - total_cycles=$total (capped window), latest_cycle_number=$latest (true lifetime); the tab reports the real cycle count, not the 50-report window"
