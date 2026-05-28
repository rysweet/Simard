#!/usr/bin/env bash
# check-tdd-ordering.sh — Verify TDD commit ordering for pilot-scope paths.
#
# For each pilot path touched by the PR, at least one test-adding commit must
# appear *before* any production-code commit in `git log --reverse`.  A commit
# carrying a `tdd-exempt:<reason>` trailer (with a non-empty reason) exempts
# the entire PR for that path.
#
# Usage:
#   scripts/check-tdd-ordering.sh <base-sha> <head-sha>
#
# See issue #1927 for the full charter.

set -euo pipefail

# ── Pilot paths (§2 of the charter) ─────────────────────────────────────────
PILOT_PATHS=(
  "src/meeting_backend/"
  "src/meeting_repl/"
  "src/meeting_facilitator/"
)

# ── Helpers ──────────────────────────────────────────────────────────────────

die()  { echo "::error::$*" >&2; exit 1; }
warn() { echo "::warning::$*" >&2; }
info() { echo "$*"; }

usage() {
  echo "Usage: $0 <base-sha> <head-sha>" >&2
  exit 1
}

# Return 0 if a file path is inside one of the pilot directories.
in_pilot_scope() {
  local file="$1"
  for p in "${PILOT_PATHS[@]}"; do
    if [[ "$file" == "$p"* ]]; then
      return 0
    fi
  done
  return 1
}

# Return 0 if a file path looks like a test file.
# Matches: files under tests/, files ending in _test.rs or _tests.rs,
# files in a test_support/ directory, or files whose path contains /tests/.
is_test_file() {
  local file="$1"
  # Integration tests live under tests/ at the repo root.
  [[ "$file" == tests/* ]] && return 0
  # Unit-test modules conventionally end with _test.rs or _tests.rs, or live
  # in a test_support/ subtree.
  [[ "$file" == *_test.rs ]] && return 0
  [[ "$file" == *_tests.rs ]] && return 0
  [[ "$file" == */test_support/* ]] && return 0
  [[ "$file" == */tests/* ]] && return 0
  # Inline #[cfg(test)] modules live inside production files — but the commit
  # diff is what matters, not the filename. We check for test additions at
  # the diff level separately.
  return 1
}

# Return 0 if a commit adds or modifies test code (heuristic: the diff adds
# lines containing #[test], #[tokio::test], or common test macros).
commit_adds_tests() {
  local sha="$1"
  shift
  local paths=("$@")
  # Look at added lines in the diff for this commit, scoped to the given paths.
  # We also include tests/ at the repo root since integration tests for pilot
  # modules may live there.
  local diff_output
  diff_output=$(git diff-tree -p "$sha" -- "${paths[@]}" tests/ 2>/dev/null || true)
  if echo "$diff_output" | grep -qE '^\+.*#\[(test|tokio::test)\]'; then
    return 0
  fi
  if echo "$diff_output" | grep -qE '^\+.*(assert_eq!|assert!|assert_ne!|#\[cfg\(test\)\])'; then
    return 0
  fi
  return 1
}

# Return 0 if a commit touches production (non-test) files in any pilot path.
commit_touches_prod() {
  local sha="$1"
  shift
  local paths=("$@")
  local files
  files=$(git diff-tree --no-commit-id -r --name-only "$sha" -- "${paths[@]}")
  while IFS= read -r f; do
    [ -z "$f" ] && continue
    if ! is_test_file "$f"; then
      return 0
    fi
  done <<< "$files"
  return 1
}

# ── Main ─────────────────────────────────────────────────────────────────────

[[ $# -eq 2 ]] || usage

BASE_SHA="$1"
HEAD_SHA="$2"

info "TDD ordering check (issue #1927)"
info "  base: $BASE_SHA"
info "  head: $HEAD_SHA"
info "  pilot paths: ${PILOT_PATHS[*]}"
info ""

# Collect commits in chronological (oldest-first) order.
COMMITS=$(git log --reverse --format='%H' "$BASE_SHA".."$HEAD_SHA" --)

if [ -z "$COMMITS" ]; then
  info "No commits in range — nothing to check."
  exit 0
fi

# ── Check for tdd-exempt trailer ─────────────────────────────────────────────
# If any commit in the range carries a valid tdd-exempt trailer, the PR is
# exempt and we exit early.
for sha in $COMMITS; do
  trailer=$(git log -1 --format='%(trailers:key=tdd-exempt,valueonly)' "$sha" 2>/dev/null || true)
  trailer=$(echo "$trailer" | xargs)  # trim whitespace
  if [ -n "$trailer" ]; then
    info "✓ tdd-exempt trailer found on $sha: $trailer"
    info "  Skipping TDD ordering check."
    exit 0
  fi
done

# ── Determine which pilot paths are touched ──────────────────────────────────
ALL_FILES=$(git diff --name-only "$BASE_SHA".."$HEAD_SHA" --)
declare -A TOUCHED_PILOTS

for f in $ALL_FILES; do
  for p in "${PILOT_PATHS[@]}"; do
    if [[ "$f" == "$p"* ]]; then
      TOUCHED_PILOTS["$p"]=1
    fi
  done
done

if [ ${#TOUCHED_PILOTS[@]} -eq 0 ]; then
  info "No pilot-scope paths touched — nothing to check."
  exit 0
fi

info "Pilot paths touched: ${!TOUCHED_PILOTS[*]}"
info ""

# ── For each touched pilot path, verify ordering ─────────────────────────────
FAILED=0

for pilot in "${!TOUCHED_PILOTS[@]}"; do
  info "Checking $pilot ..."
  test_seen=false
  prod_before_test=false
  first_prod_sha=""

  for sha in $COMMITS; do
    # Does this commit touch files in this pilot path?
    files_in_pilot=$(git diff-tree --no-commit-id -r --name-only "$sha" -- "$pilot" 2>/dev/null || true)
    [ -z "$files_in_pilot" ] && continue

    # Check if this commit adds test code for this pilot path.
    if commit_adds_tests "$sha" "$pilot"; then
      test_seen=true
    fi

    # Check if this commit touches production code in this pilot path.
    if commit_touches_prod "$sha" "$pilot"; then
      if ! $test_seen; then
        prod_before_test=true
        first_prod_sha="$sha"
      fi
    fi
  done

  if $prod_before_test; then
    short=$(git log -1 --format='%h %s' "$first_prod_sha")
    echo "::error::TDD FAIL for $pilot — production code commit appears before any test commit."
    echo "::error::  First offending commit: $short"
    echo "::error::  Add a test-first commit or use a tdd-exempt:<reason> trailer."
    echo "::error::  See: https://github.com/rysweet/Simard/issues/1927"
    FAILED=1
  else
    info "  ✓ $pilot — test-first ordering satisfied (or only test changes)."
  fi
done

echo ""
if [ "$FAILED" -ne 0 ]; then
  die "TDD ordering check failed. See errors above."
else
  info "✓ All pilot paths pass TDD ordering check."
fi
