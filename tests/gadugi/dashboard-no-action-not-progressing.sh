#!/usr/bin/env bash
# Dashboard Cycle-History honesty: a no-action cycle is NOT "progressing".
#
# Regression guard for the false-friend `commit` marker. The typed OODA outcome
# ledger commits *every* terminal — including a pure no-action — with the verb
# "committed" ("typed no-action committed: outcome=…", see
# ooda_actions::advance_goal::typed_goal_session). The old progress marker
# `commit` matched the substring inside "no-action committed", so no-action
# cycles were mislabelled `progressing` in /api/ooda-cycles, hiding goals that
# were making no shippable progress.
#
# Invariant enforced here: every cycle the dashboard labels
# `disposition == "progressing"` MUST carry at least one genuine forward-progress
# signal in its outcomes — the real-action ledger suffix "effect completed", a
# "pr #" reference, "launched"/"dispatched" work, or a live `spawn_engineer`.
# A cycle whose only "progress" was the substring "commit" inside a no-action
# ledger commit fails this check.
#
# Targets a running dashboard at $SIMARD_DASHBOARD_URL (default
# http://localhost:8080). Authenticates with $SIMARD_DASHKEY or ~/.simard/.dashkey.
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

cycles="$(curl -s -b "$CJ" "$URL/api/ooda-cycles")"

# A jq filter that selects progressing cycles which carry NO genuine
# forward-progress signal in any outcome (the mislabelled ones).
read -r -d '' OFFENDERS_FILTER <<'JQ' || true
.cycles[]
| select(.disposition == "progressing")
| . as $c
| select(
    ( ($c.outcomes // []) | any(
        (.detail // "" | ascii_downcase) as $d
        | ($d | contains("effect completed")) or ($d | contains("pr #"))
          or ($d | contains("launched")) or ($d | contains("dispatched"))
          or ((.spawn_engineer.status // "") | ascii_downcase == "live")
    ) ) | not )
JQ

offenders=$(jq "[ $OFFENDERS_FILTER | .cycle_number ] | length" <<<"$cycles")

if [ "$offenders" != "0" ]; then
  echo "FAIL: $offenders progressing cycle(s) carry no real forward-progress signal"
  detail="$(jq -r "$OFFENDERS_FILTER
    | \"  cycle #\(.cycle_number): \([.outcomes[]?.detail] | join(\" | \"))\"" \
    <<<"$cycles")"
  printf '%s\n' "$detail" | head -5
  exit 1
fi

echo "OK:   every progressing cycle carries a real forward-progress signal"
echo "RESULT: dashboard no-action/progressing honesty PASSED"
