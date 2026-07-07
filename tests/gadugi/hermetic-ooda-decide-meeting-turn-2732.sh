#!/usr/bin/env bash
# Outside-in scenario for issue #2732 — two pre-existing NON-HERMETIC tests are
# now deterministic regardless of host env, installed `copilot`, or auth state.
#
# What this proves, end-to-end, without an LLM:
#
#   (1) ooda_loop::decide::tests::decide_respects_max_concurrent_actions used
#       `OodaConfig { max_concurrent_actions: 2, ..Default::default() }`, and
#       `OodaConfig::default()` reads `SIMARD_SCALING` / `SIMARD_MAX_CONCURRENT_ACTIONS`
#       from the PROCESS ENV. On a host with `SIMARD_SCALING=auto` the default
#       AIMD scaler (seeded from `SIMARD_MAX_CONCURRENT_ACTIONS`, not from the
#       explicit `2`) overrode the cap under test, so the assertion depended on
#       the environment. We run that exact test under a HOSTILE env
#       (`SIMARD_SCALING=auto`, `SIMARD_MAX_CONCURRENT_ACTIONS=10`) and assert it
#       STILL passes — the fix pins `scaler: None` + an explicit cap.
#
#   (2) base_type_copilot `meeting_turn_*` tests guarded only on
#       `copilot_on_path()` (not auth), so on a host where the `copilot` binary
#       was installed but unauthenticated they spawned a REAL subprocess and
#       flaked. They now inject a FAKE `copilot` through a test seam
#       (`with_meeting_binary_override`). We run them with a DECOY `copilot`
#       first on PATH whose output would break the exact-match assertion if the
#       production path ever resolved `copilot` from PATH — passing proves the
#       injected fake (an absolute path) is used and no real/PATH copilot is
#       consulted.
#
#   (3) A source-level regression guard: the `copilot_on_path()` skip-guard is
#       gone and the hermetic seam + fake are present, so the anti-pattern
#       cannot silently return.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if ! command -v cargo >/dev/null 2>&1; then
  echo "SKIP: cargo not on PATH (required for this scenario)" >&2
  exit 0
fi

# Provide the prebuilt lbug static lib so `cargo test` links (mirrors CI's
# coverage/verify jobs). Best-effort: if it is already set or the helper is
# absent, leave the ambient environment untouched.
if [ -z "${LBUG_LIBRARY_DIR:-}" ] && [ -x scripts/provision-lbug-prebuilt.sh ]; then
  _lbug="$HOME/.cache/simard-lbug-precommit/lib"
  if scripts/provision-lbug-prebuilt.sh "$_lbug" >/dev/null 2>&1; then
    export LBUG_LIBRARY_DIR="$_lbug" LBUG_INCLUDE_DIR="$_lbug"
  fi
fi

WORK="$(mktemp -d /tmp/simard-hermetic-2732.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

# ---------------------------------------------------------------------------
# (3) source-level regression guard (fast; runs before the slower build)
# ---------------------------------------------------------------------------
echo "== (3) source guard: hermetic seam present, copilot_on_path guard gone =="
TESTS_SRC="src/base_type_copilot/tests.rs"
MOD_SRC="src/base_type_copilot/mod.rs"
DECIDE_SRC="src/ooda_loop/decide.rs"

grep -qF 'copilot_on_path' "$TESTS_SRC" \
  && { echo "FAIL: $TESTS_SRC still references the non-hermetic copilot_on_path guard" >&2; exit 1; }
grep -qF 'with_meeting_binary_override' "$MOD_SRC" \
  || { echo "FAIL: $MOD_SRC is missing the meeting-binary test seam" >&2; exit 1; }
grep -qF 'fn fake_copilot' "$TESTS_SRC" \
  || { echo "FAIL: $TESTS_SRC is missing the injected fake copilot helper" >&2; exit 1; }
grep -qF 'scaler: None' "$DECIDE_SRC" \
  || { echo "FAIL: $DECIDE_SRC decide test config no longer pins scaler: None" >&2; exit 1; }
echo "OK: seam + fake present; non-hermetic guard removed."

# ---------------------------------------------------------------------------
# (1) decide cap is hermetic under a HOSTILE env
# ---------------------------------------------------------------------------
echo "== (1) decide cap respects explicit max_concurrent_actions under SIMARD_SCALING=auto =="
D_LOG="$WORK/decide.log"
SIMARD_SCALING=auto SIMARD_MAX_CONCURRENT_ACTIONS=10 \
  cargo test --lib --locked \
  ooda_loop::decide::tests::decide_respects_max_concurrent_actions \
  -- --exact --nocapture >"$D_LOG" 2>&1 \
  || { echo "FAIL: decide cap test failed under hostile env (still non-hermetic)" >&2; cat "$D_LOG" >&2; exit 1; }
grep -qE 'test result: ok\. 1 passed' "$D_LOG" \
  || { echo "FAIL: expected exactly 1 decide test to pass" >&2; cat "$D_LOG" >&2; exit 1; }
echo "OK: decide cap is env-independent (scaler: None + explicit cap)."

# ---------------------------------------------------------------------------
# (2) meeting_turn tests are hermetic even with a DECOY copilot first on PATH
# ---------------------------------------------------------------------------
echo "== (2) meeting_turn tests ignore a decoy PATH copilot; use the injected fake =="
DECOY_DIR="$WORK/bin"
mkdir -p "$DECOY_DIR"
cat > "$DECOY_DIR/copilot" <<'DECOY'
#!/bin/sh
# Decoy copilot: if the production meeting path ever resolved `copilot` from
# PATH (instead of the test-injected absolute fake), the exact-match assertion
# in meeting_turn_captures_copilot_output_and_records_meeting_dispatch would
# receive THIS output and fail. Passing therefore proves PATH is not consulted.
cat >/dev/null
printf '%s' 'DECOY-COPILOT-SHOULD-NEVER-BE-USED'
DECOY
chmod +x "$DECOY_DIR/copilot"

M_LOG="$WORK/meeting.log"
PATH="$DECOY_DIR:$PATH" \
  cargo test --lib --locked base_type_copilot::tests::meeting_turn \
  -- --nocapture >"$M_LOG" 2>&1 \
  || { echo "FAIL: meeting_turn tests failed (non-hermetic, or the decoy leaked in)" >&2; cat "$M_LOG" >&2; exit 1; }
M_PASSED="$(grep -oE 'test result: ok\. [0-9]+ passed' "$M_LOG" | grep -oE '[0-9]+' | head -1)"
M_PASSED="${M_PASSED:-0}"
echo "meeting_turn tests passed: ${M_PASSED}"
[ "$M_PASSED" -ge 5 ] \
  || { echo "FAIL: expected >=5 meeting_turn tests to run (filter drift?)" >&2; cat "$M_LOG" >&2; exit 1; }
grep -qF 'meeting_turn_captures_copilot_output_and_records_meeting_dispatch' "$M_LOG" \
  || { echo "FAIL: the fake-output capture test did not run" >&2; cat "$M_LOG" >&2; exit 1; }
grep -qF 'DECOY-COPILOT-SHOULD-NEVER-BE-USED' "$M_LOG" \
  && { echo "FAIL: the decoy PATH copilot output leaked into a meeting turn" >&2; cat "$M_LOG" >&2; exit 1; }
echo "OK: meeting turns use the injected fake; the PATH decoy is never consulted."

echo "PASS: hermetic ooda_loop/decide + base_type_copilot meeting_turn scenario (issue #2732)"
