#!/usr/bin/env bash
# Dashboard self-consistency checks (issues #1678, #1679, #1680).
#
# Verifies that the panels which previously contradicted each other now read
# from a single source of truth:
#   * #1680 — the "Cycle #N" counter agrees across Workboard, Overview
#     (/api/activity) and System Status (/api/status), and is never lower than
#     the highest cycle shown in the Recent Actions feed.
#   * #1678 — the Workboard "Active Engineers" count equals the Terminal tab's
#     live subagent-session count.
#   * #1679 — the Workboard "Working Memory" slot count equals the global
#     working-memory count reported by /api/memory (the Memory tab) and the
#     Workboard's own cognitive-statistics working_count.
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
wb="$(fetch /api/workboard)"
sub="$(fetch /api/subagent-sessions)"
act="$(fetch /api/activity)"
st="$(fetch /api/status)"
mem="$(fetch /api/memory)"

fail=0
check_eq() { # label actual expected
  if [ "$2" = "$3" ]; then
    echo "OK:   $1 ($2)"
  else
    echo "FAIL: $1 — got '$2', expected '$3'"
    fail=1
  fi
}
check_ge() { # label actual floor
  if [ "$2" -ge "$3" ] 2>/dev/null; then
    echo "OK:   $1 ($2 >= $3)"
  else
    echo "FAIL: $1 — $2 is below $3"
    fail=1
  fi
}

# ---- #1680: cycle counter is a single source of truth --------------------
wb_cycle=$(jq -r '.cycle.number // 0' <<<"$wb")
act_cycle=$(jq -r '.daemon.current_cycle // 0' <<<"$act")
st_cycle=$(jq -r '.daemon_health.cycle_number // 0' <<<"$st")
max_action_cycle=$(jq -r '([.recent_actions[]?.cycle // 0] + [0]) | max' <<<"$wb")
check_eq "#1680 cycle: Workboard == Overview"      "$wb_cycle" "$act_cycle"
check_eq "#1680 cycle: Workboard == System Status" "$wb_cycle" "$st_cycle"
check_ge "#1680 cycle: header >= Recent Actions max" "$wb_cycle" "$max_action_cycle"

# ---- #1678: active engineers match the Terminal tab ----------------------
wb_eng=$(jq -r '.spawned_engineers | length' <<<"$wb")
sub_live=$(jq -r '.live | length' <<<"$sub")
check_eq "#1678 engineers: Workboard == Terminal live sessions" "$wb_eng" "$sub_live"

# ---- #1679: working-memory count matches the Memory tab ------------------
# Slot-level enumeration is not exposed by the de-forked memory library, so the
# Workboard now drives its slot badge from the same `working_count` statistic
# the Memory tab reads. The cross-panel invariant is that both report the same
# working-memory count.
wb_wc=$(jq -r '.cognitive_statistics.working_count // 0' <<<"$wb")
mem_wc=$(jq -r '.native_memory.working // 0' <<<"$mem")
check_eq "#1679 working memory: Workboard working_count == Memory tab" "$wb_wc" "$mem_wc"

if [ "$fail" -ne 0 ]; then
  echo "RESULT: dashboard self-consistency FAILED"
  exit 1
fi
echo "RESULT: dashboard self-consistency PASSED"
