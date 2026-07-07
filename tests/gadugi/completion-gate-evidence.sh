#!/usr/bin/env bash
# Outside-in scenario for the deploy-aware done-gate
# (`goal_curation::completion_gate`). The gate is the goal-board's merge-gate:
# a goal archives as "complete" ONLY with hard external evidence — a merged PR,
# a closed linked issue, and (for changes to Simard's own running code) a
# verified deploy. Anything short keeps the goal active with a recorded blocker
# instead of silently archiving an evidence-free done-claim.
#
# The gate logic is pure (evidence is injected through `EvidenceSource`), so
# these are real, hermetic tests — no network, no live `gh`. This scenario runs
# the in-tree unit tests and asserts the NAMED tests actually ran and passed
# (a cargo filter that matches zero tests still exits 0), so a future rename
# cannot silently turn this scenario into a no-op.
#
# What this proves:
#   (a) the public, kill-switch-aware entrypoint `archive_completed_evidence_aware`
#       archives only fully-verified goals, retains+annotates unverified ones,
#       leaves non-candidates untouched, and — with the kill switch off —
#       restores the legacy unguarded archive.
#   (b) the production `GhCliEvidenceSource` resolves repo slugs and answers the
#       "no tracked ref" clauses WITHOUT any subprocess (hermetic short-circuit).
#   (c) the headline done-gate correctness holds: an evidence-free Completed goal
#       stays active; a source error fails CLOSED (never archives); a standing
#       goal is never archived.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORK="$(mktemp -d /tmp/simard-completion-gate.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

TEST_LOG="$WORK/completion-gate-cargo-test.log"

echo "== deploy-aware done-gate (completion_gate) unit tests =="
# Run the module's tests and capture output for name-pinning. The filter scopes
# to this module only, so the run stays fast and hermetic (no network/gh).
cargo test --lib --locked -- \
    goal_curation::completion_gate --nocapture \
    >"$TEST_LOG" 2>&1

PASSED_LINE="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | tail -1 || true)"
echo "final ${PASSED_LINE:-<no ok line>}"
grep -qE 'test result: ok\.' "$TEST_LOG" \
  || { echo "FAIL: cargo test did not report an ok result" >&2; cat "$TEST_LOG" >&2; exit 1; }
if grep -qE 'test result: FAILED' "$TEST_LOG"; then
  echo "FAIL: one or more completion-gate tests FAILED" >&2; cat "$TEST_LOG" >&2; exit 1
fi

CG="goal_curation::completion_gate::tests"

# --- (a) archive_completed_evidence_aware: the public kill-switch-aware entry -
echo "== (a) evidence-aware archive: verified archives, unverified retained, kill-switch =="
for t in \
  "${CG}::evidence_aware_archive_archives_fully_verified_goal" \
  "${CG}::evidence_aware_archive_retains_and_annotates_unverified_goals" \
  "${CG}::evidence_aware_archive_leaves_incomplete_goals_in_place" \
  "${CG}::evidence_aware_archive_falls_back_to_legacy_when_kill_switch_off"
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected test did not run/pass: $t" >&2; exit 1; }
done
echo "OK: evidence-aware archive gates on real evidence and honors the kill switch."

# --- (b) GhCliEvidenceSource hermetic pure logic ----------------------------
echo "== (b) GhCliEvidenceSource: repo-slug resolution + no-ref clauses (no network) =="
for t in \
  "${CG}::gh_source_repo_slug_resolves_all_four_forms" \
  "${CG}::gh_source_no_pr_ref_reports_unmerged_without_network" \
  "${CG}::gh_source_no_issue_ref_reports_closed_without_network" \
  "${CG}::gh_source_first_ref_of_kind_matches_case_insensitively"
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected test did not run/pass: $t" >&2; exit 1; }
done
echo "OK: the production evidence source short-circuits ref-less clauses hermetically."

# --- (c) headline done-gate correctness (regression guards) -----------------
echo "== (c) done-gate correctness: evidence-free stays active, source error fails closed =="
for t in \
  "${CG}::archive_keeps_goal_active_when_gate_blocks" \
  "${CG}::blocked_could_not_verify_on_source_error_never_completes" \
  "${CG}::gate_never_archives_a_perpetual_goal_even_with_full_evidence" \
  "${CG}::missing_evidence_label_is_stable_for_every_kind" \
  "${CG}::render_missing_joins_labels_with_semicolons_in_order"
do
  grep -qF "$t ... ok" "$TEST_LOG" \
    || { echo "FAIL: expected test did not run/pass: $t" >&2; exit 1; }
done
echo "OK: the done-gate blocks evidence-free completions and fails closed on error."

echo "PASS: completion-gate-evidence scenario (deploy-aware done-gate; goal-board merge-gate)"
