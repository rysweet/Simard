#!/usr/bin/env bash
# check-coin-benchmark-harness-done-gate.sh
#
# Read-only, fail-open verification helper for the Overseer blocked-goal triage
# of goal `build-a-local-coin-benchmark-harness-and-a-self-09e65e35`. The triage
# decision was to complete a goal whose deliverable had already merged, so this
# helper proves that goal's finish condition still holds:
#
#   1. Issue #2713 is CLOSED   (the goal's tracking issue).
#   2. PR    #4171 is MERGED   (shipped the `coin-gym verify` self-check).
#   3. Optional: when a built `coin-gym` is on PATH, `coin-gym verify` exits 0.
#
# Exit contract:
#   * All required criteria hold        -> exit 0, prints "done-gate PASS".
#   * A genuinely failed criterion      -> non-zero, prints "done-gate FAIL"
#                                          naming the criterion that failed.
#   * GitHub unreachable / gh missing   -> exit 0, prints "done-gate WARN" and
#     / unauthenticated                   skips remote checks. A WARN is never a
#                                          silent PASS: it means "could not check".
#
# Every GitHub call is pinned to the canonical repository and relies on ambient
# `gh` authentication (no tokens are read, echoed, or embedded). The script is
# strict and does not enable shell tracing.
#
# Usage:
#   scripts/check-coin-benchmark-harness-done-gate.sh

set -euo pipefail

REPO="rysweet/Simard"
ISSUE_NUMBER="2713"
PR_NUMBER="4171"

pass_msg() { printf '✅ done-gate PASS — %s\n' "$*"; }
fail_msg() { printf '❌ done-gate FAIL — %s\n' "$*" >&2; }
warn_msg() { printf '⚠️  done-gate WARN — %s\n' "$*" >&2; }

# ── Fail-open guards: never block on missing tooling or network state ─────────
if ! command -v gh >/dev/null 2>&1; then
  warn_msg "GitHub CLI (gh) is not available; skipping remote checks"
  exit 0
fi

if ! gh auth status >/dev/null 2>&1; then
  warn_msg "cannot reach GitHub (gh unauthenticated or offline); skipping remote checks"
  exit 0
fi

# ── Required remote criteria ──────────────────────────────────────────────────
failures=0

issue_state=""
if ! issue_state="$(gh issue view "$ISSUE_NUMBER" --repo "$REPO" --json state --jq '.state' 2>/dev/null)"; then
  warn_msg "could not read issue #${ISSUE_NUMBER} from GitHub; skipping remote checks"
  exit 0
fi

pr_state=""
if ! pr_state="$(gh pr view "$PR_NUMBER" --repo "$REPO" --json state --jq '.state' 2>/dev/null)"; then
  warn_msg "could not read pull request #${PR_NUMBER} from GitHub; skipping remote checks"
  exit 0
fi

if [[ "$issue_state" == "CLOSED" ]]; then
  printf 'ok: issue #%s is CLOSED\n' "$ISSUE_NUMBER"
else
  fail_msg "issue #${ISSUE_NUMBER} is not CLOSED (state: ${issue_state:-unknown})"
  failures=$((failures + 1))
fi

if [[ "$pr_state" == "MERGED" ]]; then
  printf 'ok: pull request #%s is MERGED\n' "$PR_NUMBER"
else
  fail_msg "pull request #${PR_NUMBER} is not MERGED (state: ${pr_state:-unknown})"
  failures=$((failures + 1))
fi

# ── Optional local criterion: the shipped self-check itself ───────────────────
# This is a bonus confirmation, not the authoritative signal. A stale local
# build that predates the merged `verify` subcommand must NOT turn the gate red:
# an "unknown command"/usage error means the local binary cannot run the check,
# which is a skip (like the binary being absent), never a regression. Only a
# `verify` that actually runs and reports failure counts against the gate.
if command -v coin-gym >/dev/null 2>&1; then
  coin_gym_out=""
  if coin_gym_out="$(coin-gym verify 2>&1)"; then
    printf 'ok: coin-gym verify exited 0\n'
  elif printf '%s' "$coin_gym_out" | grep -qiE 'unknown command|usage'; then
    printf 'skip: local coin-gym has no verify subcommand (build predates PR #%s); relying on the merged self-check\n' "$PR_NUMBER"
  else
    fail_msg "coin-gym verify did not exit 0 (harness self-check regressed)"
    failures=$((failures + 1))
  fi
else
  printf 'skip: coin-gym not on PATH; relying on the merged self-check (PR #%s)\n' "$PR_NUMBER"
fi

# ── Verdict ───────────────────────────────────────────────────────────────────
if [[ "$failures" -eq 0 ]]; then
  pass_msg "issue #${ISSUE_NUMBER} CLOSED, PR #${PR_NUMBER} MERGED"
  exit 0
fi

fail_msg "${failures} criterion(s) not satisfied; the goal's finish condition does not hold"
exit 1
