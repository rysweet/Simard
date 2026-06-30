#!/usr/bin/env bash
# Outside-in coverage for issue #2511: the goal-board cross-process write lock.
#
# Each `simard goal …` invocation below is a SEPARATE OS process, mirroring the
# real `simard goal add/remove` CLI that the OODA daemon races against. The fix
# (an advisory flock over <state_root>/state/goal-board.lock held across the
# read-merge-write window) is compiled into the non-test `simard` binary, so
# this scenario exercises the actual production lock path — not the cfg(test)
# unit-test stub.
#
# Asserted behaviour:
#   1. Sequential cross-process `goal add`s accumulate (no silent clobber).
#   2. `goal remove <id>` drops only the named id; unrelated goals survive
#      (the collateral-drop symptom called out in #2511).
#   3. The advisory lock file is materialised under <state_root>/state/,
#      proving the lock path actually executed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

STATE_ROOT="$(mktemp -d /tmp/simard-goal-board-lock.XXXXXX)"
trap 'rm -rf "$STATE_ROOT"' EXIT
export SIMARD_STATE_ROOT="$STATE_ROOT"

# Warm the build once so each goal invocation is a fast, independent process.
cargo build --quiet --bin simard

goal() { cargo run --quiet --bin simard -- goal "$@"; }

# --- 1. Disjoint cross-process adds must both survive ----------------------
goal add 1 "Harden alpha repo"
goal add 2 "Harden beta repo"

LIST1="$(goal list)"
printf '%s\n' "$LIST1"
printf '%s\n' "$LIST1" | grep -F "active goals: 2 / 7" >/dev/null
printf '%s\n' "$LIST1" | grep -F "harden-alpha-repo" >/dev/null
printf '%s\n' "$LIST1" | grep -F "harden-beta-repo" >/dev/null

# --- 2. Add a third, remove the first; the other two must remain -----------
goal add 3 "Harden gamma repo"
goal remove harden-alpha-repo

LIST2="$(goal list)"
printf '%s\n' "$LIST2"
printf '%s\n' "$LIST2" | grep -F "active goals: 2 / 7" >/dev/null
printf '%s\n' "$LIST2" | grep -F "harden-beta-repo" >/dev/null
printf '%s\n' "$LIST2" | grep -F "harden-gamma-repo" >/dev/null
if printf '%s\n' "$LIST2" | grep -F "harden-alpha-repo" >/dev/null; then
  echo "removed goal 'harden-alpha-repo' must not survive (#2511)" >&2
  exit 1
fi

# --- 3. The cross-process advisory lock file must have been created --------
if [ ! -f "$STATE_ROOT/state/goal-board.lock" ]; then
  echo "expected advisory lock at $STATE_ROOT/state/goal-board.lock (#2511)" >&2
  exit 1
fi

echo "goal-board write-lock scenario: PASS"
