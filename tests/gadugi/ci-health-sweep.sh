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
# verdict, that the opt-in `--file-issues` write is guarded off the offline
# fixture path (a live-only sweep) and advertised in help, and that a cross-repo
# authorization skip (an unwritable governed sibling) is a resilient reported
# skip — not a sweep-aborting error — per the advertised contract.
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
# AND the concrete error text (its check-run failure annotations) so the tracked
# failure is actionable without re-fetching logs. The offline path cannot
# exercise the live diagnosis write, but the CLI's documented contract must
# advertise it (behavioural coverage is in the ci_health unit tests:
# parse_run_diagnosis, parse_failure_annotations, RunDiagnosis::render, and the
# steward embedding).
printf '%s\n' "$HELP_OUT" | grep -F "root-cause" >/dev/null
printf '%s\n' "$HELP_OUT" | grep -F "failure annotations" >/dev/null

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

# ── 7. --exit-zero suppresses the red-fleet non-zero exit (scheduled sweep) ──
# The unattended scheduled runner (.github/workflows/ci-health.yml) reports a
# broken fleet via the filed tracking issue, not a red run; letting the run go
# red would make Simard's own ci-health workflow a fresh actionable failure the
# next sweep re-detects. So `--exit-zero` on the SAME failing fixture still
# prints the FAILING verdict but exits 0.
set +e
EXITZERO_OUT="$(run_ci_health --exit-zero --from-json "$FAILING_FIXTURE")"
EXITZERO_CODE=$?
set -e
printf '%s\n' "$EXITZERO_OUT"
printf '%s\n' "$EXITZERO_OUT" | grep -F "CI-HEALTH: FAILING" >/dev/null
if [ "$EXITZERO_CODE" -ne 0 ]; then
  echo "FAIL: --exit-zero returned non-zero on a red fleet (verdict must be suppressed)" >&2
  exit 1
fi
# The flag is advertised in help as the scheduled-sweep escape hatch.
printf '%s\n' "$HELP_OUT" | grep -F -- "--exit-zero" >/dev/null

# ── 8. Cross-repo authorization skips are resilient, not sweep-aborting ──────
# A failing governed *sibling* repo the run's token cannot write (the default
# GITHUB_TOKEN when STEWARD_GH_TOKEN is absent) is reported as an unauthorized
# skip and must NOT abort the sweep — every writable repo is still reconciled
# and the scheduled run stays green, avoiding the self-referential red-run loop
# that a fail-the-whole-run abort used to cause. The offline path cannot
# exercise a live cross-repo denial, but the CLI's documented contract must
# advertise the resilient-skip behavior (behavioural coverage is in the
# ci_health unit tests: steward_issue_filing::a_cross_repo_write_denial_*,
# a_forbidden_403_*, a_non_authorization_gh_error_still_fails_loud, and
# steward_issue_resolution::a_cross_repo_read_denial_*).
printf '%s\n' "$HELP_OUT" | grep -F "unauthorized skip" >/dev/null
printf '%s\n' "$HELP_OUT" | grep -F "STEWARD_GH_TOKEN" >/dev/null

echo "ci-health-sweep: PASS"
