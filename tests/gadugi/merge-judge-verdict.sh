#!/usr/bin/env bash
# Outside-in scenario for issue #4721 (WS-2) — rework the merge-judge to REMOVE
# the forbidden "recipe emits JSON → Rust scrapes stdout → Rust acts" pattern.
#
# The old flow (this file's previous incarnation, #2428/#2462/#2463) proved the
# judge surfaced its verdict through the recipe-runner JSON envelope. #4721
# DELETES that transport: the merge-readiness recipe now RECORDS a typed verdict
# via the agent-facing `simard merge record-verdict` tool (the same act-via-tool
# pattern as `distill-episodes.yaml` → `simard memory remember`), prints NO JSON
# envelope, and the thin deterministic rail READS the typed record and
# INDEPENDENTLY re-verifies the hard safety gates (mergeable, not draft, CI
# green, allow-listed base) before authorizing any merge.
#
# What this scenario proves, deterministically (no LLM):
#   (a) The recipe records via the tool and emits no JSON verdict envelope.
#   (b) The rail source no longer contains the forbidden scrape (no
#       parse_merge_verdict_from_text / step_results / --output-format json) and
#       never weakens the gate (no --admin / --no-verify).
#   (c) The in-tree store + rail + CLI unit tests pass (the decision matrix:
#       merge+red-CI ⇒ refused, merge+draft ⇒ refused, hold ⇒ not-ready,
#       merge+green ⇒ ready, missing/stale record ⇒ unclear).
#   (d) End-to-end at the binary boundary: `simard merge record-verdict` writes
#       a durable, typed, freshness-tokened record a rail can read; a bogus
#       verdict is rejected with exit 2 and writes nothing.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

RECIPE="prompt_assets/simard/recipes/merge-readiness-judge.yaml"
RAIL="src/stewardship/recipe_merge_judge.rs"

# --- (a) recipe records via the tool and prints no JSON verdict envelope ------
echo "== (a) recipe acts via 'simard merge record-verdict', no JSON envelope =="
grep -qF 'merge record-verdict' "$RECIPE" \
  || { echo "FAIL: $RECIPE does not record its verdict via the record-verdict tool" >&2; exit 1; }
grep -qF 'run_token' "$RECIPE" \
  || { echo "FAIL: $RECIPE does not thread the rail-supplied run_token to the tool" >&2; exit 1; }
if grep -qF '{"verdict"' "$RECIPE"; then
  echo "FAIL: $RECIPE still prints a JSON verdict envelope for the daemon to scrape" >&2
  exit 1
fi
echo "OK: recipe records via the tool and emits no JSON envelope."

# --- (b) the rail no longer scrapes stdout and never weakens the gate ----------
echo "== (b) rail has no JSON scrape and no gate-weakening flags =="
for forbidden in parse_merge_verdict_from_text step_results extract_recipe_decision_output '--output-format'; do
  if grep -qF -- "$forbidden" "$RAIL"; then
    echo "FAIL: $RAIL still references the forbidden scrape token '$forbidden'" >&2
    exit 1
  fi
done
if grep -qF -- '--admin' "$RAIL" || grep -qF -- '--no-verify' "$RAIL"; then
  echo "FAIL: $RAIL must NEVER pass --admin/--no-verify to gh pr merge" >&2
  exit 1
fi
grep -qF 'merge_verdict_store' "$RAIL" \
  || { echo "FAIL: $RAIL must READ the typed verdict via merge_verdict_store" >&2; exit 1; }
grep -qF 'evaluate_objective_gates' "$RAIL" \
  || { echo "FAIL: $RAIL must INDEPENDENTLY re-verify the objective gates" >&2; exit 1; }
echo "OK: rail reads the typed record and independently re-verifies gates."

