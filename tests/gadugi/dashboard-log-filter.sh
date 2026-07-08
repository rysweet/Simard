#!/usr/bin/env bash
# dashboard-log-filter.sh — outside-in check for issue #1687.
#
# The Logs tab "Background Service Log" panel exposes an All/Errors/Warnings/Info
# level filter. The daemon emits human-readable lines with no level token, so the
# old filter (naive substring match on "error"/"warn"/"info") matched nothing:
# selecting any level except "All levels" — even "Info" — produced an empty list,
# making the control look inert (#1687). The fix classifies each line server-side
# (/api/logs now returns a parallel daemon_log_levels array) and the frontend
# filters on that classified level (with a client-side fallback classifier).
#
# This script boots the real dashboard binary, logs in, and asserts — against the
# served HTML and the live /api/logs response — that the backend emits a level
# per line, the frontend is wired to filter on it, and the panel uses the new
# plain-English labels instead of the "Daemon Log" jargon.
set -euo pipefail

PORT="${DASH_PORT:-8143}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-logfilter.XXXXXX.log)"
CJ="$(mktemp -t dash-logfilter-cookies.XXXXXX)"
RESP="$(mktemp -t dash-logfilter-logs.XXXXXX.json)"

echo "[log-filter] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[log-filter] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[log-filter] starting dashboard on :$PORT"
"$BIN" dashboard serve --port="$PORT" >"$LOG" 2>&1 &
DASH_PID=$!
cleanup() { kill "$DASH_PID" 2>/dev/null || true; rm -f "$CJ" "$RESP"; }
trap cleanup EXIT

# Wait for the server to accept connections.
up=0
for _ in $(seq 1 30); do
  if curl -s -o /dev/null "http://localhost:$PORT/login"; then up=1; break; fi
  sleep 1
done
if [[ "$up" -ne 1 ]]; then
  echo "[log-filter] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[log-filter] FAIL: login rejected" >&2
  exit 1
fi

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"
curl -s -b "$CJ" "http://localhost:$PORT/api/logs" >"$RESP"

fail() { echo "[log-filter] FAIL: $1" >&2; exit 1; }

# ── Served HTML: the panel is wired to filter on classified level ────────────
grep -q 'Background Service Log' <<<"$HTML" \
  || fail "Logs panel must use the plain-English 'Background Service Log' heading"
grep -q '>Daemon Log<' <<<"$HTML" \
  && fail "the 'Daemon Log' jargon heading must be gone (#1687 jargon pass)"
grep -q 'allLogLevels' <<<"$HTML" \
  || fail "frontend must store the per-line levels from /api/logs"
grep -q 'classifyLogLevel' <<<"$HTML" \
  || fail "frontend must carry a client-side level classifier fallback"
grep -q 'logLevelAt' <<<"$HTML" \
  || fail "level filter must match on the classified level, not a raw substring"

# ── Live API: the backend emits a parseable level for every line ─────────────
grep -q '"daemon_log_levels"' <<<"$(cat "$RESP")" \
  || fail "/api/logs must expose a daemon_log_levels array (#1687)"

# Validate the parallel arrays with jq (native, no Python): daemon_log_lines and
# daemon_log_levels must both be arrays of equal length, and every level token
# must be one of error/warn/info.
jq -e '
  (.daemon_log_lines | type) == "array" and
  (.daemon_log_levels | type) == "array" and
  (.daemon_log_lines | length) == (.daemon_log_levels | length) and
  (.daemon_log_levels | all(. == "error" or . == "warn" or . == "info"))
' "$RESP" >/dev/null \
  || fail "/api/logs daemon_log_lines/daemon_log_levels must be equal-length arrays of error|warn|info tokens"

n_lines="$(jq '.daemon_log_lines | length' "$RESP")"
distinct_levels="$(jq -r '[.daemon_log_levels[]] | unique | join(",")' "$RESP")"
echo "[log-filter] api levels ok: ${n_lines} lines, each classified into ${distinct_levels:-(no lines)}"

echo "[log-filter] PASS: Logs level filter is wired end-to-end (#1687)"
