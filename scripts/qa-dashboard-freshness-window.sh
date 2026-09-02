#!/usr/bin/env bash
# qa-dashboard-freshness-window.sh
#
# End-to-end regression gate for the dashboard snapshot-freshness fix
# (issue #4278): a metrics snapshot flushed ONCE PER OODA cycle must read as
# `live`, not a false `stale`, while the daemon is actually running.
#
# The daemon flushes `<state_root>/telemetry/metrics_snapshot.json` exactly once
# per OODA cycle, so a healthy reader routinely sees a `captured_at` several
# hundred seconds old (cycle runtime + the ~300s inter-cycle sleep). The pre-fix
# freshness window was hardcoded to 300s in TWO independent classifiers
# (`status::provider::snapshot_is_stale` and the `/api/enrichment` endpoint), so
# both fired `stale` on essentially every healthy cycle — directly contradicting
# the dashboard's own daemon-liveness check (`/api/status`), which treats a
# heartbeat as `running` for up to 900s.
#
# The fix single-sources the window as `telemetry::snapshot::FRESHNESS_SECS = 900`
# and references it from BOTH classifiers, so the freshness bound can never again
# drift below the once-per-cycle flush cadence or the daemon-liveness bound.
#
# This script is hermetic (no network, no live host): it drives the pure
# freshness classifiers through their unit/regression tests and adds a structural
# guard so the exact `300` hardcode / dual-source drift cannot silently return.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || { echo "QA-DASHBOARD-FRESHNESS-WINDOW: FAIL - cannot cd to repo root"; exit 1; }

SNAP="src/telemetry/snapshot.rs"
PROVIDER="src/status/provider.rs"
ENRICHMENT="src/operator_commands_dashboard/enrichment.rs"

fail() {
  echo "QA-DASHBOARD-FRESHNESS-WINDOW: FAIL - $1"
  exit 1
}

# 1) The freshness classifiers must pass their regression tests. These prove a
#    once-per-cycle-aged snapshot (600s) is `live` and a genuinely-old snapshot
#    (>900s) is `stale`, across BOTH the status provider and the enrichment
#    endpoint.
cargo test --locked --lib -- \
  once_per_cycle_aged_snapshot_is_live_not_stale \
  genuinely_old_snapshot_is_stale \
  snapshot_is_stale_uses_freshness_window \
  2>&1 | tee /tmp/qa-dfw-test.log \
  || fail "cargo test for freshness-window regression tests failed"
grep -q "test result: ok" /tmp/qa-dfw-test.log \
  || fail "freshness-window regression tests did not report 'test result: ok'"
grep -q "3 passed" /tmp/qa-dfw-test.log \
  || fail "expected all 3 freshness-window regression tests to run and pass"

# 2) Structural guard: the window must be single-sourced at 900s and referenced
#    by both classifiers, and no classifier may reintroduce a 300s hardcode.
grep -qE 'pub const FRESHNESS_SECS: i64 = 900;' "$SNAP" \
  || fail "single-sourced FRESHNESS_SECS=900 is missing from $SNAP"
grep -q 'crate::telemetry::snapshot::FRESHNESS_SECS' "$PROVIDER" \
  || fail "$PROVIDER no longer references the shared FRESHNESS_SECS (window drift risk)"
grep -q 'crate::telemetry::snapshot::FRESHNESS_SECS' "$ENRICHMENT" \
  || fail "$ENRICHMENT no longer references the shared FRESHNESS_SECS (window drift risk)"
if grep -nE '(SNAPSHOT_)?FRESHNESS_SECS:[[:space:]]*i64[[:space:]]*=[[:space:]]*300' "$PROVIDER" "$ENRICHMENT"; then
  fail "a 300s freshness hardcode reappeared — a running daemon's once-per-cycle snapshot would read false-stale again"
fi

echo "QA-DASHBOARD-FRESHNESS-WINDOW: PASS - snapshot freshness window is single-sourced at 900s; a once-per-cycle snapshot reads live, not false-stale"
