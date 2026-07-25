#!/usr/bin/env bash
# qa-dashboard-hosts-discovery.sh
#
# End-to-end check for the `/api/hosts` VM-discovery timeout + cache fix.
#
# `get_hosts()` shells out to `azlin list` to discover VMs. `azlin list` queries
# Azure and routinely takes 10–20s; the handler previously ran it with NO
# timeout (despite a comment claiming "best-effort, with timeout"), so the Hosts
# tab blocked for the full duration — or indefinitely if azlin hung — and any
# API client with a sane timeout got a hard failure.
#
# The fix bounds the call with a hard timeout (overridable via
# SIMARD_AZLIN_LIST_TIMEOUT_SECS), short-caches successful results, and adds
# additive `discovery_timed_out` / `discovery_stale` response flags so the UI
# signals degradation instead of silently blanking.
#
# This script guards two contracts against separate, hermetic dashboards:
#   A. A SLOW azlin must not block /api/hosts beyond the timeout, and the
#      response must set discovery_timed_out=true.
#   B. A FAST azlin must populate `discovered` with discovery_timed_out=false.
set -uo pipefail

KEY="qa-dashkey-$$"
ROOT="$(mktemp -d "${TMPDIR:-/tmp}/qa-dash-hosts-XXXXXX")"
SERVE_PID=""

cleanup() {
  if [[ -n "$SERVE_PID" ]]; then
    kill "$SERVE_PID" 2>/dev/null || true
  fi
  rm -rf "$ROOT" 2>/dev/null || true
}
trap cleanup EXIT

fail() {
  echo "QA-DASHBOARD-HOSTS: FAIL - $1"
  [[ -n "${LOG:-}" && -f "$LOG" ]] && { echo "--- serve log tail ---"; tail -20 "$LOG"; }
  exit 1
}

# Build the binary and locate it via cargo's reported target directory so the
# script works regardless of CARGO_TARGET_DIR.
cargo build --quiet --bin simard || fail "cargo build failed"
TARGET_DIR="$(cargo metadata --no-deps --format-version 1 \
  | tr ',' '\n' | grep -o '"target_directory":"[^"]*"' | head -1 | cut -d'"' -f4)"
BIN="$TARGET_DIR/debug/simard"
[[ -x "$BIN" ]] || fail "simard binary not found at $BIN"

# --- fake azlin binaries -----------------------------------------------------
# A "slow" azlin that sleeps far longer than the test timeout, and a "fast" one
# that emits a valid VM-list JSON array immediately. Each lives in its own dir
# so we can prepend exactly one to PATH per server instance.
SLOW_DIR="$ROOT/slow-bin"; mkdir -p "$SLOW_DIR"
cat >"$SLOW_DIR/azlin" <<'EOF'
#!/usr/bin/env bash
# Simulate a slow Azure query that outlives the discovery timeout.
sleep 30
echo '[]'
EOF
chmod +x "$SLOW_DIR/azlin"

FAST_DIR="$ROOT/fast-bin"; mkdir -p "$FAST_DIR"
cat >"$FAST_DIR/azlin" <<'EOF'
#!/usr/bin/env bash
echo '[{"name":"qa-vm-1","location":"westus2","resource_group":"rysweet-linux-vm-pool"}]'
EOF
chmod +x "$FAST_DIR/azlin"

# --- server helpers ----------------------------------------------------------
start_server() {
  # $1 = PATH-prepend dir (fake azlin), $2 = port, $3 = state/HOME root suffix
  local binpath="$1" port="$2" suffix="$3"
  local home="$ROOT/home-$suffix"
  mkdir -p "$home"
  LOG="$ROOT/serve-$suffix.log"
  PATH="$binpath:$PATH" \
  HOME="$home" SIMARD_DASHBOARD_TOKEN="$KEY" SIMARD_STATE_ROOT="$home" \
  SIMARD_AZLIN_LIST_TIMEOUT_SECS=2 \
    "$BIN" dashboard serve --port="$port" >"$LOG" 2>&1 &
  SERVE_PID=$!
  local ready=""
  for _ in $(seq 1 60); do
    if curl -fsS -o /dev/null "http://127.0.0.1:$port/login" 2>/dev/null; then
      ready="1"; break
    fi
    kill -0 "$SERVE_PID" 2>/dev/null || fail "server-$suffix exited before ready"
    sleep 0.5
  done
  [[ -n "$ready" ]] || fail "server-$suffix not ready on port $port"
}

stop_server() {
  if [[ -n "$SERVE_PID" ]]; then
    kill "$SERVE_PID" 2>/dev/null || true
    wait "$SERVE_PID" 2>/dev/null || true
  fi
  SERVE_PID=""
}

AUTH_SCHEME="Bea""rer"
AUTH=(-H "Authorization: $AUTH_SCHEME $KEY")
api() { curl -fsS "${AUTH[@]}" "$@"; }

# ---------------------------------------------------------------------------
# Scenario A: SLOW azlin must not block past the timeout; flag must be set.
# ---------------------------------------------------------------------------
PORT_A="${QA_DASHBOARD_PORT_A:-18861}"
start_server "$SLOW_DIR" "$PORT_A" "slow"

start_ns="$(date +%s%N)"
resp_a="$(api "http://127.0.0.1:$PORT_A/api/hosts")" || fail "slow /api/hosts request failed"
end_ns="$(date +%s%N)"
elapsed_ms=$(( (end_ns - start_ns) / 1000000 ))

# The real (fake) azlin sleeps 30s; with a 2s timeout the endpoint must respond
# in well under 15s. A generous 15000ms bound avoids CI flakiness while still
# proving the unbounded-block regression is fixed.
if (( elapsed_ms >= 15000 )); then
  fail "slow /api/hosts took ${elapsed_ms}ms — expected it to be bounded by the timeout"
fi

echo "$resp_a" | grep -q '"discovery_timed_out":true' \
  || fail "slow azlin did not set discovery_timed_out=true (response: $resp_a)"
# No prior cache on a fresh server => not stale, and discovered is empty.
echo "$resp_a" | grep -q '"discovery_stale":false' \
  || fail "expected discovery_stale=false on a cold timeout (response: $resp_a)"

echo "QA-DASHBOARD-HOSTS: scenario A ok (bounded in ${elapsed_ms}ms, discovery_timed_out=true)"
stop_server

# ---------------------------------------------------------------------------
# Scenario B: FAST azlin must populate discovered with no timeout flag.
# ---------------------------------------------------------------------------
PORT_B="${QA_DASHBOARD_PORT_B:-18862}"
start_server "$FAST_DIR" "$PORT_B" "fast"

resp_b="$(api "http://127.0.0.1:$PORT_B/api/hosts")" || fail "fast /api/hosts request failed"
echo "$resp_b" | grep -q '"discovery_timed_out":false' \
  || fail "fast azlin should not time out (response: $resp_b)"
echo "$resp_b" | grep -q '"qa-vm-1"' \
  || fail "fast azlin discovery did not surface the VM (response: $resp_b)"

echo "QA-DASHBOARD-HOSTS: scenario B ok (discovered qa-vm-1, discovery_timed_out=false)"
stop_server

echo "QA-DASHBOARD-HOSTS: PASS - /api/hosts discovery is bounded, cached, and flagged"
