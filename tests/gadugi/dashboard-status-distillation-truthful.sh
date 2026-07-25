#!/usr/bin/env bash
# Dashboard Status-snapshot cognitive-processes distillation truthfulness.
#
# The unified Status snapshot (Overview "System Status" → MEMORY / BRAIN
# "cognitive" line, `GET /api/status/snapshot`, `simard status`, TUI Status tab)
# renders `cognitive  distillation X · consolidation Y · introspection Z`.
# Before this fix `cognitive_processes` was hard-coded to
# `CognitiveHealth::default()`, so `distillation` was ALWAYS `null` — the line
# read `distillation absent` even while distillation was demonstrably running
# (the daemon flushed `simard.distill.runs{result="ok"}` and the same snapshot's
# Telemetry section derived a live `distill_fail_pct` from it). An operator (or
# Simard) could never learn from that surface whether the distillation loop was
# alive.
#
# This outside-in check asserts the single-source-of-truth invariant: when the
# telemetry snapshot has published the distill-runs counter (surfaced as a
# non-null `data.telemetry.data.distill_fail_pct`, derived from the SAME
# `simard.distill.runs` counter), the Status snapshot's
# `data.memory.data.cognitive_processes.distillation` MUST agree — a non-null,
# non-"absent" health label — instead of contradicting itself with a permanent
# `null`. When the counter has not been flushed yet the field is honestly
# `null`/absent and the run-state assertion is skipped (the invariant is about
# *agreement*, never forcing a value to exist).
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

snap="$(curl -s -b "$CJ" "$URL/api/status/snapshot")"

fail=0

# Whatever the run-state, the section must never be a hard error object.
if jq -e '.error' >/dev/null 2>&1 <<<"$snap"; then
  echo "FAIL: /api/status/snapshot returned an error object: $(jq -r '.error' <<<"$snap")"
  echo "RESULT: dashboard status distillation truthfulness FAILED"
  exit 1
fi

mem_avail="$(jq -r '.data.memory.availability // "missing"' <<<"$snap")"
distill_pct="$(jq -r '.data.telemetry.data.distill_fail_pct // "null"' <<<"$snap")"
distillation="$(jq -r '.data.memory.data.cognitive_processes.distillation // "null"' <<<"$snap")"

echo "INFO: snapshot memory availability=$mem_avail"
echo "INFO: snapshot telemetry distill_fail_pct=$distill_pct"
echo "INFO: snapshot memory cognitive_processes.distillation='$distillation'"

# The invariant only applies when the memory section is present AND the distill
# counter has actually been published (its derived distill_fail_pct is non-null).
if [ "$mem_avail" = "ok" ] && [ "$distill_pct" != "null" ]; then
  if [ "$distillation" = "null" ]; then
    echo "FAIL: distill telemetry is published (distill_fail_pct=$distill_pct) but the"
    echo "      Status snapshot cognitive line reports distillation 'absent' (null) —"
    echo "      the MEMORY / BRAIN cognitive line contradicts its own Telemetry section."
    fail=1
  else
    echo "OK:   distillation health agrees with published telemetry ('$distillation')"
  fi
else
  echo "SKIP: distill-runs counter not published (or memory section absent) —"
  echo "      honestly-absent distillation is in-contract; agreement check not applicable"
fi

if [ "$fail" -ne 0 ]; then
  echo "RESULT: dashboard status distillation truthfulness FAILED"
  exit 1
fi
echo "RESULT: dashboard status distillation truthfulness PASSED"
