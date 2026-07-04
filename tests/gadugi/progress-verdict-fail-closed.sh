#!/usr/bin/env bash
# Outside-in scenario for the reasoner-reliability fix: the progress-evidence
# gate must fail CLOSED on a SEMANTIC verdict parse-miss (a successful,
# non-empty reviewer run that carries no accept/reject verdict), while staying
# fail-OPEN on a genuine infra gap (empty output / transport error). This is
# the sibling policy of the merge judge's fail-closed-to-`Unclear`
# (#2462 / #2463 / #2569).
#
# What this proves, without an LLM, via the in-tree parser + decision tests:
#
#   (a) recipe-progress-checker: non-empty output with no verdict keyword now
#       yields Reject (fail-closed), while EMPTY output stays Accept (fail-open).
#   (b) progress-reviewer (direct-LLM fallback tier): an unknown verdict string
#       and a non-empty unparseable response now yield Reject; an empty response
#       stays Accept.
#   (c) merge judge (#2569 regression): the reporter's exact SUCCESS-with-no-
#       verdict banner (BOTH 30s and 102s) fails closed to `Verdict::Unclear`,
#       never a hard error and never a spurious `ready`.
#
# Each rung captures `cargo test` output and asserts the NAMED tests actually
# ran (a cargo filter that matches zero tests still exits 0), so a future rename
# cannot silently turn this scenario into a no-op.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORK="$(mktemp -d /tmp/simard-progress-fail-closed.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

TEST_LOG="$WORK/progress-verdict-cargo-test.log"

echo "== progress-evidence + merge-judge verdict fail-closed unit/decision tests =="
# Run the three modules' tests together and capture output for name-pinning.
cargo test --lib --locked -- \
    progress_reviewer recipe_progress_checker recipe_merge_judge --nocapture \
    >"$TEST_LOG" 2>&1

PASSED_LINE="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | tail -1 || true)"
echo "final ${PASSED_LINE:-<no ok line>}"
grep -qE 'test result: ok\.' "$TEST_LOG" \
  || { echo "FAIL: cargo test did not report an ok result" >&2; cat "$TEST_LOG" >&2; exit 1; }
if grep -qE 'test result: FAILED' "$TEST_LOG"; then
  echo "FAIL: one or more verdict tests FAILED" >&2; cat "$TEST_LOG" >&2; exit 1
fi

# --- (a) recipe-progress-checker fail-closed on non-empty parse-miss ---------
echo "== (a) recipe-progress-checker: JSON-first robustness + fail-closed parse-miss =="
for t in \
  'recipe_progress_checker::tests::json_accept_verdict_with_reject_in_rationale_accepts' \
  'recipe_progress_checker::tests::json_unknown_verdict_fails_closed' \
  'recipe_progress_checker::tests::text_verdict_no_keyword_fails_closed_to_reject' \
  'recipe_progress_checker::tests::text_verdict_empty_falls_back_to_accept' \
  'recipe_progress_checker::tests::no_verdict_keyword_rejects_unverified_progress_bump' \
  'recipe_progress_checker::tests::log_noise_only_output_is_infra_accept' \
  'recipe_progress_checker::tests::production_success_banner_without_verdict_fails_closed' \
  'recipe_progress_checker::tests::parse_verdict_outcome_reports_match_flag_for_counter'
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected test did not run/pass: $t" >&2; exit 1; }
done
echo "OK: recipe-progress-checker fails closed on a semantic parse-miss."

# --- (b) progress-reviewer (direct-LLM fallback) fail-closed -----------------
echo "== (b) progress-reviewer fallback: unknown verdict / unparseable ⇒ Reject =="
for t in \
  'progress_reviewer::tests::unknown_verdict_fails_closed_to_reject' \
  'progress_reviewer::tests::unparseable_nonempty_response_fails_closed_to_reject' \
  'progress_reviewer::tests::empty_response_falls_back_to_accept' \
  'progress_reviewer::tests::llm_submit_failure_falls_back_to_accept'
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected test did not run/pass: $t" >&2; exit 1; }
done
echo "OK: progress-reviewer fallback fails closed on a semantic parse-miss, open on infra."

# --- (c) merge-judge #2569 regression ---------------------------------------
echo "== (c) merge judge: #2569 banners (30s AND 102s) ⇒ Unclear (fail-closed) =="
grep -qF 'recipe_merge_judge::issue_2428_production_tests::issue_2569_reported_banners_fail_closed_to_unclear ... ok' "$TEST_LOG" \
  || { echo "FAIL: the #2569 banner regression test did not run/pass" >&2; exit 1; }
echo "OK: #2569 SUCCESS-with-no-verdict banners fail closed to Unclear."

echo "PASS: progress-verdict-fail-closed scenario (reasoner reliability; #2569 family)"
