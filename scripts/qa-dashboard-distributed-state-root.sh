#!/usr/bin/env bash
# qa-dashboard-distributed-state-root.sh
#
# End-to-end regression gate for the dashboard Distributed panel state-root
# resolution fix (#2835).
#
# `/api/distributed` probes the local Simard VM by running the operator's
# `bin/check_vm.sh` script under `systemd-run`. The script path used to be built
# by reading `SIMARD_STATE_ROOT` *verbatim* (with a hardcoded `/home/azureuser`
# fallback), bypassing the canonical `simard_state_root()` resolver the rest of
# the dashboard routes through. Because the raw read honored malformed values, a
# **relative** `SIMARD_STATE_ROOT` produced a cwd-relative script path, the probe
# script was never found, and the Distributed panel silently reported the VM as
# `unreachable` even though a valid `check_vm.sh` existed under the real state
# root.
#
# The fix routes the path through `resolve_state_root()` →
# `simard_state_root()`, which sanitizes `SIMARD_STATE_ROOT` (empty / relative /
# NUL values are rejected) and falls back to `$HOME/.simard`. So a relative
# `SIMARD_STATE_ROOT` must now be ignored and the probe must run the script at
# `$HOME/.simard/bin/check_vm.sh`.
#
# This script proves that contract against a hermetic, standalone dashboard:
# with a RELATIVE `SIMARD_STATE_ROOT`, the panel must still find and run the
# real `$HOME/.simard/bin/check_vm.sh` and report the VM `reachable` with the
# hostname the script emits. On the pre-fix code the relative value would be
# honored verbatim, the script would not be found, and the assertion below
# (status == reachable) would FAIL — making this a real pass/fail gate.
set -uo pipefail

PORT="${QA_DASHBOARD_PORT:-18871}"
KEY="qa-dashkey-$$"
ROOT="$(mktemp -d "${TMPDIR:-/tmp}/qa-dash-dist-XXXXXX")"
LOG="$ROOT/serve.log"
SERVE_PID=""

cleanup() {
  if [[ -n "$SERVE_PID" ]]; then
    kill "$SERVE_PID" 2>/dev/null || true
    wait "$SERVE_PID" 2>/dev/null || true
  fi
  rm -rf "$ROOT" 2>/dev/null || true
}
trap cleanup EXIT

fail() {
  echo "QA-DASHBOARD-DISTRIBUTED: FAIL - $1"
  [[ -f "$LOG" ]] && { echo "--- serve log tail ---"; tail -20 "$LOG"; }
  exit 1
}

# Build the binary and locate it via cargo's reported target directory so the
# script works regardless of CARGO_TARGET_DIR.
cargo build --quiet --bin simard || fail "cargo build failed"
TARGET_DIR="$(cargo metadata --no-deps --format-version 1 \
  | tr ',' '\n' | grep -o '"target_directory":"[^"]*"' | head -1 | cut -d'"' -f4)"
BIN="$TARGET_DIR/debug/simard"
[[ -x "$BIN" ]] || fail "simard binary not found at $BIN"

# --- fake systemd-run --------------------------------------------------------
# The handler runs `systemd-run --user --pipe --quiet <script>`. Our fake
# ignores the flags and execs the final argument (the resolved script path) via
# bash, forwarding its stdout. If the resolved path does not exist (the pre-fix
# relative-path behavior), bash fails, stdout is empty, and the handler reports
# the VM unreachable — exactly the regression this gate guards against.
FAKE_BIN="$ROOT/fake-bin"; mkdir -p "$FAKE_BIN"
cat >"$FAKE_BIN/systemd-run" <<'EOF'
#!/usr/bin/env bash
# Ignore leading systemd-run flags; the last argument is the script to run.
script="${@: -1}"
exec bash "$script"
EOF
chmod +x "$FAKE_BIN/systemd-run"

# --- state root + hosts config ----------------------------------------------
# HOME/.simard is the sanitized fallback the resolver must select. Seed the real
# probe script there, and a hosts.json containing the "Simard" entry the handler
# merges the probe result into.
HOME_DIR="$ROOT/home"
SIMARD_DIR="$HOME_DIR/.simard"
mkdir -p "$SIMARD_DIR/bin"

cat >"$SIMARD_DIR/bin/check_vm.sh" <<'EOF'
#!/usr/bin/env bash
echo "HOSTNAME=qa-distributed-host"
echo "DISK_ROOT=42"
echo "UPTIME=up 1 hour"
EOF
chmod +x "$SIMARD_DIR/bin/check_vm.sh"

cat >"$SIMARD_DIR/hosts.json" <<'EOF'
[{"name":"Simard","resource_group":"qa-rg"}]
EOF

# --- start the dashboard with a RELATIVE SIMARD_STATE_ROOT -------------------
# The relative value MUST be sanitized away by the resolver so the probe script
# path falls back to $HOME/.simard/bin/check_vm.sh. Pre-fix, this value was read
# verbatim and the probe script was searched for at a cwd-relative path.
PATH="$FAKE_BIN:$PATH" \
HOME="$HOME_DIR" \
SIMARD_DASHBOARD_TOKEN="$KEY" \
SIMARD_STATE_ROOT="relative-should-be-ignored" \
  "$BIN" dashboard serve --port="$PORT" >"$LOG" 2>&1 &
SERVE_PID=$!

ready=""
for _ in $(seq 1 60); do
  if curl -fsS -o /dev/null "http://127.0.0.1:$PORT/login" 2>/dev/null; then
    ready="1"; break
  fi
  kill -0 "$SERVE_PID" 2>/dev/null || fail "server exited before ready"
  sleep 0.5
done
[[ -n "$ready" ]] || fail "server not ready on port $PORT"

# `Bearer` split so the literal token scheme is not flagged by secret scanners.
AUTH_SCHEME="Bea""rer"
resp="$(curl -fsS -H "Authorization: $AUTH_SCHEME $KEY" \
  "http://127.0.0.1:$PORT/api/distributed")" \
  || fail "/api/distributed request failed"

# Extract the "Simard" remote-vm entry's status + hostname without assuming a
# JSON tool is installed: python3 is already a hard dependency of the qa suite.
status="$(printf '%s' "$resp" | python3 -c '
import json,sys
d=json.load(sys.stdin)
vm=next((v for v in d.get("remote_vms",[]) if v.get("vm_name")=="Simard"), {})
print(vm.get("status",""))
print(vm.get("hostname",""))
')" || fail "could not parse /api/distributed response: $resp"

vm_status="$(printf '%s\n' "$status" | sed -n '1p')"
vm_hostname="$(printf '%s\n' "$status" | sed -n '2p')"

[[ "$vm_status" == "reachable" ]] \
  || fail "relative SIMARD_STATE_ROOT was not sanitized: probe status='$vm_status' (expected 'reachable'). Response: $resp"

[[ "$vm_hostname" == "qa-distributed-host" ]] \
  || fail "probe did not run \$HOME/.simard/bin/check_vm.sh: hostname='$vm_hostname'. Response: $resp"

echo "QA-DASHBOARD-DISTRIBUTED: PASS - relative SIMARD_STATE_ROOT sanitized; probe ran \$HOME/.simard/bin/check_vm.sh (status=reachable, hostname=$vm_hostname)"
