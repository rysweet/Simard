#!/usr/bin/env bash
# qa-dashboard-memory-recent-items.sh
#
# End-to-end check for the dashboard Memory tab "Recent Memories" panel.
#
# The panel (`GET /api/memory/recent` → `items[]`, rendered by
# `fetchRecentMemories` in index_html/part_03.rs as the plain-English #1997
# view) must list the newest episodic memories. Before the fix it was a stale
# stub that ALWAYS returned `items: []` with `available: false` and a note
# claiming "per-item recent-memory listing is unavailable on the library
# backend" — even though the very same shared reader that backs
# `/api/memory/graph` enumerates episodes via `list_all_episodes` (newest-first).
# So the panel could never answer "what has Simard been remembering recently?"
# while thousands of episodes were held.
#
# This script seeds three known episodes into a hermetic cognitive store (via
# `simard memory import`), serves a standalone dashboard against that store, and
# asserts over HTTP that `/api/memory/recent`:
#   * reports `available: true`,
#   * lists exactly the three seeded episodes,
#   * orders them newest-first (the last-imported episode is item 0),
#   * tags each item with the frontend's "Past event" category,
#   * carries a non-null, parseable RFC3339 `timestamp` for each item so the
#     frontend's `timeAgo()` can render a "time ago" label (issue #4383 — the
#     timestamp used to be structurally `null` for every item even though the
#     library records a real `created_at`).
# Any regression back to the empty-stub behaviour, or back to the always-null
# timestamp, makes this script `exit 1`, which the `gadugi-test` cli agent
# treats as a hard step failure.
set -uo pipefail

PORT="${QA_DASHBOARD_PORT:-18844}"
TOKEN="qa-recent-$$"
ROOT="$(mktemp -d "${TMPDIR:-/tmp}/qa-dash-recent-XXXXXX")"
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
  echo "QA-DASHBOARD-RECENT: FAIL - $1"
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

# Seed three distinct episodes (temporal_index ascending → import order is the
# recency order the store preserves; gamma is newest).
cat > "$ROOT/snap.json" <<'JSON'
{
  "facts": [],
  "procedures": [],
  "episodes": [
    {"node_id":"e1","content":"alpha episode content","source_label":"qa","temporal_index":1,"compressed":false},
    {"node_id":"e2","content":"beta episode content","source_label":"qa","temporal_index":2,"compressed":false},
    {"node_id":"e3","content":"gamma episode content","source_label":"qa","temporal_index":3,"compressed":false}
  ],
  "prospective": [],
  "exported_at": 1700000000,
  "source_agent": "qa"
}
JSON

SIMARD_STATE_ROOT="$ROOT" "$BIN" memory import "$ROOT/snap.json" "$ROOT" >"$ROOT/import.log" 2>&1 \
  || fail "memory import failed: $(tail -5 "$ROOT/import.log")"

# Start a standalone dashboard against the hermetic state root. HOME is pinned
# into the temp root so the login-code file (~/.simard/.dashkey) stays hermetic.
# SIMARD_DASHBOARD_TOKEN is the API bearer token used by the curl calls.
HOME="$ROOT" SIMARD_DASHBOARD_TOKEN="$TOKEN" SIMARD_STATE_ROOT="$ROOT" \
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

AUTH=(-H "Authorization: Bearer $TOKEN")

resp="$(curl -fsS "${AUTH[@]}" "http://127.0.0.1:$PORT/api/memory/recent")" \
  || fail "GET /api/memory/recent failed"

# Assert the whole contract in one Python pass; print PASS or exit 1 with detail.
# The response is passed via env (QA_RESP) because the heredoc already occupies
# stdin (so it cannot also be piped in).
QA_RESP="$resp" python3 <<'PY' || exit 1
import json, os, sys
from datetime import datetime, timezone

raw = os.environ["QA_RESP"]
d = json.loads(raw)

def bail(msg):
    print(f"QA-DASHBOARD-RECENT: FAIL - {msg}. Response: {raw}")
    sys.exit(1)

if d.get("available") is not True:
    bail("`available` must be true once episodes exist (regressed to empty stub?)")

items = d.get("items")
if not isinstance(items, list):
    bail("`items` must be a JSON array")
if len(items) != 3:
    bail(f"expected exactly 3 seeded episodes, got {len(items)}")

summaries = [i.get("summary") for i in items]
if summaries != ["gamma episode content", "beta episode content", "alpha episode content"]:
    bail(f"episodes must be newest-first (gamma, beta, alpha); got {summaries}")

for i in items:
    if i.get("category") != "Past event":
        bail(f"each item must use the 'Past event' category; got {i.get('category')}")
    if "timestamp" not in i:
        bail("each item must carry a `timestamp` key (string or null)")
    # Issue #4383: the timestamp must now be the episode's real created_at,
    # surfaced as a parseable RFC3339 string at or near "now" — never the old
    # structural null, and never a fabricated 1970s epoch.
    ts = i.get("timestamp")
    if not isinstance(ts, str) or not ts:
        bail(f"each item must carry a non-null RFC3339 `timestamp` (issue #4383); got {ts!r}")
    norm = ts.replace("Z", "+00:00")
    try:
        parsed = datetime.fromisoformat(norm)
    except ValueError:
        bail(f"`timestamp` must be parseable RFC3339; got {ts!r}")
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    age = (datetime.now(timezone.utc) - parsed).total_seconds()
    if not (-5 <= age <= 3600):
        bail(f"`timestamp` must be a recent wall-clock instant (age={age:.0f}s); got {ts!r}")

print("QA-DASHBOARD-RECENT: PASS - /api/memory/recent lists the 3 seeded "
      "episodes newest-first with available:true, 'Past event' category, and a "
      "non-null RFC3339 timestamp per item (issue #4383)")
PY
