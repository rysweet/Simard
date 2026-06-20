#!/usr/bin/env bash
# dashboard-memory-fidelity.sh — outside-in check for issue #1681.
#
# The Memory tab used to render four always-on "Memory Files" tiles, including
# retired JSON snapshot files (memory_records / evidence_records / handoff).
# When those files were empty the panel showed "0 records / 0 B" tiles right
# next to a populated native Memory Store, telling the operator memory was
# empty when it was rich. This script boots the real dashboard binary, logs in,
# and asserts — against the served HTML and the live /api/memory response —
# that the misleading tiles are gone, the live Memory Store counts are exposed,
# and the goals snapshot uses plain language with a link to the Goals tab.
set -euo pipefail

PORT="${DASH_PORT:-8137}"
DASHKEY_FILE="${HOME}/.simard/.dashkey"
LOG="$(mktemp -t dash-fidelity.XXXXXX.log)"
CJ="$(mktemp -t dash-fidelity-cookies.XXXXXX)"

echo "[fidelity] building simard binary…"
cargo build --quiet --bin simard
BIN="${CARGO_TARGET_DIR:-target}/debug/simard"
if [[ ! -x "$BIN" ]]; then
  echo "[fidelity] FAIL: built binary not found at $BIN" >&2
  exit 1
fi

echo "[fidelity] starting dashboard on :$PORT"
"$BIN" dashboard serve --port="$PORT" >"$LOG" 2>&1 &
DASH_PID=$!
cleanup() { kill "$DASH_PID" 2>/dev/null || true; rm -f "$CJ"; }
trap cleanup EXIT

# Wait for the server to accept connections.
up=0
for _ in $(seq 1 30); do
  if curl -s -o /dev/null "http://localhost:$PORT/login"; then up=1; break; fi
  sleep 1
done
if [[ "$up" -ne 1 ]]; then
  echo "[fidelity] FAIL: dashboard did not come up on :$PORT" >&2
  cat "$LOG" >&2
  exit 1
fi

KEY="$(cat "$DASHKEY_FILE")"
if ! curl -s -c "$CJ" -X POST -H 'Content-Type: application/json' \
      -d "{\"code\":\"$KEY\"}" "http://localhost:$PORT/api/login" | grep -q '"ok":true'; then
  echo "[fidelity] FAIL: login rejected" >&2
  exit 1
fi

HTML="$(curl -s -b "$CJ" "http://localhost:$PORT/")"
MEM="$(curl -s -b "$CJ" "http://localhost:$PORT/api/memory")"

fail() { echo "[fidelity] FAIL: $1" >&2; exit 1; }

# ── Served HTML: the fixed Memory Files panel ────────────────────────────────
grep -q 'Goals (snapshot)' <<<"$HTML" \
  || fail "goals snapshot tile missing plain-language label"
grep -q 'data-tab=goals' <<<"$HTML" \
  || fail "goals snapshot must link back to the Goals tab"
grep -q 'Legacy snapshots (superseded by the Memory Store)' <<<"$HTML" \
  || fail "legacy files must collapse into a single plain-language disclosure"
grep -q 'legacyWithData' <<<"$HTML" \
  || fail "legacy tiles must be gated to files that actually have content"
grep -q '(info.size_bytes||0)<=0' <<<"$HTML" \
  || fail "legacy gating must require non-zero size_bytes (#1681)"
grep -q 'info.count<=0' <<<"$HTML" \
  || fail "legacy JSON files must have records to render, never '0 records' (#1681)"

# ── Served HTML: the misleading artifacts must be gone ───────────────────────
grep -q 'Goal Records (agent memory)' <<<"$HTML" \
  && fail "legacy 'Goal Records (agent memory)' jargon tile still present"
grep -qF ")':'Never'" <<<"$HTML" \
  && fail "Last Memory Compaction must not fall back to the literal 'Never'"
grep -q 'superseded by LadybugDB' <<<"$HTML" \
  && fail "operator labels must avoid the 'LadybugDB' jargon"

# ── Live API: the source of truth that the panel renders ─────────────────────
grep -q '"native_memory"' <<<"$MEM" \
  || fail "/api/memory must expose native_memory counts (the Memory Store)"
grep -q '"total_facts"' <<<"$MEM" \
  || fail "/api/memory must expose total_facts"

echo "[fidelity] PASS: Memory tab data-fidelity holds (#1681)"
