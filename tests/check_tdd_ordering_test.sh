#!/usr/bin/env bash
# tests/check_tdd_ordering_test.sh
#
# Test harness for scripts/check-tdd-ordering.sh.
# Creates temporary git repos with various commit orderings and verifies the
# script produces the correct exit code for each scenario.
#
# Usage: bash tests/check_tdd_ordering_test.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHECK_SCRIPT="$SCRIPT_DIR/scripts/check-tdd-ordering.sh"

PASS=0
FAIL=0
ERRORS=""

# ── helpers ──────────────────────────────────────────────────────────────────

setup_repo() {
  local tmpdir
  tmpdir=$(mktemp -d)
  cd "$tmpdir"
  git init --initial-branch=main -q
  git config user.email "test@test.com"
  git config user.name "Test"
  git config core.hooksPath /dev/null
  mkdir -p src/meeting_backend src/meeting_repl src/meeting_facilitator tests
  echo "// initial" > src/meeting_backend/mod.rs
  echo "// initial" > src/meeting_repl/mod.rs
  echo "// initial" > src/meeting_facilitator/mod.rs
  git add -A && git commit -q -m "Initial commit"
  echo "$tmpdir"
}

cleanup_repo() { rm -rf "$1"; }

run_test() {
  local name="$1" expected_exit="$2"
  local base_sha head_sha actual_exit output

  base_sha=$(git log --reverse --format='%H' | head -1)
  head_sha=$(git log --format='%H' -1)

  set +e
  output=$("$CHECK_SCRIPT" "$base_sha" "$head_sha" 2>&1)
  actual_exit=$?
  set -e

  if [[ "$actual_exit" -eq "$expected_exit" ]]; then
    echo "  PASS: $name (exit=$actual_exit)"
    ((PASS++)) || true
  else
    echo "  FAIL: $name (expected exit=$expected_exit, got exit=$actual_exit)"
    echo "  --- output ---"
    echo "$output" | sed 's/^/  | /'
    echo "  ---"
    ((FAIL++)) || true
    ERRORS="${ERRORS}\n  FAIL: $name"
  fi
}

# ── test cases ───────────────────────────────────────────────────────────────

echo "=== TDD Ordering Check — Test Harness ==="
echo ""

# 1. Correct order: test commit before production commit → PASS
echo "Test 1: test commit before production commit"
tmpdir=$(setup_repo)
cd "$tmpdir"
cat >> src/meeting_backend/mod.rs <<'RUST'

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() { assert!(true); }
}
RUST
git add -A && git commit -q -m "Add test for meeting_backend"
cat >> src/meeting_backend/mod.rs <<'RUST'

pub fn handle_meeting() -> bool { true }
RUST
git add -A && git commit -q -m "Add handle_meeting function"
run_test "test-before-production" 0
cleanup_repo "$tmpdir"

# 2. Wrong order: production before test → FAIL
echo "Test 2: production commit before test commit"
tmpdir=$(setup_repo)
cd "$tmpdir"
cat >> src/meeting_backend/mod.rs <<'RUST'

pub fn handle_meeting() -> bool { true }
RUST
git add -A && git commit -q -m "Add handle_meeting function"
cat >> src/meeting_backend/mod.rs <<'RUST'

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() { assert!(true); }
}
RUST
git add -A && git commit -q -m "Add test for meeting_backend"
run_test "production-before-test" 1
cleanup_repo "$tmpdir"

# 3. tdd-exempt trailer in commit message → PASS
echo "Test 3: tdd-exempt trailer in commit message"
tmpdir=$(setup_repo)
cd "$tmpdir"
cat >> src/meeting_repl/mod.rs <<'RUST'

pub fn start_repl() {}
RUST
git add -A && git commit -q -m "Fix typo in meeting_repl

tdd-exempt: one-line fix"
run_test "tdd-exempt-commit" 0
cleanup_repo "$tmpdir"

# 4. No pilot-path changes → PASS
echo "Test 4: no pilot-path changes"
tmpdir=$(setup_repo)
cd "$tmpdir"
mkdir -p src
echo "pub fn unrelated() {}" > src/lib.rs
git add -A && git commit -q -m "Unrelated change"
run_test "no-pilot-changes" 0
cleanup_repo "$tmpdir"

# 5. tdd-exempt in PR body (env var) → PASS
echo "Test 5: tdd-exempt trailer in PR body"
tmpdir=$(setup_repo)
cd "$tmpdir"
cat >> src/meeting_facilitator/mod.rs <<'RUST'

