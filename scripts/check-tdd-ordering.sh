#!/usr/bin/env bash
# scripts/check-tdd-ordering.sh
#
# Enforces TDD authoring order for PRs touching pilot paths.
# See issue #1927 for the charter.
#
# Usage: check-tdd-ordering.sh <base-sha> <head-sha>
#
# Environment:
#   PR_BODY  — (optional) pull-request body text; checked for tdd-exempt trailer.
#
# Exits 0 on pass (correct ordering, exempt, or no pilot-path changes).
# Exits 1 on violation with a message naming the offending commit.

set -euo pipefail

BASE_SHA="${1:?Usage: check-tdd-ordering.sh <base-sha> <head-sha>}"
HEAD_SHA="${2:?Usage: check-tdd-ordering.sh <base-sha> <head-sha>}"

# Pilot paths per issue #1927 §2
PILOT_PATHS=(
  "src/meeting_backend/"
  "src/meeting_repl/"
  "src/meeting_facilitator/"
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# Returns 0 if any commit message or PR_BODY contains a tdd-exempt trailer.
check_tdd_exempt() {
  if [[ -n "${PR_BODY:-}" ]]; then
    if printf '%s' "$PR_BODY" | grep -qE 'tdd-exempt:\s*\S'; then
      echo "✓ tdd-exempt trailer found in PR body"
      return 0
    fi
  fi

  local sha
  while IFS= read -r sha; do
    if git log -1 --format='%B' "$sha" | grep -qE 'tdd-exempt:\s*\S'; then
      echo "✓ tdd-exempt trailer found in commit $(git log -1 --format='%h' "$sha")"
      return 0
    fi
  done < <(git log --reverse --format='%H' "${BASE_SHA}..${HEAD_SHA}")

  return 1
}

# Returns 0 if the commit modifies .rs files under any pilot path.
touches_pilot_production() {
  local sha="$1"
  local files
  files=$(git diff-tree --no-commit-id --name-only -r "$sha" 2>/dev/null || true)
  if [[ -z "$files" ]]; then
    return 1
  fi
  local p
  for p in "${PILOT_PATHS[@]}"; do
    if echo "$files" | grep -q "^${p}.*\.rs$"; then
      return 0
    fi
  done
  return 1
}

# Returns 0 if the commit adds #[test] or #[tokio::test] lines, or touches
# test files under tests/ related to the meeting subsystem.
is_test_commit() {
  local sha="$1"

  # Added lines containing test attributes anywhere in the diff.
  if git diff-tree -p "$sha" 2>/dev/null \
       | grep '^+' | grep -v '^+++' \
       | grep -qE '#\[(tokio::)?test\]'; then
    return 0
  fi

  # Touches integration-test files whose names reference the meeting subsystem.
  if git diff-tree --no-commit-id --name-only -r "$sha" 2>/dev/null \
       | grep -qiE '^tests/.*meeting'; then
    return 0
  fi

  return 1
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

main() {
  echo "=== TDD Ordering Check ==="
  echo "Base: ${BASE_SHA:0:12}"
  echo "Head: ${HEAD_SHA:0:12}"
  echo "Pilot paths: ${PILOT_PATHS[*]}"
  echo ""

  # Collect commits in chronological order.
  local commits=()
  while IFS= read -r sha; do
    commits+=("$sha")
  done < <(git log --reverse --format='%H' "${BASE_SHA}..${HEAD_SHA}")

  if [[ ${#commits[@]} -eq 0 ]]; then
    echo "✓ No commits in range — nothing to check."
    exit 0
  fi

  echo "Commits in range: ${#commits[@]}"

  # Fast path: no pilot-path changes at all.
  local has_pilot=false
  for sha in "${commits[@]}"; do
    if touches_pilot_production "$sha"; then
      has_pilot=true
      break
    fi
  done

  if [[ "$has_pilot" == "false" ]]; then
    echo "✓ No commits touch pilot paths — nothing to enforce."
    exit 0
  fi

  # Check for the escape-hatch trailer.
  if check_tdd_exempt; then
    exit 0
  fi

  # Walk commits chronologically and enforce ordering.
  local seen_test=false

  for sha in "${commits[@]}"; do
    local short
    short=$(git log -1 --format='%h %s' "$sha")

    # Test detection runs first so a mixed commit sets the flag before
    # the pilot-path check below reads it.
    if is_test_commit "$sha"; then
      seen_test=true
      echo "  ✓ $short — test commit"
    fi

    if touches_pilot_production "$sha"; then
      if [[ "$seen_test" == "false" ]]; then
        echo "  ✗ $short — pilot-path production change (no preceding test)"
        echo ""
        echo "VIOLATION: Commit $(git log -1 --format='%h' "$sha") modifies"
        echo "pilot-path production code but no test commit appears before it."
        echo ""
        echo "  Offending commit: $(git log -1 --format='%H %s' "$sha")"
        echo "  Pilot-path files changed:"
        local p
        for p in "${PILOT_PATHS[@]}"; do
          git diff-tree --no-commit-id --name-only -r "$sha" \
            | grep "^${p}" | sed 's/^/    /' || true
        done
        echo ""
        echo "To fix, either:"
        echo "  1. Reorder commits so a test commit appears before this one, or"
        echo "  2. Add a 'tdd-exempt:<reason>' trailer to the PR body or a commit"
        echo "     message (valid reasons: doc-only, one-line fix, generated code,"
        echo "     pure refactor)."
        exit 1
      else
        echo "  ✓ $short — pilot-path change (test already seen)"
      fi
    fi

    # Non-pilot, non-test commits are irrelevant.
    if ! is_test_commit "$sha" && ! touches_pilot_production "$sha"; then
      echo "  · $short — no pilot-path or test changes"
    fi
  done

  echo ""
  echo "✓ TDD ordering check passed."
  exit 0
}

main
