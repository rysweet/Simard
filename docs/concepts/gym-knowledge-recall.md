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

[issue]: https://github.com/rysweet/Simard/issues/1459

## Tools sub-family (PR #1461)

The second scenario-level PR after [#1460][pr-1460] adds three
`tools-knowledge` scenarios. Each one verifies that Simard recalls a
concrete operating-tool fact rather than confabulating an interface:

- `knowledge-recall-tool-amplihack-recipe` — recalls how the amplihack
  recipe runner is invoked for development and investigation work: the
  `amplihack recipe run` sub-command, the `smart-orchestrator` recipe
  name, and at least one required environment variable
  (`AMPLIHACK_HOME` or `AMPLIHACK_AGENT_BINARY`).
- `knowledge-recall-tool-pre-push-skip` — recalls that
  `SKIP=cargo-test` is the approved override for the cargo-test stage
  of the local pre-push hook on known-flaky tests, and explains why
  `--no-verify` is forbidden as a bypass.
- `knowledge-recall-tool-redeploy-script` — recalls that
  `scripts/redeploy-local.sh` rebuilds the simard daemon using the
  `SIMARD_SHARED_TARGET` target directory and reinstalls to
  `~/.simard/bin/simard` after a main-branch merge.

All three scenarios reuse the same two-check template seeded by the
first PR (`knowledge-recall-evidence-grounded` and
`knowledge-recall-topic-cited`), with topic matchers extended in
`src/gym/scenarios/checks_6.rs` to look for the canonical tokens above.

The next planned PR adds the `repo-knowledge` sub-family.

[pr-1460]: https://github.com/rysweet/Simard/pull/1460

## Repo-knowledge sub-family (this PR)

The third (and final scaffolding) scenario-level PR for [#1459][issue]
adds three `repo-knowledge` scenarios. Each one verifies that Simard
recalls a structural fact about her own repository layout rather than
re-deriving it from a fresh `grep`:

- `knowledge-recall-repo-ooda-loop-layout` — recalls the layout of
  Simard's OODA loop module: the four canonical phase modules under
  `src/ooda_loop/` (`observe`, `orient`, `decide`, `act`) and the file
  that holds the cycle entry point (`cycle.rs` / `mod.rs`).
- `knowledge-recall-repo-cognitive-memory-store` — recalls the storage
  backend used by the cognitive memory subsystem (`ladybug`) and the
  on-disk filename of the primary persistent store under
  `~/.simard/` (`cognitive_memory.ladybug`).
- `knowledge-recall-repo-engineer-worktree-pattern` — recalls how the
  OODA daemon spawns engineer subagents into isolated worktrees: the
  `~/.simard/engineer-worktrees/` directory and the
  `engineer-<goal-id>-<timestamp>` naming convention.

All three scenarios reuse the same two-check template seeded by the
first PR (`knowledge-recall-evidence-grounded` and
`knowledge-recall-topic-cited`). Topic matchers for the new scenarios
live in `src/gym/scenarios/checks_7.rs`, split out of `checks_6.rs` to
respect the 400-LOC per-module cap (#1266).

This PR completes the four-sub-family scaffolding from [#1459][issue]:
self-code, user-preference, tools-knowledge, and repo-knowledge.

## Cross-session recall (this PR)

The capstone PR for the four-sub-family roadmap from [#1459][issue]
adds a fifth sub-family: **cross-session recall**. Where the previous
sub-families verify that Simard remembers facts about her own code,
her tools, the repos she maintains, and the operator's preferences,
cross-session recall verifies the underlying capability that makes any
of those memories useful in the long run — that they **survive session
boundaries**.

This is the hardest variant of the family because every other check in
this file can be satisfied by the agent re-deriving the answer from the
current session's prompt context. Cross-session checks deliberately
cannot: each scenario's check looks for tokens that only make sense if
the agent read accumulated cognitive memory from a *prior* gym run.
That makes these scenarios a direct stress test of the cognitive memory
persistence subsystem — and a complement to the still-wedged
`improve-cognitive-memory-persistence` daemon goal, whose whole purpose
is to make that subsystem reliable across restarts.

The two new scenarios are:

- `knowledge-recall-cross-session-fact` — recalls a fact stored in a
  prior gym session as a memory tagged `gym-cross-session-canary` with
  a deterministic canary token. The check requires the runtime evidence
  to cite the canary token, the tag, and the prior-session origin from
  accumulated cognitive memory.
- `knowledge-recall-cross-session-preference` — recalls the
  user-stated preference that Simard's brain be prompt-driven rather
  than code-driven, including the date the preference was stated
  (Apr 29) and the architectural pattern it produced
  (`prompt_assets/simard/*.md` + `include_str!` + an LLM trait such as
  `OodaBrain`).

Both scenarios reuse the same two-check template seeded by the first
PR (`knowledge-recall-evidence-grounded` and
`knowledge-recall-topic-cited`), with topic matchers tuned for
cross-session tokens in `src/gym/scenarios/checks_8.rs` (split out of
`checks_7.rs` to respect the 400-LOC per-module cap from #1266).

This PR closes out the four-sub-family roadmap from [#1459][issue]:
self-code, user-preference, tools-knowledge, repo-knowledge, and now
cross-session.
