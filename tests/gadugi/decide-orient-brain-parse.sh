#!/usr/bin/env bash
# Outside-in scenario for issue #4785 (Group A) — the decide + orient RecipeBrain
# phases now ACT through a typed `simard ooda record-decide|record-orient` tool
# + a fail-CLOSED reader, exactly like the #4734 per-goal-cycle seam. The former
# "recipe emits a JSON envelope / first action word / bare decimal → Rust scrapes
# stdout → Rust acts" pattern (issue #2421) is GONE.
#
# What replaced it:
#   - decide: the recipe instructs the agent to run `simard ooda record-decide`
#     (writing a typed DecideDecisionRecord); judge_decision reads it via
#     read_verified_decide and NEVER scrapes stdout. A parse/verify failure is an
#     explicit Err — never a fabricated `advance_goal`.
#   - orient: the recipe instructs the agent to run `simard ooda record-orient`
#     (writing a typed OrientDecisionRecord, including the persisted base_urgency
#     for the reader's self-consistent no-escalation re-check); judge_orientation
#     reads it via read_verified_orient. A failure keeps the goal's BASE urgency —
#     never a fabricated demotion scraped from a banner timing string.
#
# This scenario validates that new contract WITHOUT an LLM:
#   (a) both recipes call the record tool and declare that NOTHING is scraped
#       from stdout (the source-of-truth prompt contract), and
#   (b) the in-tree Rust seam proves it: the round-trip + R1–R8 fail-closed
#       reader tests (tests_record_orient_decide) and the source/recipe rework
#       contract (tests_rework_contract) that asserts the old scrape machinery is
#       deleted and the typed-record seam is present.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

DECIDE_RECIPE="prompt_assets/simard/recipes/ooda-decide.yaml"
ORIENT_RECIPE="prompt_assets/simard/recipes/ooda-orient.yaml"

# ---------------------------------------------------------------------------
# (a) The recipe prompts ACT via the record tool and scrape NOTHING from stdout.
# ---------------------------------------------------------------------------
echo "== (a) decide recipe records via the tool, declares no stdout scraping =="
grep -qF 'ooda record-decide' "$DECIDE_RECIPE" \
  || { echo "FAIL: ooda-decide.yaml must call 'simard ooda record-decide'" >&2; exit 1; }
for flag in '--record-path' '--goal-id' '--cycle-number'; do
  grep -qF -- "$flag" "$DECIDE_RECIPE" \
    || { echo "FAIL: ooda-decide.yaml tool call must pass ${flag}" >&2; exit 1; }
done
grep -qiF 'none scraped from stdout' "$DECIDE_RECIPE" \
  || { echo "FAIL: ooda-decide.yaml must declare 'Output: NONE scraped from stdout'" >&2; exit 1; }
# The forbidden emit->scrape instructions must be gone.
grep -qF '"decision"' "$DECIDE_RECIPE" \
  && { echo "FAIL: ooda-decide.yaml still instructs a {\"decision\": ...} envelope scrape" >&2; exit 1; }
grep -qiF 'first word' "$DECIDE_RECIPE" \
  && { echo "FAIL: ooda-decide.yaml still instructs a first-word stdout scrape" >&2; exit 1; }
echo "OK: decide recipe acts via record-decide; no stdout scraping."

echo "== (a) orient recipe records via the tool, declares no stdout scraping =="
grep -qF 'ooda record-orient' "$ORIENT_RECIPE" \
  || { echo "FAIL: ooda-orient.yaml must call 'simard ooda record-orient'" >&2; exit 1; }
for flag in '--record-path' '--goal-id' '--cycle-number' '--base-urgency'; do
  grep -qF -- "$flag" "$ORIENT_RECIPE" \
    || { echo "FAIL: ooda-orient.yaml tool call must pass ${flag}" >&2; exit 1; }
done
grep -qiF 'none scraped from stdout' "$ORIENT_RECIPE" \
  || { echo "FAIL: ooda-orient.yaml must declare 'Output: NONE scraped from stdout'" >&2; exit 1; }
grep -qiF 'bare decimal' "$ORIENT_RECIPE" \
  && { echo "FAIL: ooda-orient.yaml still instructs a bare-decimal stdout scrape" >&2; exit 1; }
grep -qiF 'first token' "$ORIENT_RECIPE" \
  && { echo "FAIL: ooda-orient.yaml still instructs a first-token stdout scrape" >&2; exit 1; }
echo "OK: orient recipe acts via record-orient; no stdout scraping."

# ---------------------------------------------------------------------------
# (b) In-tree Rust seam: typed-record round-trip + R1–R8 fail-closed reader, and
#     the source/recipe rework contract (old scrape machinery deleted, typed seam
#     present).
# ---------------------------------------------------------------------------
WORK="$(mktemp -d /tmp/simard-decide-orient-4785.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

echo "== (b) typed-record reader + round-trip tests (tests_record_orient_decide) =="
REC_LOG="$WORK/record-cargo-test.log"
cargo test --lib --locked ooda_brain::tests_record_orient_decide -- --nocapture >"$REC_LOG" 2>&1
REC_PASSED="$(grep -oE 'test result: ok\. [0-9]+ passed' "$REC_LOG" | grep -oE '[0-9]+' | head -1)"
REC_PASSED="${REC_PASSED:-0}"
echo "record seam tests passed: ${REC_PASSED}"
[ "$REC_PASSED" -ge 1 ] \
  || { echo "FAIL: tests_record_orient_decide matched zero tests (module renamed?)" >&2; cat "$REC_LOG" >&2; exit 1; }
grep -qF 'read_verified_decide_round_trips_every_variant' "$REC_LOG" \
  || { echo "FAIL: the decide 10-variant round-trip test did not run" >&2; exit 1; }
grep -qF 'orient_r4_escalating_record_fails_closed' "$REC_LOG" \
  || { echo "FAIL: the orient no-escalation fail-closed test did not run" >&2; exit 1; }
grep -qF 'orient_r6_goal_id_mismatch_fails_closed' "$REC_LOG" \
  || { echo "FAIL: the goal-id identity-binding fail-closed test did not run" >&2; exit 1; }

echo "== (b) source/recipe rework contract (tests_rework_contract) =="
CON_LOG="$WORK/contract-cargo-test.log"
cargo test --lib --locked ooda_brain::tests_rework_contract -- --nocapture >"$CON_LOG" 2>&1
CON_PASSED="$(grep -oE 'test result: ok\. [0-9]+ passed' "$CON_LOG" | grep -oE '[0-9]+' | head -1)"
CON_PASSED="${CON_PASSED:-0}"
echo "rework contract tests passed: ${CON_PASSED}"
[ "$CON_PASSED" -ge 1 ] \
  || { echo "FAIL: tests_rework_contract matched zero tests (module renamed?)" >&2; cat "$CON_LOG" >&2; exit 1; }
grep -qF 'recipe_brain_has_no_orient_decide_scrape_machinery' "$CON_LOG" \
  || { echo "FAIL: the 'old scrape machinery deleted' contract did not run" >&2; exit 1; }
grep -qF 'recipe_brain_judge_seams_read_the_typed_record' "$CON_LOG" \
  || { echo "FAIL: the 'judge seams read the typed record' contract did not run" >&2; exit 1; }
echo "OK: typed-record seam present; old decide/orient stdout-scrape machinery deleted."

echo "PASS: decide-orient-brain typed-record seam scenario (issue #4785, Group A)"
