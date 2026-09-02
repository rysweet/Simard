#!/usr/bin/env bash
# qa-journal-past-day-reconcile.sh
#
# Regression gate for issue #4225: the dashboard Journal tab (and the Overview
# journal date list) must NOT under-report a PAST day's merged-PR count.
#
# The bug: a journal entry only (re)generates while it is *today*; once the day
# passes it FREEZES (`journal::thread` builds only `clock.today()`). So a day
# whose PRs merged after its final tick — or whose entry froze before the #4140
# merged-PR wiring shipped — reports `merged: 0` forever, even after ten PRs
# landed. `GET /api/journal/dates` faithfully surfaces the frozen zero.
#
# The fix (this gate guards it): a merged-ONLY reconciliation pass
# (`journal::reconcile`) revisits the last few past days on each journal tick
# and folds their real merges back into the frozen entry — upgrading a
# "still open" row to `merged`, appending any merged PR the entry never saw —
# while never touching today, never fabricating an entry for an absent day, and
# degrading honestly (never erasing data) on a `gh` blip.
#
# This is a hermetic, network-free pass/fail gate. It:
#   1. Runs the deterministic unit regressions that prove the pure fold and the
#      driver behave (upgrade/append/idempotent/no-downgrade/quiet-flip, plus
#      never-touch-today, skip-absent, and honest gh-blip degradation).
#   2. Structurally guards the production wiring so a future edit cannot quietly
#      unwire the daemon reconciliation call or swap the merged-only seam for an
#      open-PR one without also deleting these lines.
#
# A non-zero exit on any failed assertion is treated by the gadugi `cli` agent
# runner as a step failure, so this is a real gate (not a cosmetic assertion).
set -uo pipefail

fail() {
  echo "QA-JOURNAL-RECONCILE: FAIL - $1"
  exit 1
}

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || fail "cannot cd to repo root"

# 1. Deterministic unit regressions (no network, no daemon). These fail loudly
#    if the pure fold or the driver's safety invariants regress.
TESTS=(
  "journal::tests_reconcile::upgrades_a_still_open_row_to_merged"
  "journal::tests_reconcile::appends_a_merged_pr_the_frozen_entry_never_saw"
  "journal::tests_reconcile::is_idempotent_when_the_entry_already_reflects_the_merge"
  "journal::tests_reconcile::never_downgrades_or_erases_existing_rows"
  "journal::tests_reconcile::does_not_double_count_when_a_merged_row_for_the_pr_already_exists"
  "journal::tests_reconcile::folding_a_merge_flips_a_quiet_day_to_not_quiet"
  "journal::tests_reconcile::backfills_a_frozen_past_day_and_persists_the_real_count"
  "journal::tests_reconcile::never_touches_today"
  "journal::tests_reconcile::skips_absent_days_without_fabricating_an_entry"
  "journal::tests_reconcile::degrades_honestly_on_a_gh_blip_and_carries_on"
  "journal::tests_reconcile::a_second_pass_is_idempotent"
)
echo "QA-JOURNAL-RECONCILE: running ${#TESTS[@]} deterministic regressions…"
if ! cargo test --lib --locked -- --exact "${TESTS[@]}" 2>&1 | tee /tmp/qa-journal-reconcile.log; then
  fail "journal past-day reconciliation regression tests failed"
fi

# Belt-and-suspenders: confirm each named test actually ran and passed, so a
# silent "0 tests ran" (e.g. a renamed test) can never be a false green.
for t in "${TESTS[@]}"; do
  short="${t##*::}"
  grep -Eq "test .*${short} \.\.\. ok" /tmp/qa-journal-reconcile.log \
    || fail "expected test '${short}' to run and pass, but it did not appear in the results"
done

# 2. Structural guards: the reconciliation must actually be wired into the
#    daemon journal tick, and it must flow through the merged-ONLY seam (so a
#    past-day backfill can never graft today's still-open PRs onto history).
grep -q "reconcile_recent_days" src/operator_commands_ooda/daemon/mod.rs \
  || fail "daemon journal tick no longer runs the past-day reconciliation pass"
grep -q "GhMergedPrSource" src/operator_commands_ooda/daemon/mod.rs \
  || fail "daemon reconciliation no longer uses the merged-only GhMergedPrSource seam"
grep -q "trait MergedPrSource" src/journal/reconcile.rs \
  || fail "the merged-only MergedPrSource seam is gone"
grep -q "fn merged_prs_for_date" src/journal/reconcile.rs \
  || fail "MergedPrSource no longer exposes a merged-only per-date fetch"

echo "QA-JOURNAL-RECONCILE: PASS - past-day merged counts are reconciled to reality (#4225)"
