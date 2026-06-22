#!/usr/bin/env bash
# Outside-in QA for issue #2359 BUG 1: goal -> target-repo routing, surfaced
# through the `simard goal add [--repo <slug>]` operator CLI.
#
# This is the user-facing entry point for the fix: an operator (or the meeting /
# dashboard ingress that reuses the same validator) tags a goal with the repo
# its engineer should work in. The scenario asserts the contract end to end
# against the real `simard` binary and a throwaway state root:
#
#   1. `goal --help` documents the `--repo <slug>` routing flag.
#   2. A valid ecosystem slug is accepted and the add confirms the route
#      (`-> repo '<slug>'`).
#   3. A repo-less add defaults to the daemon's own repo
#      (`-> repo Simard (daemon)`) -- backward compatible.
#   4. Traversal / argv-injection / out-of-charset slugs are REJECTED at
#      ingress (non-zero exit) so an engineer can never be routed outside the
#      `~/src/` ecosystem root.
#   5. Only the two accepted goals land on the board; no rejected slug leaks.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

export SIMARD_NO_UPDATE_CHECK=1
export RUST_LOG=error

STATE_ROOT="$(mktemp -d /tmp/simard-goal-repo-routing.XXXXXX)"
trap 'rm -rf "$STATE_ROOT"' EXIT
export SIMARD_STATE_ROOT="$STATE_ROOT"

# Resolve the `simard` binary. Honour an explicit override; otherwise prefer an
# already-built binary (respecting CARGO_TARGET_DIR), falling back to a build.
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
if [[ -n "${SIMARD_BIN:-}" ]]; then
  BIN="$SIMARD_BIN"
elif [[ -x "$TARGET_DIR/release/simard" ]]; then
  BIN="$TARGET_DIR/release/simard"
elif [[ -x "$TARGET_DIR/debug/simard" ]]; then
  BIN="$TARGET_DIR/debug/simard"
else
  cargo build --quiet --bin simard
  BIN="$TARGET_DIR/debug/simard"
fi
echo "Using simard binary: $BIN"

OUT=""
STATUS=0
run_goal() {
  # Capture combined output + exit status without tripping `set -e`.
  set +e
  OUT="$("$BIN" goal "$@" 2>&1)"
  STATUS=$?
  set -e
  printf '%s\n' "$OUT"
}

# 1. Help documents the routing flag.
run_goal --help
printf '%s\n' "$OUT" | grep -F -- "--repo <slug>" >/dev/null
printf '%s\n' "$OUT" | grep -F -- "routes the goal's engineer" >/dev/null

# 2. A valid ecosystem slug is accepted and the route is confirmed.
run_goal add 2 --repo amplihack-rs "route coverage work to amplihack-rs"
test "$STATUS" -eq 0
printf '%s\n' "$OUT" | grep -F -- "-> repo 'amplihack-rs'" >/dev/null

# 3. A repo-less add defaults to the daemon's own repo (backward compatible).
run_goal add 3 "keep simard healthy"
test "$STATUS" -eq 0
printf '%s\n' "$OUT" | grep -F -- "-> repo Simard (daemon)" >/dev/null

# 4. Dangerous slugs are rejected at ingress (non-zero exit, nothing persisted).
for bad in "../Simard" ".." "-rm" "a/b" "repo;rm" ".git"; do
  run_goal add 2 --repo "$bad" "should be rejected"
  if [[ "$STATUS" -eq 0 ]]; then
    echo "SECURITY: slug '$bad' was accepted but MUST be rejected" >&2
    exit 1
  fi
done

# 5. Exactly the two accepted goals are on the board; no rejected slug leaked.
run_goal list
printf '%s\n' "$OUT" | grep -F -- "route coverage work to amplihack-rs" >/dev/null
printf '%s\n' "$OUT" | grep -F -- "keep simard healthy" >/dev/null
if printf '%s\n' "$OUT" | grep -F -- "should be rejected" >/dev/null; then
  echo "a rejected-slug goal leaked onto the board" >&2
  exit 1
fi

echo "PASS: goal -> target-repo routing CLI behaves correctly"
