#!/usr/bin/env bash
#
# Done-gate for goal: advance agent-kgpacks-rs to full parity
# (goal id: advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c)
#
# Purpose: give this long-running goal a single, machine-checkable finish
# condition so the Overseer's no-progress safeguard can certify "done" instead
# of re-parking it every cycle. Before this gate, the goal had no tracked
# PR/issue the completion check could observe, so it could never be certified
# and kept re-investigating without shipping.
#
# Finish condition (authoritative): tracking issue rysweet/Simard#4321 is CLOSED.
# That issue is the goal's single verifiable finish line — it closes only once
# the three remaining in-scope criteria (KGP-Q4, KGP-T3, KGP-Q5) ship and the
# two spec commands below are green on main. See:
#   Specs/agent-kgpacks-rs-parity.md   (source of truth)
#   https://github.com/rysweet/Simard/issues/4321   (done-gate issue)
#
# The gate is truthful:
#   * The AUTHORITATIVE verdict is the observable state of issue #4321
#     (CLOSED => done). This is what Simard's completion check certifies on.
#   * As corroboration it also runs the two spec acceptance suites so a local
#     checkout can see the same green bar the issue's close criteria require.
#
# Exit codes:
#   0  => issue #4321 is CLOSED (goal delivered and certifiable)
#   1  => not certified; the concrete reason is printed by name
#   2  => cannot determine issue state (no `gh` / no network); the spec suites
#         are still run and reported, but the authoritative gate is inconclusive
#         and the goal must NOT be auto-completed on this run.
#
# No silent fallback — every path prints why it decided.

set -uo pipefail

REPO="rysweet/Simard"
DONE_GATE_ISSUE=4321
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SPEC="${ROOT}/Specs/agent-kgpacks-rs-parity.md"

cd "${ROOT}" || { echo "[done-gate] FAIL: cannot cd to repo root ${ROOT}" >&2; exit 1; }

echo "[done-gate] goal: advance agent-kgpacks-rs to full parity"

fail() {
  echo "[done-gate] FAIL: $1" >&2
  exit 1
}

# 0) The spec (source of truth for the finish line) must be present.
[ -f "${SPEC}" ] || fail "spec Specs/agent-kgpacks-rs-parity.md is absent; nothing defines the finish line."
echo "[done-gate] [PASS] spec present (Specs/agent-kgpacks-rs-parity.md)."

# 1) Corroborating acceptance suites named by the done-gate issue.
#    These are the same green bar #4321's close criteria require.
run_suite() {
  local target="$1"
  echo "[done-gate] running acceptance suite: cargo test --lib ${target} ..."
  local log
  log="$(mktemp)"
  if cargo test --lib "${target}" >"${log}" 2>&1; then
    echo "[done-gate] [PASS] ${target} suite green ($(grep -Eo '[0-9]+ passed' "${log}" | tail -1))."
    rm -f "${log}"
    return 0
  fi
  echo "[done-gate] ${target} test log tail:" >&2
  tail -n 40 "${log}" | sed 's/^/[done-gate]     /' >&2
  echo "[done-gate] full ${target} log: ${log}" >&2
  return 1
}

suites_green=1
run_suite native_knowledge || suites_green=0
run_suite knowledge_client || suites_green=0

# 2) AUTHORITATIVE gate: observable state of tracking issue #4321.
if ! command -v gh >/dev/null 2>&1; then
  echo "[done-gate] (inconclusive) 'gh' is unavailable; cannot observe issue #${DONE_GATE_ISSUE} state." >&2
  echo "[done-gate] spec suites green=${suites_green}. Authoritative gate could not run — NOT certifying." >&2
  exit 2
fi

state="$(gh issue view "${DONE_GATE_ISSUE}" --repo "${REPO}" --json state -q .state 2>/dev/null || true)"
if [ -z "${state}" ]; then
  echo "[done-gate] (inconclusive) could not read issue #${DONE_GATE_ISSUE} state (no network / auth?)." >&2
  echo "[done-gate] spec suites green=${suites_green}. Authoritative gate could not run — NOT certifying." >&2
  exit 2
fi

echo "[done-gate] tracking issue #${DONE_GATE_ISSUE} state: ${state}"
if [ "${state}" = "CLOSED" ]; then
  if [ "${suites_green}" -ne 1 ]; then
    fail "issue #${DONE_GATE_ISSUE} is CLOSED but a spec acceptance suite is red in this checkout."
  fi
  echo "[done-gate] CERTIFIED: agent-kgpacks-rs full parity delivered (issue #${DONE_GATE_ISSUE} CLOSED, spec suites green)."
  exit 0
fi

fail "issue #${DONE_GATE_ISSUE} is ${state}; the goal finishes only when it is CLOSED (KGP-Q4, KGP-T3, KGP-Q5 must ship)."
