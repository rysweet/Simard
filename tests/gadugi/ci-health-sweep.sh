#!/usr/bin/env bash
# Outside-in coverage for `simard ci-health`: the codified governed-fleet
# CI-health sweep. Drives the CLI with committed offline fixtures (no network)
# that encode the real-world signal classes we observed:
#   - active workflow, latest run success        -> green
#   - disabled workflow, stale last-run failure  -> ignored (workflow_disabled)
#   - active workflow, cancelled last run        -> ignored (non-failure)
#   - active workflow, in-progress last run       -> ignored (in progress)
#   - active workflow, latest run failure        -> ACTIONABLE failure (red)
#
# Asserts that disabled/cancelled/in-progress signals never fail the fleet,
# that a genuine active-CI failure does, that the exit code follows the
# verdict, and that the opt-in `--file-issues` write is guarded off the offline
# fixture path (a live-only sweep) and advertised in help.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

GREEN_FIXTURE="tests/gadugi/fixtures/ci-health-green.json"
FAILING_FIXTURE="tests/gadugi/fixtures/ci-health-failing.json"

run_ci_health() {
  cargo run --quiet --bin simard -- ci-health "$@" 2>/dev/null
}

# ── 1. Green fixture: only disabled/cancelled/in-progress non-green signals ──
GREEN_OUT="$(run_ci_health --from-json "$GREEN_FIXTURE")"
printf '%s\n' "$GREEN_OUT"

printf '%s\n' "$GREEN_OUT" | grep -F "CI-HEALTH: GREEN" >/dev/null
printf '%s\n' "$GREEN_OUT" | grep -F "actionable failures: 0" >/dev/null
# The disabled azlin monitors must be ignored with the disabled reason, not
# counted as failures.
printf '%s\n' "$GREEN_OUT" | grep -F "Code Quality Tracker (workflow_disabled)" >/dev/null
printf '%s\n' "$GREEN_OUT" | grep -F "CI/CD Workflow Health Monitor (workflow_disabled)" >/dev/null
# The cancelled Build Knowledge Pack must be an ignored non-failure.
printf '%s\n' "$GREEN_OUT" | grep -F "Build Knowledge Pack (non_failure_conclusion:cancelled)" >/dev/null
# The in-progress Simard verify run must be ignored, not treated as a failure.
printf '%s\n' "$GREEN_OUT" | grep -F "verify (run_in_progress)" >/dev/null

# Exit code must be 0 for a green fleet.
if ! run_ci_health --from-json "$GREEN_FIXTURE" >/dev/null; then
  echo "FAIL: green fixture returned non-zero exit code" >&2
  exit 1
fi

# ── 2. JSON output shape ────────────────────────────────────────────────────
GREEN_JSON="$(run_ci_health --json --from-json "$GREEN_FIXTURE")"
printf '%s\n' "$GREEN_JSON" | grep -F '"green": true' >/dev/null
printf '%s\n' "$GREEN_JSON" | grep -F '"actionable_failures": []' >/dev/null

# ── 3. Failing fixture: one ACTIVE workflow failed ──────────────────────────
set +e
FAIL_OUT="$(run_ci_health --from-json "$FAILING_FIXTURE")"
FAIL_CODE=$?
set -e
printf '%s\n' "$FAIL_OUT"

printf '%s\n' "$FAIL_OUT" | grep -F "CI-HEALTH: FAILING" >/dev/null
# The active CI failure is flagged with its repo and conclusion...
printf '%s\n' "$FAIL_OUT" | grep -F "rysweet/azlin" >/dev/null
printf '%s\n' "$FAIL_OUT" | grep -F -- "-> failure" >/dev/null
# ...while the disabled workflow in the SAME repo stays ignored.
printf '%s\n' "$FAIL_OUT" | grep -F "Code Quality Tracker (workflow_disabled)" >/dev/null

if [ "$FAIL_CODE" -eq 0 ]; then
  echo "FAIL: failing fixture returned zero exit code" >&2
  exit 1
fi

# ── 4. --file-issues is an opt-in live-only write, guarded off the fixture ───
# Filing deduplicated tracking issues requires a live sweep; combining the
# write flag with an offline fixture must be rejected (deterministic, no
# network, never creates a real issue).
set +e
GUARD_OUT="$(cargo run --quiet --bin simard -- ci-health --file-issues --from-json "$FAILING_FIXTURE" 2>&1)"
GUARD_CODE=$?
set -e
printf '%s\n' "$GUARD_OUT"
if [ "$GUARD_CODE" -eq 0 ]; then
  echo "FAIL: --file-issues --from-json was accepted (must be rejected)" >&2
  exit 1
fi
printf '%s\n' "$GUARD_OUT" | grep -F -- "--file-issues" >/dev/null
printf '%s\n' "$GUARD_OUT" | grep -F "cannot be combined with" >/dev/null

# The opt-in write flag is advertised in help.
HELP_OUT="$(cargo run --quiet --bin simard -- ci-health --help 2>/dev/null)"
printf '%s\n' "$HELP_OUT" | grep -F -- "--file-issues" >/dev/null
# ── 5. Root-cause diagnosis is advertised as part of the file-issues write ──
# Each newly-filed tracking issue embeds the failing run's failing jobs/steps
# so the tracked failure is actionable without re-fetching logs. The offline
# path cannot exercise the live diagnosis write, but the CLI's documented
# contract must advertise it (behavioural coverage is in the ci_health unit
# tests: parse_run_diagnosis, RunDiagnosis::render, and the steward embedding).
printf '%s\n' "$HELP_OUT" | grep -F "root-cause" >/dev/null

# ── 6. Resolution: --file-issues also closes recovered tracking issues ──────
# The write is bidirectional: besides filing one issue per still-broken
# workflow, the same sweep CLOSES any open tracking issue whose workflow is
# green again, so the fleet keeps exactly one open issue per still-broken
# workflow and none for already-recovered ones. The offline path cannot
# exercise the live close, but the CLI's documented contract must advertise it
# (behavioural coverage is in the ci_health unit tests:
# steward_issue_resolution::* — signature parity with filing, closing only
# green workflows, skipping failing/ignored/cache-served ones, and fail-loud
# search/close error propagation).
printf '%s\n' "$HELP_OUT" | grep -F "green-evidence comment" >/dev/null
printf '%s\n' "$HELP_OUT" | grep -F "still-broken" >/dev/null

echo "ci-health-sweep: PASS"