pub fn facilitate() {}
RUST
git add -A && git commit -q -m "Add facilitate function"
PR_BODY=$'This is a simple fix.\n\ntdd-exempt: pure refactor' \
  run_test "tdd-exempt-pr-body" 0
cleanup_repo "$tmpdir"

# 6. Mixed commit (test + production in same commit) → PASS (lenient)
echo "Test 6: test and production in same commit (lenient — passes)"
tmpdir=$(setup_repo)
cd "$tmpdir"
cat >> src/meeting_backend/mod.rs <<'RUST'

pub fn process() -> u32 { 42 }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_process() { assert_eq!(process(), 42); }
}
RUST
git add -A && git commit -q -m "Add process with inline test"
run_test "mixed-commit" 0
cleanup_repo "$tmpdir"

# 7. Production-only commit, no tests anywhere → FAIL
echo "Test 7: production-only commit, no tests"
tmpdir=$(setup_repo)
cd "$tmpdir"
cat >> src/meeting_backend/mod.rs <<'RUST'

pub fn new_feature() -> bool { true }
RUST
git add -A && git commit -q -m "Add new feature without tests"
run_test "production-only" 1
cleanup_repo "$tmpdir"

# 8. Integration test in tests/ before production → PASS
echo "Test 8: integration test in tests/ before production"
tmpdir=$(setup_repo)
cd "$tmpdir"
cat > tests/meeting_close_test.rs <<'RUST'
#[test]
fn meeting_close_works() { assert!(true); }
RUST
git add -A && git commit -q -m "Add meeting close integration test"
cat >> src/meeting_backend/mod.rs <<'RUST'

pub fn close_meeting() -> bool { true }
RUST
git add -A && git commit -q -m "Implement close_meeting"
run_test "integration-test-before-prod" 0
cleanup_repo "$tmpdir"

# 9. #[tokio::test] attribute recognised → PASS
echo "Test 9: #[tokio::test] recognised as test commit"
tmpdir=$(setup_repo)
cd "$tmpdir"
cat >> src/meeting_repl/mod.rs <<'RUST'

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn async_test() { assert!(true); }
}
RUST
git add -A && git commit -q -m "Add async test"
cat >> src/meeting_repl/mod.rs <<'RUST'

pub async fn run_repl() {}
RUST
git add -A && git commit -q -m "Add run_repl"
run_test "tokio-test-recognised" 0
cleanup_repo "$tmpdir"

# 10. Empty commit range → PASS
echo "Test 10: empty commit range"
tmpdir=$(setup_repo)
cd "$tmpdir"
sha=$(git log --format='%H' -1)
set +e
output=$("$CHECK_SCRIPT" "$sha" "$sha" 2>&1)
rc=$?
set -e
if [[ "$rc" -eq 0 ]]; then
  echo "  PASS: empty-range (exit=0)"
  ((PASS++)) || true
else
  echo "  FAIL: empty-range (expected 0, got $rc)"
  ((FAIL++)) || true
  ERRORS="${ERRORS}\n  FAIL: empty-range"
fi
cleanup_repo "$tmpdir"

# 11. Multiple pilot paths — test for one, production for another → PASS
echo "Test 11: test in meeting_backend, production in meeting_repl (cross-module)"
tmpdir=$(setup_repo)
cd "$tmpdir"
cat >> src/meeting_backend/mod.rs <<'RUST'

#[cfg(test)]
mod tests {
    #[test]
    fn cross_module_test() { assert!(true); }
}
RUST
git add -A && git commit -q -m "Add cross-module test"
cat >> src/meeting_repl/mod.rs <<'RUST'

pub fn repl_feature() {}
RUST
git add -A && git commit -q -m "Add repl feature"
run_test "cross-module-test-before-prod" 0
cleanup_repo "$tmpdir"

# 12. Non-.rs file under pilot path → not flagged
echo "Test 12: non-.rs file under pilot path (not enforced)"
tmpdir=$(setup_repo)
cd "$tmpdir"
echo "# README" > src/meeting_backend/README.md
git add -A && git commit -q -m "Add README to meeting_backend"
run_test "non-rs-ignored" 0
cleanup_repo "$tmpdir"

# ── summary ──────────────────────────────────────────────────────────────────

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="
if [[ $FAIL -gt 0 ]]; then
  echo -e "Failures:$ERRORS"
  exit 1
fi
exit 0
