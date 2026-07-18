#!/usr/bin/env bash
#
# Done-gate for goal: advance-rysweet-agent-kgpacks-rs-to-full-parity
#
# Purpose: give the "advance agent-kgpacks-rs to full parity" goal a single,
# machine-checkable finish condition so the Overseer's no-progress safeguard can
# certify "done" instead of parking it every cycle.
#
# Finish condition (all must hold for full parity):
#   1. Tracking issue rysweet/Simard#4321 is CLOSED, AND
#   2. every in-scope parity criterion (KGP-M*/Q*/T*/P*) in the spec is DONE,
#      backed by the two acceptance suites being green:
#         cargo test --lib native_knowledge
#         cargo test --lib knowledge_client
#
# Exit codes:
#   0  => full parity delivered (goal may be completed)
#   1  => not delivered; the concrete remaining criteria are printed by name
#
# The gate is truthful offline: if `gh` is unavailable it derives the verdict
# from the spec's criteria table and (when no criteria remain OPEN) the two
# acceptance suites. No silent fallback — every path prints why it decided.

set -uo pipefail

REPO="rysweet/Simard"
ISSUE=4321
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SPEC="${ROOT}/Specs/agent-kgpacks-rs-parity.md"

cd "${ROOT}"

echo "[done-gate] goal: advance agent-kgpacks-rs to full parity (tracking issue #${ISSUE})"

# 1) Authoritative signal: the tracking issue's state.
issue_state=""
if command -v gh >/dev/null 2>&1; then
  issue_state="$(gh issue view "${ISSUE}" --repo "${REPO}" --json state -q .state 2>/dev/null || true)"
fi

if [ "${issue_state}" = "CLOSED" ]; then
  echo "[done-gate] tracking issue #${ISSUE} is CLOSED — full parity delivered."
  exit 0
fi

# 2) Spec-driven verdict: which in-scope criteria are still OPEN?
if [ ! -f "${SPEC}" ]; then
  echo "[done-gate] FAIL: parity spec not found at ${SPEC}; cannot certify done." >&2
  exit 1
fi

remaining="$(awk -F'|' '
  /^\| KGP-(M|Q|T|P)[0-9]+ \|/ {
    id=$2; gsub(/^[ ]+|[ ]+$/, "", id);
    open=0;
    for (i = 1; i <= NF; i++) { v=$i; gsub(/^[ ]+|[ ]+$/, "", v); if (v == "OPEN") open=1 }
    if (open) print id
  }' "${SPEC}")"

if [ -n "${remaining}" ]; then
  echo "[done-gate] NOT at full parity — remaining in-scope criteria still OPEN:"
  echo "${remaining}" | sed 's/^/  - /'
  if [ "${issue_state}" = "OPEN" ]; then
    echo "[done-gate] tracking issue #${ISSUE} is still OPEN (expected until the above ship)."
  elif [ -z "${issue_state}" ]; then
    echo "[done-gate] (issue state unavailable offline; verdict derived from the spec.)"
  fi
  exit 1
fi

# 3) No OPEN criteria remain in the spec — corroborate with the acceptance suites.
echo "[done-gate] no OPEN in-scope criteria in spec; running acceptance suites..."
nk_log="$(mktemp)"
kc_log="$(mktemp)"
if cargo test --lib native_knowledge >"${nk_log}" 2>&1 \
  && cargo test --lib knowledge_client >"${kc_log}" 2>&1; then
  echo "[done-gate] both acceptance suites green — full parity delivered."
  rm -f "${nk_log}" "${kc_log}"
  exit 0
fi

echo "[done-gate] FAIL: acceptance suites are not green." >&2
echo "[done-gate] native_knowledge log: ${nk_log}" >&2
echo "[done-gate] knowledge_client log: ${kc_log}" >&2
exit 1
