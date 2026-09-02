#!/usr/bin/env bash
# qa-cost-meeting-prompt-tokens.sh
#
# Regression gate for issue #4164: the dashboard Resources → Costs tab
# (`GET /api/costs`) must NOT undercount meeting-mode prompt tokens.
#
# Root cause (fixed): `run_meeting_turn` in `src/base_type_copilot/mod.rs`
# streams the fully-rendered enriched prompt (`formatted` — preamble + identity
# context + objective wrapped in the `## Objective` / `## Instructions`
# scaffold) to copilot on stdin, but recorded the cost with
# `prompt_chars = input.objective.len()` — the BARE objective, excluding the
# entire preamble/scaffold actually sent. This produced an impossible
# `total_prompt_tokens ≪ total_completion_tokens` ratio on the Cost tab and
# understated spend. The PTY path already records the enriched objective's
# length; meeting mode was the buggy outlier. The fix records `formatted.len()`.
#
# This script is a hermetic, network-free pass/fail gate. It:
#   1. Runs the deterministic hermetic regression that runs the REAL
#      `run_meeting_turn` (fake copilot on stdin, isolated per-test HOME cost
#      ledger) and asserts the recorded prompt tokens exceed the bare
#      objective's token count — which only holds when the full streamed prompt
#      is recorded.
#   2. Structurally guards the production recording site so a future edit cannot
#      quietly revert meeting-mode prompt accounting back to the bare objective
#      without also deleting these lines.
#
# A non-zero exit on any failed assertion is treated by the gadugi `cli` agent
# runner as a step failure, so this is a real gate (not a cosmetic assertion).
set -uo pipefail

fail() {
  echo "QA-COST-MEETING-TOKENS: FAIL - $1"
  exit 1
}

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || fail "cannot cd to repo root"

# 1. Deterministic hermetic regression (no network, no daemon, no real copilot).
#    Fails loudly if meeting-mode cost recording reverts to the bare objective.
TEST="base_type_copilot::tests::meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective"
echo "QA-COST-MEETING-TOKENS: running hermetic regression ($TEST)…"
if ! cargo test --quiet --lib -- --exact "$TEST"; then
  fail "meeting-mode prompt-token cost regression test failed"
fi

# 2. Structural source-contract guards on the production recording site. The
#    meeting turn must record the FULL streamed prompt (`formatted.len()`) and
#    must NOT record the bare objective as the prompt size. `formatted` is the
#    exact byte buffer handed to `attach_prompt_std(... formatted.as_bytes())`.
MOD="src/base_type_copilot/mod.rs"
grep -q "let prompt_chars = formatted.len();" "$MOD" \
  || fail "run_meeting_turn no longer records formatted.len() as prompt_chars (#4164 regressed)"
grep -q "attach_prompt_std(&mut command, formatted.as_bytes())" "$MOD" \
  || fail "meeting prompt is no longer streamed from 'formatted' — the recorded size may not match what is sent"
if grep -q "let prompt_chars = input.objective.len();" "$MOD"; then
  fail "run_meeting_turn records the BARE objective as prompt_chars again — meeting cost undercount reintroduced (#4164)"
fi

echo "QA-COST-MEETING-TOKENS: PASS - meeting prompt cost reflects the full streamed prompt, not the bare objective (#4164)"
