# Gym: Knowledge Recall Family

The `KnowledgeRecall` benchmark class measures **longitudinal learning** —
whether Simard remembers what she should already know by now. Every other
benchmark class scores a single-shot task. Knowledge Recall scores
*accumulation*.

It was introduced in [issue #1459][issue] as the first scenario-level addition
on top of the prompt-driven OODA brain (PR #1458): now that the brain reads
stored memories as context, memory accumulation is the gating capability for
nearly every other quality the gym tries to measure. Without recall scoring
we cannot detect when memory consolidation regresses.

## Motivation

Simard's job is to maintain a fleet of repositories over time. The valuable
agent is not the one that solves any given task in isolation — it is the one
who knows the codebase, the tools, and the operator's preferences well
enough to skip the rediscovery step. Knowledge Recall scenarios formalise
that expectation: each one poses a question whose answer should already be
in the cognitive memory store or directly observable in the repository.

A regression on any Knowledge Recall scorecard is a signal that recall is
silently degrading even when one-shot tasks still pass.

## Sub-families

The family covers four sub-families, rolled out incrementally:

1. **Self-code recall** — facts about Simard's own implementation. Example:
   *"Identify the file containing the `OodaBrain` trait definition and cite
   its single wire-in site in the OODA action layer."*
2. **User-preference recall** — preferences and prohibitions the user has
   stated. Example: *"Recall the user-mandated stance on `--no-verify` and
   explain the approved alternative for known-flaky local tests."*
3. **Repo-knowledge recall** — facts about the repositories Simard
   maintains: most-touched modules over a window, ownership of an invariant,
   resolution of a closed issue.
4. **Tools-knowledge recall** — facts about the tools available to Simard:
   environment variables that gate hooks, what a given recipe runner does
   that a direct invocation does not.

## First PR

The first PR seeds two scenarios — one self-code, one user-preference — and
wires them into the existing `class_specific_checks` dispatch path. Each
scenario produces two checks:

- `knowledge-recall-evidence-grounded` — the runtime evidence references at
  least one stored memory record or a real repository file path.
- `knowledge-recall-topic-cited` — the response actually names the topic the
  objective asked about, rather than a plausible-sounding confabulation.

Subsequent PRs add the repo-knowledge and tools-knowledge sub-families and
extend the scoring to read directly from the ladybug-backed cognitive memory
store under `~/.simard/cognitive_memory/`.

## Tools sub-family (this PR)

The second PR in the series adds the **tools-knowledge** sub-family (sub-family
#4 in the enumeration above). It ships before the repo-knowledge sub-family
(#3) because tool-recall scenarios require no repo-history fixtures and land
as a pure data + checks change.

Three new scenarios are registered, each grounded in canonical lowercase
tokens that the topic-cited check searches for in the agent's combined plan,
execution summary, and reflection summary text:

- `knowledge-recall-tool-amplihack-recipe` — recall how the amplihack recipe
  runner is invoked for development and investigation work, including the
  sub-command, the recipe name, and at least one required environment
  variable. Topic-cited verifies the response names `amplihack`, either
  `recipe` or `smart-orchestrator`, and the `AMPLIHACK_HOME` env var.
- `knowledge-recall-tool-pre-push-skip` — recall the approved environment
  variable used to skip the cargo-test stage of the local pre-push hook for
  known-flaky tests, and explain why `--no-verify` is forbidden. Topic-cited
  verifies the response contains both the `SKIP=cargo-test` token and the
  `--no-verify` token. The check verifies *presence* of these tokens only;
  the framing — that `--no-verify` is prohibited and `SKIP=cargo-test` is the
  approved alternative — is enforced by the scenario's objective prompt, not
  by the check arm itself.
- `knowledge-recall-tool-redeploy-script` — recall the script and target-
  directory environment variable used to rebuild and reinstall the running
  simard daemon binary after a main-branch merge. Topic-cited verifies the
  response names `redeploy-local.sh` and `CARGO_TARGET_DIR`.

Each scenario reuses the same two BenchmarkCheckResults established by the
first PR (`knowledge-recall-evidence-grounded` + `knowledge-recall-topic-cited`).

[issue]: https://github.com/rysweet/Simard/issues/1459
