#!/usr/bin/env bash
# qa-dashboard-daily-report-window.sh
#
# End-to-end regression gate for the dashboard Overview "daily report" fix:
# (issue #4256) `prs_merged` and `bugs_fixed` must be the count of activity WITHIN the report's
# 24-hour window, not a constant capped at 5.
#
# The pre-fix collectors (`collect_prs_merged` / `collect_bugs_fixed` in
# `src/self_metrics/mod.rs`) ran `gh ... --limit 5 --json number` with NO time
# filter, so both metrics were structurally pinned at `min(5, total-ever) = 5.0`
# every cycle — the Overview tile perpetually claimed "5 PRs merged / 5 bugs
# fixed (24h)" even on days with 50+ merges, silently misreporting throughput to
# the operator.
#
# This script is hermetic (no network, no live host): it drives the pure,
# time-window-filtering core (`count_entries_since`) through its unit tests and
# adds a structural guard so the exact `--limit 5` / count-by-`number` bug cannot
# silently return.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || { echo "QA-DASHBOARD-DAILY-REPORT: FAIL - cannot cd to repo root"; exit 1; }

MOD="src/self_metrics/mod.rs"

fail() {
  echo "QA-DASHBOARD-DAILY-REPORT: FAIL - $1"
  exit 1
}

# 1) The pure window-filter core must pass its unit tests. These prove the count
#    reflects only entries inside the window and is NOT capped at five.
cargo test --locked --lib self_metrics::tests::count_entries_since 2>&1 | tee /tmp/qa-ddrw-test.log \
  || fail "cargo test for count_entries_since failed"
grep -q "test result: ok" /tmp/qa-ddrw-test.log \
  || fail "count_entries_since unit tests did not report 'test result: ok'"
grep -q "count_entries_since_counts_all_in_window_not_capped_at_five" /tmp/qa-ddrw-test.log \
  || fail "the >5-in-window regression test did not run (cap-at-five guard missing)"

# 2) Structural guard: the merge/close collectors must count by the merge/close
#    TIMESTAMP and must NOT reintroduce the bare `--limit 5` count-by-`number`
#    cap on the activity path.
grep -q '"mergedAt"' "$MOD" \
  || fail "collect_prs_merged no longer references mergedAt (lost its time window)"
grep -q '"closedAt"' "$MOD" \
  || fail "collect_bugs_fixed no longer references closedAt (lost its time window)"
if grep -nE '"--limit",[[:space:]]*"5"' "$MOD"; then
  fail "a '--limit 5' cap reappeared in $MOD — the daily-report metric would pin at 5 again"
fi

echo "QA-DASHBOARD-DAILY-REPORT: PASS - daily-report prs_merged/bugs_fixed are 24h-windowed counts, not a capped-at-5 constant"
