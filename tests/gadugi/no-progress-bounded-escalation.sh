#!/usr/bin/env bash
# Outside-in scenario for the issue #16 follow-up — the OODA no-progress
# breaker's BOUNDED evidence-less re-investigation.
#
# Background. Issue #16 (#4096) fixed the live-daemon defect of parking a stalled
# goal with a bare `🔒 [OODA-SAFEGUARD] … why=GENUINELY-STUCK evidence=[(none)]`
# block: it made the evidence-less terminal rung *non-terminal* — it surfaces the
# failure and lets the goal re-investigate next cycle. But an *unbounded*
# re-investigation is its OWN livelock: a goal whose done-criteria are
# permanently unclear (the six `simard-identity-*` codename goals) re-investigates
# → produces no evidence → surfaces → resets → forever, making NO shippable
# progress and NEVER reaching a human. That is the exact "stalled with no
# shippable progress" symptom.
#
# What this proves, without an LLM, end-to-end through the shared breaker seam:
#
#   (a) The production policy pins the bound: the
#       `SURFACED_INVESTIGATION_FAILURE_LIMIT` constant exists in
#       `src/goal_curation/no_progress_breaker.rs` and the surfaced-failure
#       counter is wired into the tracker.
#
#   (b) The in-tree Rust behavior tests are exercised end-to-end via `cargo test`
#       and ACTUALLY run (a filter that matches zero tests still exits 0, so a
#       future rename must not silently turn this rung into a no-op):
#       - the bound is reached and the goal is ESCALATED to a human, with the
#         re-investigation count as concrete evidence (never evidence=[(none)])
#         and a measurable make-the-done-criteria-machine-checkable ask;
#       - real progress RESETS the surfaced-failure window (a transient
#         investigation hiccup never accumulates toward a spurious escalation);
#       - the pre-existing invariant that an evidence-less terminal outcome is
#         surfaced (never parked with `(none)`) is preserved, not regressed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

BREAKER="src/goal_curation/no_progress_breaker.rs"

# --- (a) production policy pins the bound + wires the counter -----------------
echo "== (a) production breaker pins SURFACED_INVESTIGATION_FAILURE_LIMIT + counter =="
grep -qF 'pub const SURFACED_INVESTIGATION_FAILURE_LIMIT' "$BREAKER" \
  || { echo "FAIL: $BREAKER is missing the SURFACED_INVESTIGATION_FAILURE_LIMIT bound" >&2; exit 1; }
grep -qF 'fn record_surfaced_failure' "$BREAKER" \
  || { echo "FAIL: $BREAKER is missing the surfaced-failure counter wiring" >&2; exit 1; }
echo "OK: the surfaced-failure bound and counter are present in production code."

WORK="$(mktemp -d /tmp/simard-no-progress-bound.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

# --- (b) in-tree behavior tests genuinely run and pass ------------------------
echo "== (b) in-tree bounded-escalation behavior tests =="
TEST_LOG="$WORK/no-progress-cargo-test.log"
# Scope to the no-progress breaker + investigation test modules (a single cargo
# filter substring matches them all). `--nocapture` so the per-test names appear
# in the log for the by-name pins below.
cargo test --lib --locked no_progress -- --nocapture >"$TEST_LOG" 2>&1

PASSED="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" \
  | grep -oE '[0-9]+' | awk '{s+=$1} END {print s+0}')"
echo "no-progress tests passed: ${PASSED}"
[ "$PASSED" -ge 1 ] \
  || { echo "FAIL: the no-progress filter matched zero tests — module rename silently no-op'd this rung" >&2; cat "$TEST_LOG" >&2; exit 1; }

# Pin the representative tests by name so the bound, the escalation evidence, the
# human triage ask, and the reset window are all GENUINELY covered.
grep -qF 'evidenceless_reinvestigation_is_bounded_then_escalated_to_a_human' "$TEST_LOG" \
  || { echo "FAIL: the bounded-escalation test did not run" >&2; cat "$TEST_LOG" >&2; exit 1; }
grep -qF 'real_progress_resets_the_surfaced_failure_counter' "$TEST_LOG" \
  || { echo "FAIL: the surfaced-failure reset test did not run" >&2; cat "$TEST_LOG" >&2; exit 1; }
grep -qF 'genuinely_stuck_with_no_evidence_surfaces_investigation_error_never_parks_none' "$TEST_LOG" \
  || { echo "FAIL: the never-parks-(none) invariant test did not run (regression guard lost)" >&2; cat "$TEST_LOG" >&2; exit 1; }

# All three must be reported passing (not failed/ignored).
grep -qE 'test result: ok\.' "$TEST_LOG" \
  || { echo "FAIL: cargo reported a non-ok result for the no-progress tests" >&2; cat "$TEST_LOG" >&2; exit 1; }
echo "OK: bounded escalation + evidence shape + human ask + reset window pass (${PASSED} tests)."

echo "PASS: no-progress-bounded-escalation scenario (issue #16 follow-up)"
