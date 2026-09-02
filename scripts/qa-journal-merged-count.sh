#!/usr/bin/env bash
# qa-journal-merged-count.sh
#
# Regression gate for issue #4140: the dashboard Journal tab's `merged` PR count
# must reflect the day's real landed changes, not be structurally zero.
#
# Root cause (fixed): the production journal PR source (`GhPrListSource`) only
# ingested OPEN PRs and mapped every one to a "still open — …" outcome, so
# `JournalEntry::merged_pr_count()` — which counts `outcome == "merged"` rows —
# could never be non-zero. `/api/journal/dates` therefore always reported
# `"merged": 0`, even on days with many merges.
#
# This script is a hermetic, network-free pass/fail gate. It:
#   1. Runs the deterministic unit regressions that prove the seam now feeds
#      merged PRs through with the canonical "merged" outcome and that
#      `merged_pr_count()` is non-zero (and survives persistence).
#   2. Structurally guards the production wiring so a future edit cannot quietly
#      stub the merged-PR path back to empty without also deleting these lines.
#
# A non-zero exit on any failed assertion is treated by the gadugi `cli` agent
# runner as a step failure, so this is a real gate (not a cosmetic assertion).
set -uo pipefail

fail() {
  echo "QA-JOURNAL-MERGED: FAIL - $1"
  exit 1
}

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || fail "cannot cd to repo root"

# 1. Deterministic unit regressions (no network, no daemon). These fail loudly
#    if the merged-PR outcome mapping, the merge counter, or the honest
#    degradation on a gh blip regress.
TESTS=(
  "journal::tests_pr_source::gh_source_appends_the_days_merged_prs_with_merged_outcome"
  "journal::tests_pr_source::merged_pr_count_reflects_the_days_landed_changes"
  "journal::tests_pr_source::merged_pr_fetch_failure_keeps_open_rows_and_degrades_merges"
  "stewardship::merge_authority::tests::parse_merged_pr_list_json_round_trips_journal_shape"
  "stewardship::merge_authority::tests::parse_merged_pr_list_json_accepts_empty_array"
)
echo "QA-JOURNAL-MERGED: running ${#TESTS[@]} deterministic regressions…"
if ! cargo test --quiet --lib -- --exact "${TESTS[@]}"; then
  fail "journal merged-count regression tests failed"
fi

# 2. Structural guards: the production source must actually call the merged-PR
#    fetch and tag rows "merged", and RealPrGhClient must implement it. This
#    catches a regression that reverts the wiring while leaving the tests named.
grep -q "list_merged_prs" src/journal/pr_source.rs \
  || fail "GhPrListSource no longer fetches merged PRs (list_merged_prs missing)"
grep -q "merged_pr_to_summary" src/journal/pr_source.rs \
  || fail "GhPrListSource no longer maps merged PRs to journal rows"
grep -q "fn list_merged_prs" src/stewardship/merge_authority.rs \
  || fail "RealPrGhClient no longer implements list_merged_prs"

echo "QA-JOURNAL-MERGED: PASS - Journal merged-PR count reflects real merges (#4140)"
