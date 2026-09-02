#!/usr/bin/env bash
# qa-dashboard-memory-record-count.sh
#
# End-to-end check for the dashboard Memory tab record-count fix (#4075).
#
# The Memory tab's "Memory records" tile (`GET /api/memory` →
# `memory_records.count`) must reflect the number of records in the on-disk
# `memory_records.json`, which the FileBackedMemoryStore persists as a
# checksummed envelope:
#
#     { "crc32": <u32>, "records": [ ... ] }
#
# Before the fix, `count_json_records` counted the object's two top-level keys
# (`crc32` + `records`) and reported `2` for a store holding thousands of
# records — silently misreporting memory health to the operator. This script
# seeds an envelope with a known record count against a standalone dashboard
# and asserts the reported count over HTTP matches, not `2`.
set -uo pipefail

PORT="${QA_DASHBOARD_PORT:-18843}"
KEY="qa-dashkey-$$"
ROOT="$(mktemp -d "${TMPDIR:-/tmp}/qa-dash-mem-XXXXXX")"
LOG="$ROOT/serve.log"
SERVE_PID=""
RECORD_COUNT=1244

cleanup() {
  if [[ -n "$SERVE_PID" ]]; then
    kill "$SERVE_PID" 2>/dev/null || true
  fi
  rm -rf "$ROOT" 2>/dev/null || true
}
trap cleanup EXIT

fail() {
  echo "QA-DASHBOARD-MEMCOUNT: FAIL - $1"
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

# Seed a checksummed-envelope memory_records.json with a known record count.
# The crc32 value is irrelevant to the count path (the dashboard only reads the
# `records` array length), so any placeholder integer is fine here.
python3 - "$ROOT/memory_records.json" "$RECORD_COUNT" <<'PY' || fail "seed memory_records.json failed"
import json, sys
path, n = sys.argv[1], int(sys.argv[2])
payload = {"crc32": 0, "records": [{"id": f"r{i}"} for i in range(n)]}
with open(path, "w") as f:
    json.dump(payload, f)
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

resp="$(curl -fsS "${AUTH[@]}" "http://127.0.0.1:$PORT/api/memory")" \
  || fail "GET /api/memory failed"

count="$(printf '%s' "$resp" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["memory_records"]["count"])' 2>/dev/null)" \
  || fail "could not parse memory_records.count (response: $resp)"

[[ "$count" == "$RECORD_COUNT" ]] \
  || fail "expected memory_records.count=$RECORD_COUNT (the records-array length), got '${count:-<none>}'. \
A value of 2 means the checksummed envelope keys were counted instead of the records (#4075). Response: $resp"

echo "QA-DASHBOARD-MEMCOUNT: PASS - memory_records.count=$count reflects the records array, not the envelope keys"
