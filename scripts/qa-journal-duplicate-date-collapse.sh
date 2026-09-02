#!/usr/bin/env bash
# qa-journal-duplicate-date-collapse.sh
#
# Regression gate for the dashboard Journal duplicate-date bug.
#
# The bug: the live cognitive store was observed holding TWO
# `journal:YYYY-MM-DD` facts for the same day. The dashboard read path did not
# collapse them, so `GET /api/journal/dates` and `POST /api/journal/search`
# listed the same day TWICE with conflicting PR counts, while
# `GET /api/journal/entry/{date}` returned only one (arbitrary) entry.
#
# The fix collapses same-day facts at READ time, keeping the newest-generated
# entry (`generated_at`), across every read surface.
#
# This gate drives the reproduction through the REAL `LibraryCognitiveMemory`
# backend (not a fake): the two focused tests below inject two same-day journal
# facts via `store_fact` (which bypasses caller-key dedup, exactly reproducing
# the anomaly) and assert the day appears exactly once with the newest
# generation's counts — one at the store boundary, one at the actual
# `journal_dates()` / `journal_search()` HTTP handler boundary. A non-zero exit
# (a lost collapse) fails the scenario; there is no cosmetic-only assertion.
set -uo pipefail

fail() {
  echo "QA-JOURNAL-DUP: FAIL - $1"
  exit 1
}

# The exact tests that reproduce the duplicate-day corruption and assert the
# newest-generation collapse across the store and the dashboard HTTP handler.
TESTS=(
  "journal::tests_store::duplicate_day_facts_collapse_to_newest_in_dates_and_search"
  "journal::tests_store::duplicate_facts_across_multiple_days_each_collapse_independently"
  "operator_commands_dashboard::journal::tests::dates_collapse_duplicate_day_facts_newest_wins"
)

echo "QA-JOURNAL-DUP: running ${#TESTS[@]} duplicate-date collapse regression tests"
# `--exact` + explicit names so the gate is precise; `2>&1` so a compile or
# link failure is surfaced, not swallowed.
if ! cargo test --lib --locked -- --exact "${TESTS[@]}" 2>&1 | tee /tmp/qa-journal-dup.log; then
  fail "duplicate-date collapse regression tests did not pass (see log above)"
fi

# Belt-and-suspenders: confirm all three named tests actually ran and passed,
# so a silent "0 tests ran" (e.g. a renamed test) can never be a false green.
for t in "${TESTS[@]}"; do
  short="${t##*::}"
  grep -Eq "test .*${short} \.\.\. ok" /tmp/qa-journal-dup.log \
    || fail "expected test '${short}' to run and pass, but it did not appear in the results"
done

echo "QA-JOURNAL-DUP: PASS - duplicate journal days collapse to the newest entry on every read surface"
