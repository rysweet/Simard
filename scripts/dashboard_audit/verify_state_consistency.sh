#!/usr/bin/env bash
#
# Dashboard state-consistency verification (issues #1678, #1679, #1680).
#
# Asserts that the Whiteboard/Workboard and Overview panels agree with the
# Terminal/Memory/Thinking tabs by comparing the live JSON APIs (and the served
# SPA) that back them:
#
#   #1678  /api/workboard .spawned_engineers       == /api/subagent-sessions .live
#   #1679  the Workboard working-memory count binds to the authoritative
#          working_count statistic (same source as the Memory tab):
#            - served HTML binds `wb-wm-count` to `cognitive_statistics.working_count`
#            - /api/workboard .cognitive_statistics.working_count
#              == /api/memory/graph .stats.working
#   #1680  every "Cycle #N" source agrees:
#          /api/workboard .cycle.number
#          == /api/activity .daemon.current_cycle
#          == /api/status   .daemon_health.cycle_number
#          == max(/api/activity .recent_cycles[].cycle_number)   (Thinking tab)
#
# Usage: verify_state_consistency.sh [BASE_URL] [DASHKEY]
#   BASE_URL  default $DASHBOARD_URL or http://localhost:8080
#   DASHKEY   default $DASHBOARD_KEY or $(cat ~/.simard/.dashkey)
#
# Exits 0 when all three panels are consistent, 1 otherwise.
set -euo pipefail

BASE_URL="${1:-${DASHBOARD_URL:-http://localhost:8080}}"
DASHKEY="${2:-${DASHBOARD_KEY:-$(cat "${HOME}/.simard/.dashkey")}}"

work_dir="$(mktemp -d)"
trap 'rm -rf "${work_dir}"' EXIT

cookie="$(curl -s -i -X POST "${BASE_URL}/api/login" \
  -H 'content-type: application/json' \
  -d "{\"code\":\"${DASHKEY}\"}" \
  | sed -n 's/.*simard_session=\([^;]*\).*/\1/p' | tr -d '\r')"

if [[ -z "${cookie}" ]]; then
  echo "FAIL: could not authenticate to ${BASE_URL} (bad dashkey?)" >&2
  exit 1
fi

auth=(-H "Cookie:simard_session=${cookie}")
curl -s "${auth[@]}" "${BASE_URL}/api/workboard"          > "${work_dir}/workboard.json"
curl -s "${auth[@]}" "${BASE_URL}/api/subagent-sessions"  > "${work_dir}/subagents.json"
curl -s "${auth[@]}" "${BASE_URL}/api/activity"           > "${work_dir}/activity.json"
curl -s "${auth[@]}" "${BASE_URL}/api/status"             > "${work_dir}/status.json"
curl -s "${auth[@]}" "${BASE_URL}/api/memory/graph"       > "${work_dir}/memgraph.json"
curl -s "${auth[@]}" "${BASE_URL}/"                       > "${work_dir}/index.html"

WORK_DIR="${work_dir}" python3 - <<'PY'
import json, os, sys

d = os.environ["WORK_DIR"]


def load(name):
    with open(os.path.join(d, name)) as fh:
        return json.load(fh)


wb = load("workboard.json")
sa = load("subagents.json")
ac = load("activity.json")
st = load("status.json")
mg = load("memgraph.json")
with open(os.path.join(d, "index.html")) as fh:
    html = fh.read()

failures = []

# --- #1678: active engineers ---
engineers = len(wb.get("spawned_engineers", []))
live = len(sa.get("live", []))
ok_1678 = engineers == live
print(f"#1678 engineers: workboard={engineers} subagent_live={live} -> {'OK' if ok_1678 else 'MISMATCH'}")
if not ok_1678:
    failures.append("#1678")

# --- #1679: working-memory count sourced from working_count ---
wb_wc = (wb.get("cognitive_statistics") or {}).get("working_count")
mem_wc = (mg.get("stats") or {}).get("working")
bound = "d.cognitive_statistics.working_count" in html
source_ok = wb_wc is not None and wb_wc == mem_wc
ok_1679 = bound and source_ok
print(
    f"#1679 working memory: workboard.working_count={wb_wc} memory.working={mem_wc} "
    f"served_html_binds_working_count={bound} -> {'OK' if ok_1679 else 'MISMATCH'}"
)
if not ok_1679:
    failures.append("#1679")

# --- #1680: cycle number agreement ---
wb_cycle = (wb.get("cycle") or {}).get("number")
ac_cycle = (ac.get("daemon") or {}).get("current_cycle")
st_cycle = (st.get("daemon_health") or {}).get("cycle_number")
report_cycles = [
    (c.get("report") or {}).get("cycle_number") or c.get("cycle_number") or 0
    for c in ac.get("recent_cycles", [])
]
thinking_max = max(report_cycles) if report_cycles else None
shown = {wb_cycle, ac_cycle, st_cycle}
ok_1680 = len(shown) == 1 and (thinking_max is None or wb_cycle == thinking_max)
print(
    f"#1680 cycle: workboard={wb_cycle} activity={ac_cycle} status={st_cycle} "
    f"thinking_max={thinking_max} -> {'OK' if ok_1680 else 'MISMATCH'}"
)
if not ok_1680:
    failures.append("#1680")

if failures:
    print("FAIL: dashboard panels disagree: " + ", ".join(failures))
    sys.exit(1)
print("PASS: Whiteboard/Overview agree with Terminal/Memory/Thinking")
PY
