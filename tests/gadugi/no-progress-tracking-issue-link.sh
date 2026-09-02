#!/usr/bin/env bash
# Outside-in scenario for the UNCLEAR-CRITERIA done-gate fix — the OODA
# no-progress breaker must LINK the human-triage tracking issue it files back
# onto the stalled goal, so the goal's done-criteria become machine-verifiable.
#
# Background. A goal whose done-criteria reference no tracked PR/issue is
# structurally unmeasurable: `has_derivable_signal` stays false and the
# completion done-gate can never verify it (`gh` PR MERGED / issue CLOSED has
# nothing to query). The breaker already escalated such goals to a human by
# FILING a `gh` tracking issue — but it discarded the created issue, never
# linking it back onto the goal. So `wip_refs` stayed empty, the done-gate
# stayed blind, and the goal re-stalled with the identical
# `why=UNCLEAR-CRITERIA … no tracked PR/issue the done-gate can verify`
# diagnosis, cycle after cycle. That is the exact "stalled with no shippable
# progress" symptom the codename `simard-identity-*` goals exhibited.
#
# What this proves, without an LLM, end-to-end through the shared breaker seam:
#
#   (a) The production breaker filer now RETURNS the filed issue and LINKS it:
#       `src/ooda_loop/no_progress.rs` defines `FiledIssue`, the
#       `escalate_with_tracking_issue` helper, `link_tracking_issue` (idempotent
#       back-link), `is_breaker_tracking_ref` (dedupe guard), and the
#       `[no-progress-tracking] ` label prefix that marks the linked ref.
#
#   (b) The in-tree Rust behavior tests are exercised end-to-end via `cargo
#       test` and ACTUALLY run (a filter that matches zero tests still exits 0,
#       so a future rename must not silently turn this rung into a no-op):
#       - the first escalation of an UNCLEAR-CRITERIA goal files exactly one
#         tracking issue and links it back, flipping the goal from
#         "no tracked PR/issue" to a done-gate-verifiable artifact;
#       - a re-escalation is idempotent — it never appends a duplicate tracking
#         ref and never spams a second `ooda-stuck` issue.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

SRC="src/ooda_loop/no_progress.rs"

# --- (a) production breaker returns + links the filed tracking issue ----------
echo "== (a) production breaker returns FiledIssue + links it back onto the goal =="
grep -qF 'struct FiledIssue' "$SRC" \
  || { echo "FAIL: $SRC is missing the FiledIssue return type" >&2; exit 1; }
grep -qF 'fn escalate_with_tracking_issue' "$SRC" \
  || { echo "FAIL: $SRC is missing the escalate_with_tracking_issue helper" >&2; exit 1; }
grep -qF 'fn link_tracking_issue' "$SRC" \
  || { echo "FAIL: $SRC is missing the link_tracking_issue back-link" >&2; exit 1; }
grep -qF 'fn is_breaker_tracking_ref' "$SRC" \
  || { echo "FAIL: $SRC is missing the is_breaker_tracking_ref dedupe guard" >&2; exit 1; }
grep -qF 'NO_PROGRESS_TRACKING_LABEL_PREFIX' "$SRC" \
  || { echo "FAIL: $SRC is missing the tracking-ref label prefix" >&2; exit 1; }
echo "OK: the tracking-issue return + back-link + dedupe guard are present in production code."

WORK="$(mktemp -d /tmp/simard-no-progress-link.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

# --- (b) in-tree behavior tests genuinely run and pass ------------------------
echo "== (b) in-tree tracking-issue-link behavior tests =="
TEST_LOG="$WORK/no-progress-link-cargo-test.log"
# Scope to the no-progress test modules (a single cargo filter substring matches
# them all). `--nocapture` so the per-test names appear in the log for the
# by-name pins below.
cargo test --lib --locked no_progress -- --nocapture >"$TEST_LOG" 2>&1

PASSED="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" \
  | grep -oE '[0-9]+' | awk '{s+=$1} END {print s+0}')"
echo "no-progress tests passed: ${PASSED}"
[ "$PASSED" -ge 1 ] \
  || { echo "FAIL: the no-progress filter matched zero tests — module rename silently no-op'd this rung" >&2; cat "$TEST_LOG" >&2; exit 1; }

# Pin the two representative tests by name so the back-link + idempotence are
# GENUINELY covered.
grep -qF 'unclear_criteria_escalation_links_tracking_issue_making_criteria_measurable' "$TEST_LOG" \
  || { echo "FAIL: the tracking-issue back-link test did not run" >&2; cat "$TEST_LOG" >&2; exit 1; }
grep -qF 're_escalation_is_idempotent_no_duplicate_tracking_issue' "$TEST_LOG" \
  || { echo "FAIL: the re-escalation idempotence test did not run" >&2; cat "$TEST_LOG" >&2; exit 1; }

grep -qE 'test result: ok\.' "$TEST_LOG" \
  || { echo "FAIL: cargo reported a non-ok result for the no-progress tests" >&2; cat "$TEST_LOG" >&2; exit 1; }
echo "OK: tracking-issue back-link + re-escalation idempotence pass (${PASSED} tests)."

echo "PASS: no-progress-tracking-issue-link scenario (UNCLEAR-CRITERIA done-gate fix)"
