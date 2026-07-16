#!/usr/bin/env bash
# qa-dashboard-memory-growth-rate-window.sh
#
# End-to-end check for the dashboard Memory tab growth-rate window fix (#4107).
#
# The Memory tab's growth panel shows a "long-term mem/hr" rate
# (`GET /api/memory/history` -> `rate_per_hour.long_term_total`) and derives the
# growth-trend badge from it. The rate must be a *recent-activity* signal:
# measured over a bounded trailing 24 h window ending at the newest sample, NOT
# averaged across the entire retained ring buffer (which spans weeks, including
# multi-day daemon-down gaps).
#
# Before the fix, `rate_per_hour` used `snapshots[0]` (oldest retained) and
# divided by the full multi-week span, so an active hour of memory formation was
# diluted to ~0/hr. This script seeds a history with an ANCIENT baseline plus
# recent in-window samples against a standalone dashboard and asserts, over
# HTTP, that the served rate:
#   1. equals an INDEPENDENT 24 h-windowed recomputation from the served
#      snapshots, and
#   2. DIFFERS from the naive whole-history rate (which the older sample makes
#      meaningfully smaller).
set -uo pipefail

PORT="${QA_DASHBOARD_PORT:-18847}"
KEY="qa-dashkey-$$"
ROOT="$(mktemp -d "${TMPDIR:-/tmp}/qa-dash-rate-XXXXXX")"
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
  echo "QA-DASHBOARD-RATE-WINDOW: FAIL - $1"
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

# Seed memory_history.json with three snapshots, anchored at "now" so the
# handler does not append a fresh live snapshot (append is due only after
# >=300s since the last entry; the newest seeded epoch == now):
#   * ancient   (now - 8 days)  long_term=100  -> OUTSIDE the 24 h window
#   * in-window (now - 1 hour)  long_term=200  -> the 24 h-window baseline
#   * newest    (now)           long_term=260
# Windowed rate = (260-200)/1h = 60/hr; naive whole-history rate
# = (260-100)/(8 days) ~= 0.83/hr. The two MUST differ.
python3 - "$ROOT/memory_history.json" <<'PY' || fail "seed memory_history.json failed"
import json, sys, time
path = sys.argv[1]
now = float(int(time.time()))
def snap(epoch, lt):
    return {
        "timestamp": "", "epoch_secs": epoch,
        "sensory": 0, "working": 0,
        "episodic": lt, "semantic": 0, "procedural": 0, "prospective": 0,
        "total": lt, "long_term_total": lt,
    }
history = [
    snap(now - 8 * 86400, 100),  # ancient, outside the window
    snap(now - 3600, 200),       # 1 h ago, inside the window (baseline)
    snap(now, 260),              # newest
]
with open(path, "w") as f:
    json.dump(history, f)
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

resp="$(curl -fsS "${AUTH[@]}" "http://127.0.0.1:$PORT/api/memory/history")" \
  || fail "GET /api/memory/history failed"
printf '%s' "$resp" > "$ROOT/history_resp.json"

# Independently recompute the 24 h-windowed rate AND the naive whole-history
# rate from the SERVED snapshots, then assert the served rate matches the
# windowed recompute and differs from the naive one. The response is passed via
# a file path (argv) so the heredoc can own stdin for the program text.
python3 - "$ROOT/history_resp.json" <<'PY' || exit 1
import json, sys

WINDOW = 86400.0
with open(sys.argv[1]) as f:
    d = json.load(f)

served_block = d.get("rate_per_hour")
if not isinstance(served_block, dict):
    print("QA-DASHBOARD-RATE-WINDOW: FAIL - rate_per_hour missing/non-object: "
          + json.dumps(d)[:400])
    sys.exit(1)
served = served_block.get("long_term_total")
if not isinstance(served, (int, float)):
    print("QA-DASHBOARD-RATE-WINDOW: FAIL - rate_per_hour.long_term_total not numeric: "
          + json.dumps(served_block))
    sys.exit(1)

snaps = d.get("snapshots") or []
if len(snaps) < 2:
    print("QA-DASHBOARD-RATE-WINDOW: FAIL - expected the seeded snapshots to be "
          "served, got %d" % len(snaps))
    sys.exit(1)

newest = snaps[-1]
oldest = snaps[0]

# Independent 24 h-windowed recompute: baseline = oldest snapshot at-or-after
# (newest_epoch - WINDOW), edge inclusive, excluding the newest itself.
window_start = newest["epoch_secs"] - WINDOW
baseline = None
for s in snaps[:-1]:
    if s["epoch_secs"] >= window_start:
        baseline = s
        break
if baseline is None:
    print("QA-DASHBOARD-RATE-WINDOW: FAIL - no in-window baseline among served "
          "snapshots; test seed is wrong")
    sys.exit(1)
win_hours = (newest["epoch_secs"] - baseline["epoch_secs"]) / 3600.0
windowed = (newest["long_term_total"] - baseline["long_term_total"]) / win_hours

# Naive whole-history rate (the buggy behaviour): oldest retained -> newest over
# the full span.
naive_hours = (newest["epoch_secs"] - oldest["epoch_secs"]) / 3600.0
naive = (newest["long_term_total"] - oldest["long_term_total"]) / naive_hours

if abs(served - windowed) > 1e-6:
    print("QA-DASHBOARD-RATE-WINDOW: FAIL - served rate %.6f != independent "
          "24h-windowed recompute %.6f" % (served, windowed))
    sys.exit(1)

if abs(served - naive) < 1e-3:
    print("QA-DASHBOARD-RATE-WINDOW: FAIL - served rate %.6f matches the naive "
          "whole-history rate %.6f; the ancient baseline is NOT being excluded "
          "(#4107 regression)" % (served, naive))
    sys.exit(1)

# The served rate must also honour the advertised window width.
if d.get("rate_window_secs") != WINDOW:
    print("QA-DASHBOARD-RATE-WINDOW: FAIL - rate_window_secs=%r, expected %d"
          % (d.get("rate_window_secs"), int(WINDOW)))
    sys.exit(1)

print("QA-DASHBOARD-RATE-WINDOW: PASS - served long-term rate %.3f/hr equals the "
      "24h-windowed recompute and differs from the naive whole-history %.3f/hr"
      % (served, naive))
PY
