# Gym: ErrorHandling Debug Sub-family

The `ErrorHandling` benchmark class historically contained generic
error-handling scenarios. The **debug sub-family** added in
[issue #1461][issue] sharpens the class: it scores the agent's ability to
diagnose **real Simard runtime errors** — not synthetic ones — and propose
the documented remediation.

It complements the [`KnowledgeRecall`](gym-knowledge-recall.md) family added
in [PR #1467][knowledge-recall-pr]: where Knowledge Recall asks "do you
remember the canonical fact?", the ErrorHandling debug sub-family asks
"when this canonical fact is wrong in production, can you spot the symptom
and apply the fix?".

## Why a debug-specific sub-family

Generic ErrorHandling scenarios test whether an agent reasons about retries,
timeouts, and error propagation in the abstract. They do not stress-test
recall of project-specific failure modes. A daemon that maintains Simard
itself is graded on a much narrower distribution: it sees the same handful
of recurring failures every day. The debug sub-family encodes those failures
directly, so the gym scorecard regresses the moment the agent forgets one.

## Scenarios

Four scenarios are registered under `BenchmarkClass::ErrorHandling`:

1. **`error-handling-debug-stale-engineer-worktree`** — the worktree under
   `~/.simard/engineer-worktrees/` is alive but the subagent process has
   exited. Verifies the agent cites the OODA dispatch layer's
   `find_live_engineer_for_goal` liveness check and proposes a remediation.
2. **`error-handling-debug-pre-push-clippy-failure`** — a single
   `unused_imports` warning trips clippy under `-D warnings`. Verifies the
   agent names the lint, the tool, and explicitly forbids `--no-verify`
   bypass per user policy.
3. **`error-handling-debug-mkdocs-strict-broken-link`** — the `docs/build`
   CI job fails because mkdocs strict mode rejects a cross-document link.
   Verifies the agent names `mkdocs.yml`, the strict mode setting, the
   `docs/` tree boundary, and `prompt_assets/` as the unresolvable target.
4. **`error-handling-debug-recipe-runner-hollow-success`** —
   `step-08c-implementation-no-op-guard` reports "produced no output" even
   though the recipe completed structurally. Verifies the agent names the
   guard, identifies the missing-worktree symptom, and references the
   documented Opus 4.7 sub-agent fallback pattern.

Each scenario emits two checks: `error-handling-debug-evidence-grounded`
(runtime evidence references a stored memory record or repo file path) and
`error-handling-debug-canonical-token-cited` (the response actually names
the canonical diagnosis tokens the objective asked about).

## Why this PR matters

This family was **proposed by the prompt-driven OODA brain itself** in
cycle 2 — directly after the Knowledge Recall family landed in PR #1467.
The brain observed the new scenarios in its reflection input, noticed the
gap in error-handling debug coverage, opened issue #1461, and the
implementation followed. That closes the loop demonstrated by
[PR #1458][prompt-driven-pr]: brain proposes work → human-readable goal →
implementation → merge → brain observes the new scenarios → uses them to
grade itself. It is the first end-to-end proof point that the
prompt-driven brain pattern can extend its own evaluation surface without
human intervention.

[issue]: https://github.com/rysweet/Simard/issues/1461
[knowledge-recall-pr]: https://github.com/rysweet/Simard/pull/1467
[prompt-driven-pr]: https://github.com/rysweet/Simard/pull/1458
