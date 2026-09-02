#!/usr/bin/env bash
# QA driver for issue #2484 — shared robust JSON/verdict extractor.
#
# Exercises the hermetic unit suite that proves a raw, ANSI/log-noised
# recipe-runner-rs span fails to parse while the shared `recipe_output`
# extractor strips the noise and recovers the payload, across both the
# shared module and the adopted distill path.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# The `recipe_output` filter also matches the distill `parse_recipe_output_*`
# tests (substring), so this one invocation covers the shared module and its
# adoption in the distill path.
TEST_OUTPUT="$(
  cargo test --lib recipe_output -- --test-threads=1 2>&1
)"

printf '%s\n' "$TEST_OUTPUT"

# Shared-module recovery proofs.
printf '%s\n' "$TEST_OUTPUT" | grep -F "strip_recipe_noise_strips_ansi_and_logs_together ... ok" >/dev/null
printf '%s\n' "$TEST_OUTPUT" | grep -F "strip_recipe_noise_drops_tracing_log_lines ... ok" >/dev/null
printf '%s\n' "$TEST_OUTPUT" | grep -F "strip_recipe_noise_drops_runner_banner_lines ... ok" >/dev/null
printf '%s\n' "$TEST_OUTPUT" | grep -F "extract_verdict_ignores_keyword_substring_inside_dropped_log_line ... ok" >/dev/null

# Distill-path adoption recovery proofs.
printf '%s\n' "$TEST_OUTPUT" | grep -F "parse_recipe_output_recovers_from_ansi_log_noise ... ok" >/dev/null
printf '%s\n' "$TEST_OUTPUT" | grep -F "parse_recipe_output_recovers_from_runner_banner ... ok" >/dev/null

# Whole filtered suite passed with zero failures.
printf '%s\n' "$TEST_OUTPUT" | grep -E "test result: ok\. [0-9]+ passed; 0 failed" >/dev/null

echo "recipe-output-extractor: all #2484 extractor recovery scenarios passed"
