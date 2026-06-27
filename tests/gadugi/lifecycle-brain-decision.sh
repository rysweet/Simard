#!/usr/bin/env bash
# Outside-in scenario for issue #2419 — the engineer-lifecycle brain
# decision-keyword parse failure.
#
# Root cause: RecipeBrain::decide_engineer_lifecycle invoked recipe-runner-rs
# in its DEFAULT `text` output mode, which prints only a summary banner
# ("Recipe: <name> ... SUCCESS ...") to stdout. The agent's actual decision
# text is NOT on stdout in text mode — it is only exposed via
# `--output-format json` (step_results[].output). First-word extraction over
# the banner therefore always saw "Recipe:", matched no lifecycle variant, and
# silently defaulted to continue_skipping on ~99.6% of invocations.
#
# This scenario validates the contract at the recipe-runner boundary WITHOUT
# an LLM: a deterministic bash-step recipe emits a known decision keyword, and
# we assert that
#   (a) text mode hides it behind the banner (the bug), and
#   (b) json mode exposes it as the final step output (the fix), whose first
#       word IS a real lifecycle variant.
#
# It also exercises the in-tree Rust parser/metric path via `cargo test` so the
# four metric outcomes (parsed | default_empty | default_malformed | error)
# and the happy-path keyword extraction are validated end-to-end.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if ! command -v recipe-runner-rs >/dev/null 2>&1; then
  echo "SKIP: recipe-runner-rs not on PATH (required for this scenario)" >&2
  exit 0
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "SKIP: jq not on PATH (required for this scenario)" >&2
  exit 0
fi

WORK="$(mktemp -d /tmp/simard-lifecycle-brain-2419.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

RECIPE="$WORK/lifecycle-fixture.yaml"
DECISION="reclaim_and_redispatch worktree idle 7h, log truncated mid-tool-call"

cat > "$RECIPE" <<EOF
name: "lifecycle-fixture-2419"
description: "deterministic lifecycle decision fixture (no LLM)"
version: "1.0.0"
context: {}
steps:
  - id: "engineer-lifecycle-decision"
    type: "bash"
    command: |
      echo '${DECISION}'
    output: "lifecycle_result"
EOF

# --- (a) text mode reproduces the bug: first word is the banner, not a variant
TEXT_OUT="$(recipe-runner-rs "$RECIPE" 2>/dev/null)"
printf '%s\n' "$TEXT_OUT"
TEXT_FIRST_WORD="$(printf '%s\n' "$TEXT_OUT" | awk 'NF{print $1; exit}')"
echo "text-mode first word: ${TEXT_FIRST_WORD}"
if [ "$TEXT_FIRST_WORD" = "reclaim_and_redispatch" ]; then
  echo "FAIL: text mode unexpectedly exposed the decision as the first word" >&2
  exit 1
fi
# The banner's first token is "Recipe:" — proving raw stdout is unparseable.
printf '%s\n' "$TEXT_OUT" | grep -qE '^Recipe:' \
  || { echo "FAIL: expected a 'Recipe:' summary banner in text mode" >&2; exit 1; }

# --- (b) json mode exposes the real decision as the final step output (the fix)
JSON_OUT="$(recipe-runner-rs "$RECIPE" --output-format json 2>/dev/null)"
printf '%s' "$JSON_OUT" | jq -e '.success == true' >/dev/null \
  || { echo "FAIL: recipe did not report success in json mode" >&2; exit 1; }

STEP_OUTPUT="$(printf '%s' "$JSON_OUT" | jq -r '.step_results | last | .output')"
echo "json-mode final step output: ${STEP_OUTPUT}"
JSON_FIRST_WORD="$(printf '%s\n' "$STEP_OUTPUT" | awk 'NF{print $1; exit}')"
echo "json-mode first word: ${JSON_FIRST_WORD}"
[ "$JSON_FIRST_WORD" = "reclaim_and_redispatch" ] \
  || { echo "FAIL: json-mode first word is not the expected lifecycle variant" >&2; exit 1; }

# Also assert the captured context var equals the step output (recipe contract).
CTX_VAL="$(printf '%s' "$JSON_OUT" | jq -r '.context.lifecycle_result')"
[ "$CTX_VAL" = "$DECISION" ] \
  || { echo "FAIL: context.lifecycle_result did not match emitted decision" >&2; exit 1; }

echo "OK: text mode hides the decision behind the banner; json mode exposes it."

# --- (c) in-tree Rust parser + metric outcome coverage (the four branches)
echo "Running in-tree parser/metric unit tests (issue_2419_tests)..."
cargo test --lib --locked issue_2419_tests -- --nocapture >/dev/null
echo "OK: parser outcome classification + metric context tests pass."

echo "PASS: lifecycle-brain-decision scenario (issue #2419)"