# --- (c) in-tree store + rail + CLI unit tests --------------------------------
echo "== (c) in-tree store + rail + CLI unit tests (#4721) =="
WORK="$(mktemp -d /tmp/simard-merge-judge-4721.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
TEST_LOG="$WORK/cargo-test.log"
cargo test --lib --locked \
  -- merge_verdict_store_tests issue_4721 --nocapture >"$TEST_LOG" 2>&1 \
  || { echo "FAIL: #4721 in-tree tests did not pass" >&2; cat "$TEST_LOG" >&2; exit 1; }
for must_run in \
  merge_verdict_store_tests::write_then_read_verified_round_trips_all_fields \
  issue_4721_rail_tests::merge_verdict_with_red_ci_is_refused \
  issue_4721_rail_tests::merge_verdict_with_draft_pr_is_refused \
  issue_4721_rail_tests::hold_verdict_is_not_ready_even_when_gates_green \
  issue_4721_rail_tests::missing_record_is_unclear \
  issue_4721_record_verdict_tests::run_records_merge_and_rail_reads_it_back ; do
  grep -qF "$must_run" "$TEST_LOG" \
    || { echo "FAIL: expected test '$must_run' did not run (module renamed?)" >&2; cat "$TEST_LOG" >&2; exit 1; }
done
echo "OK: store + rail decision-matrix + CLI round-trip tests all ran and passed."

# --- (d) end-to-end at the binary boundary ------------------------------------
echo "== (d) e2e: 'simard merge record-verdict' writes a typed, tokened record =="
cargo build --locked --bin simard >"$WORK/build.log" 2>&1 \
  || { echo "FAIL: could not build the simard binary" >&2; cat "$WORK/build.log" >&2; exit 1; }
SIMARD="target/debug/simard"

STATE_ROOT="$WORK/state"
mkdir -p "$STATE_ROOT"
"$SIMARD" merge record-verdict \
  --pr 4721 --repo rysweet/Simard --verdict merge \
  --reason "crusty passed; CI green; diff reviewed" \
  --run-token "e2e-token-1" --state-root "$STATE_ROOT" \
  || { echo "FAIL: record-verdict merge exited non-zero" >&2; exit 1; }

RECORD="$STATE_ROOT/merge_verdicts/rysweet__Simard/4721.json"
[ -f "$RECORD" ] \
  || { echo "FAIL: record file not written at $RECORD" >&2; exit 1; }
if command -v jq >/dev/null 2>&1; then
  [ "$(jq -r '.verdict' "$RECORD")" = "merge" ] \
    || { echo "FAIL: recorded verdict is not 'merge'" >&2; cat "$RECORD" >&2; exit 1; }
  [ "$(jq -r '.run_token' "$RECORD")" = "e2e-token-1" ] \
    || { echo "FAIL: recorded run_token mismatch" >&2; cat "$RECORD" >&2; exit 1; }
  [ "$(jq -r '.pr' "$RECORD")" = "4721" ] \
    || { echo "FAIL: recorded pr mismatch" >&2; cat "$RECORD" >&2; exit 1; }
else
  grep -qF '"verdict"' "$RECORD" || { echo "FAIL: record missing verdict field" >&2; exit 1; }
fi
echo "OK: durable typed record written and readable."

# A bogus verdict must be rejected (exit 2) and write nothing new.
echo "== (d2) a bogus verdict is rejected with exit 2 =="
set +e
"$SIMARD" merge record-verdict \
  --pr 4722 --repo rysweet/Simard --verdict yes \
  --reason "x" --run-token "t" --state-root "$STATE_ROOT" 2>"$WORK/bogus.err"
RC=$?
set -e
[ "$RC" -eq 2 ] \
  || { echo "FAIL: bogus --verdict should exit 2, got $RC" >&2; cat "$WORK/bogus.err" >&2; exit 1; }
[ ! -f "$STATE_ROOT/merge_verdicts/rysweet__Simard/4722.json" ] \
  || { echo "FAIL: a rejected invocation must not write a record" >&2; exit 1; }
echo "OK: bogus verdict rejected with exit 2 and wrote nothing."

echo "PASS: merge-judge record-verdict rework scenario (#4721)"
