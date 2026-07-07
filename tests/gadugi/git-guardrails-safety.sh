#!/usr/bin/env bash
# QA driver for the git guardrails safety contract.
#
# `check_git_safety` is the autonomous-mode guardrail that blocks destructive
# git operations (force push, reset --hard, branch -D main/release/master,
# clean -fdx, reflog expire, aggressive gc) globally, and restricts every
# non-safe-listed command inside a configured *protected* repository root.
#
# This scenario exercises the hermetic unit suite that proves the guardrail
# behavior end-to-end: the globally-destructive patterns are rejected, the
# protected-repo safe-list is enforced (and skipped for unprotected repos),
# the destructive-pattern check takes precedence over the safe-list, empty
# invocations are handled, and the SIMARD_GIT_GUARDRAILS disable toggle works.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

TEST_OUTPUT="$(
  cargo test --lib git_guardrails -- --test-threads=1 2>&1
)"

printf '%s\n' "$TEST_OUTPUT"

# --- Global destructive-pattern blocks -------------------------------------
printf '%s\n' "$TEST_OUTPUT" | grep -F "blocks_force_push ... ok" >/dev/null
printf '%s\n' "$TEST_OUTPUT" | grep -F "blocks_force_push_short_flag ... ok" >/dev/null
printf '%s\n' "$TEST_OUTPUT" | grep -F "blocks_reset_hard ... ok" >/dev/null
printf '%s\n' "$TEST_OUTPUT" | grep -F "blocks_delete_main_branch ... ok" >/dev/null
printf '%s\n' "$TEST_OUTPUT" | grep -F "blocks_clean_fdx ... ok" >/dev/null
printf '%s\n' "$TEST_OUTPUT" | grep -F "blocks_gc_prune_aggressive ... ok" >/dev/null

# --- Protected-repo safe-list enforcement ----------------------------------
printf '%s\n' "$TEST_OUTPUT" | grep -F "protected_repo_blocks_command_outside_safe_list ... ok" >/dev/null
printf '%s\n' "$TEST_OUTPUT" | grep -F "protected_repo_allows_each_safe_command ... ok" >/dev/null
printf '%s\n' "$TEST_OUTPUT" | grep -F "unprotected_repo_allows_command_outside_safe_list ... ok" >/dev/null
printf '%s\n' "$TEST_OUTPUT" | grep -F "protected_repos_ignores_empty_colon_entries ... ok" >/dev/null

# --- Precedence, empty args, and disable toggle ----------------------------
printf '%s\n' "$TEST_OUTPUT" | grep -F "global_destructive_pattern_beats_protected_safe_list ... ok" >/dev/null
printf '%s\n' "$TEST_OUTPUT" | grep -F "empty_args_in_protected_repo_are_blocked ... ok" >/dev/null
printf '%s\n' "$TEST_OUTPUT" | grep -F "flag_unrecognized_value_keeps_guardrails_enabled ... ok" >/dev/null

# Whole filtered suite passed with zero failures.
printf '%s\n' "$TEST_OUTPUT" | grep -E "test result: ok\. [0-9]+ passed; 0 failed" >/dev/null

echo "git-guardrails-safety: all guardrail safety scenarios passed"
