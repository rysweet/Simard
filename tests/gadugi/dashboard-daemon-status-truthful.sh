#!/usr/bin/env bash
# Dashboard Status-snapshot daemon truthfulness (#4215).
#
# The unified Status snapshot (Overview "System Status", `GET
# /api/status/snapshot`, `simard status`, TUI Status tab) must not report the
# OODA daemon as `unavailable` when it is actually running. Before #4215 the
# daemon section was assembled ONLY from `systemctl show`, so in every
# non-systemd deployment (dev / worktree / container) it read
# `availability: unavailable`, note `"systemctl: unit not loaded"` even while
# the daemon was mid-cycle — an operator (or Simard) could never learn the most
# basic health fact from that surface.
#
# This outside-in check asserts the single-source-of-truth invariant: when the
# durable heartbeat surfaced by `/api/status` says the daemon is running, the
# Status snapshot's `data.daemon` section must agree (availability=ok, a
# non-empty running/stale state), instead of contradicting itself with
# `unavailable`.
#
# Targets a running dashboard at $SIMARD_DASHBOARD_URL (default
# http://localhost:8080). Authenticates with $SIMARD_DASHKEY or
# ~/.simard/.dashkey. Exits non-zero on the first contradiction.
set -euo pipefail

URL="${SIMARD_DASHBOARD_URL:-http://localhost:8080}"
KEY="${SIMARD_DASHKEY:-}"
if [ -z "$KEY" ] && [ -f "$HOME/.simard/.dashkey" ]; then
  KEY="$(tr -d '[:space:]' < "$HOME/.simard/.dashkey")"
fi
if [ -z "$KEY" ]; then
  echo "FAIL: no dashkey (set SIMARD_DASHKEY or provide ~/.simard/.dashkey)"
  exit 1
fi

CJ="$(mktemp)"
trap 'rm -f "$CJ"' EXIT

http_code=$(curl -s -o /dev/null -w '%{http_code}' -c "$CJ" \
  -X POST "$URL/api/login" -H 'content-type: application/json' \
  -d "{\"code\":\"$KEY\"}")
if [ "$http_code" != "200" ]; then
  echo "FAIL: login to $URL returned HTTP $http_code"
  exit 1
fi

fetch() { curl -s -b "$CJ" "$URL$1"; }
st="$(fetch /api/status)"
snap="$(fetch /api/status/snapshot)"

fail=0

# The daemon-health heartbeat `/api/status` reads is the single source of truth
# for "is the daemon running". Skip the run-state assertions only when it is
# genuinely stopped/stale — the invariant is about *agreement*, not forcing a
# daemon to exist.
ooda="$(jq -r '.ooda_daemon // "unknown"' <<<"$st")"
d_avail="$(jq -r '.data.daemon.availability // "missing"' <<<"$snap")"
d_state="$(jq -r '.data.daemon.data.state // ""' <<<"$snap")"
d_note="$(jq -r '.data.daemon.note // ""' <<<"$snap")"

echo "INFO: /api/status ooda_daemon=$ooda"
echo "INFO: snapshot daemon availability=$d_avail state='$d_state' note='$d_note'"

if [ "$ooda" = "running" ]; then
  # #4215 core invariant: a running daemon must never read as unavailable.
  if [ "$d_avail" = "ok" ]; then
    echo "OK:   #4215 running daemon reported available in Status snapshot"
  else
    echo "FAIL: #4215 daemon is running (/api/status) but Status snapshot says availability='$d_avail' (note='$d_note')"
    fail=1
  fi
  # The state string must carry the running signal, not be blank.
  if printf '%s' "$d_state" | grep -qi 'running'; then
    echo "OK:   #4215 snapshot daemon state carries 'running' ('$d_state')"
  else
    echo "FAIL: #4215 snapshot daemon state does not reflect running: '$d_state'"
    fail=1
  fi
  # Regression guard: the misleading systemctl note must be gone once running.
  if printf '%s' "$d_note" | grep -qi 'systemctl: unit not loaded'; then
    echo "FAIL: #4215 snapshot still shows 'systemctl: unit not loaded' while daemon runs"
    fail=1
  else
    echo "OK:   #4215 no misleading 'unit not loaded' note while daemon runs"
  fi
else
  echo "SKIP: /api/status reports daemon '$ooda' (not running) — run-state agreement checks not applicable"
fi

# Whatever the run-state, the section must never be a hard error object.
if jq -e '.error' >/dev/null 2>&1 <<<"$snap"; then
  echo "FAIL: #4215 /api/status/snapshot returned an error object: $(jq -r '.error' <<<"$snap")"
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  echo "RESULT: dashboard daemon-status truthfulness FAILED"
  exit 1
fi
echo "RESULT: dashboard daemon-status truthfulness PASSED"
