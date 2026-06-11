---
title: Mandatory workflow and merge-ready gates
description: Why every engineer cycle must use the recipe runner and why every PR merge requires 6-check evidence.
last_updated: 2026-06-11
review_schedule: as-needed
owner: simard
---

# Mandatory Workflow and Merge-Ready Gates

Simard enforces two non-negotiable rules on every engineer cycle. This
document explains the design rationale behind each rule, the failure modes
they prevent, and the enforcement surfaces that make them stick.

## Background

Between issues #1712 and #1714, a pattern emerged: engineer sessions that
bypassed the amplihack default workflow produced code that looked correct
in isolation but lacked the quality gates the workflow enforces —
inspection before action, qa-team coverage, quality-audit cycles, and
evidence-backed PR descriptions. The result was uncommitted-edit drift,
missed evidence headings, accidental data loss, and recursive cycle
thrash.

A second pattern appeared around merge time: PRs were merged CI-green but
without verified evidence for documentation, QA, quality-audit, or scope
focus. The merge-readiness judge (`merge_readiness_judge.md`) existed but
was not enforced as a hard gate — engineers could (and did) skip it.

The two mandatory rules convert these lessons into hard constraints
embedded directly in the engineer system prompt and the PM goal-session
objective.

## Rule 1: All code changes must go through the recipe runner

### What it says

The FIRST tool action of every code-producing engineer cycle MUST be one
of:

- **Interactive:** `Skill(skill="dev-orchestrator")` — the
  dev-orchestrator skill auto-launches the smart-orchestrator recipe.
- **Non-interactive:** `amplihack recipe run smart-orchestrator -c
  task_description="..." -c repo_path=.`

Direct `edit`/`create` of source files outside the workflow is forbidden.

### Why

The amplihack workflow encodes the quality discipline accumulated across
hundreds of engineer cycles:

1. **Inspection before action** — Step 0 reads the codebase before
   proposing changes.
2. **Planning with verification steps** — the plan explicitly lists how
   each change will be verified.
3. **QA-team coverage** — the workflow gates on scenario creation and
   validation.
4. **Quality-audit cycles** — ≥3 SEEK→VALIDATE→FIX passes catch
   regressions the author missed.
5. **Evidence-backed PR descriptions** — the workflow populates the six
   merge-ready headings automatically.
6. **Merge-ready gating** — the workflow invokes the merge-readiness
   judge before declaring completion.
7. **Recursion guards** — the recipe runner prevents infinite agent
   spawning.
8. **Goal verification** — post-execution reflection confirms the
   original goal was met.

Skipping the workflow means skipping all eight of these. "The task seems
simple" is not a valid reason — simple tasks take seconds through the
workflow and get the same quality coverage.

### Narrow exceptions

Direct `edit`/`create` without the recipe runner is permitted ONLY for:

1. Trivial single-line documentation typos (no semantic change).
2. Editing commit messages (`git commit --amend`, `git rebase -i`
   reword).
3. Editing scratch files under `/tmp` that are never committed.

Everything else — including "small" bug fixes, dependency bumps, prompt
tweaks, README edits longer than one line, and test additions — MUST go
through the workflow.

### Enforcement surfaces

| Surface | Mechanism |
|---------|-----------|
| Engineer system prompt | `⛔ MANDATORY RULES` section, Rule 1 — the LLM reads this before every action |
| OODA brain lifecycle | `reclaim_and_redispatch` fires when the cycle output shows direct edits without workflow evidence |
| PostToolUse hook | `amplihack-hooks` monitors tool calls; 3+ calls without recipe-runner evidence triggers a hard WARNING |
| Forbidden anti-patterns | The prompt explicitly lists "bypassing the workflow" as a `reclaim_and_redispatch` trigger |

## Rule 2: All PR merges must pass 6-check merge-ready validation

### What it says

Before merging any PR, the engineer MUST verify all six merge-ready
criteria are satisfied with concrete evidence in the PR description:

| # | Criterion | What constitutes evidence |
|---|-----------|--------------------------|
| 1 | **QA-team** | Scenarios written, validated (`gadugi-test validate`), and run (`gadugi-test run`) — output pasted or linked |
| 2 | **Documentation** | User-facing docs updated if change affects APIs, config, CLI, or deployment — OR explicit internal-only justification |
| 3 | **Quality-audit** | ≥3 SEEK→VALIDATE→FIX cycles completed, final cycle clean (zero critical/high findings) — cycle count and commit SHAs cited |
| 4 | **CI** | All CI checks green (0 failures) — link to the green run |
| 5 | **PR description** | Updated with concrete evidence for criteria 1–4 and 6 under the six standard headings |
| 6 | **Scope** | Diff contains no unrelated changes — summary confirming focus |

### Why

CI-green is necessary but not sufficient. A PR can pass all CI checks
while:

- Having zero QA scenarios (no outside-in test coverage).
- Lacking documentation for new CLI flags or config options.
- Never running a quality-audit (latent bugs remain).
- Containing unrelated "drive-by" edits that complicate future bisects.
- Having an empty or placeholder PR description that tells reviewers
  nothing.

The six-check gate ensures that every merged PR carries its own audit
trail. When a regression surfaces later, the PR description contains the
evidence needed to understand what was tested, what was reviewed, and
what was intentionally in or out of scope.

### PM-side enforcement

The goal-session objective (`goal_session_objective.md`) contains a
matching gating rule:

> You MUST NOT instruct an engineer to merge a PR unless you have
> reviewed the PR description and confirmed that ALL SIX criteria above
> have substantive evidence (not placeholders, not "will do later").

This creates a two-layer gate: the engineer must produce the evidence,
and the PM must verify it before authorizing merge. Neither layer alone
is sufficient — the PM gate catches cases where the engineer populated
evidence headings with placeholders, and the engineer gate catches cases
where the PM forgot to check.

### Enforcement surfaces

| Surface | Mechanism |
|---------|-----------|
| Engineer system prompt | `⛔ MANDATORY RULES` section, Rule 2 — engineers cannot merge without evidence |
| Goal-session objective | 6-row evidence table + `⛔ GATING RULE` — PM cannot instruct merge without verified evidence |
| Merge-readiness judge | `merge_readiness_judge.md` recipe evaluates each criterion and produces a PASS/FAIL verdict |
| Definition of Done | The DoD section requires all six headings filled before a PR is considered complete |
| Forbidden anti-patterns | "Opening a PR without all six evidence headings" triggers `reclaim_and_redispatch` |

## Interaction between the two rules

The rules are complementary:

1. **Rule 1** ensures the workflow produces the evidence.
2. **Rule 2** ensures the evidence is verified before merge.

When an engineer follows Rule 1 (uses the recipe runner), the workflow
automatically generates QA scenarios, runs quality-audit cycles, and
populates the PR description with evidence headings. Rule 2 then becomes
a verification step rather than a manual evidence-gathering exercise.

When an engineer bypasses Rule 1, Rule 2 becomes much harder to satisfy
because the evidence was never generated. This is by design — the rules
create a natural dependency that makes compliance the path of least
resistance.

## Failure modes prevented

| Failure mode | Which rule prevents it | Historical incident |
|---|---|---|
| Uncommitted-edit drift | Rule 1 (workflow commits atomically) | #1712 |
| Missing evidence headings | Rule 2 (PM gate rejects empty headings) | #1714 |
| Recursive cycle thrash | Rule 1 (recipe runner has recursion guards) | #1712 |
| Data loss from direct edits | Rule 1 (workflow uses worktree isolation) | #1714 |
| Merging without QA coverage | Rule 2 (criterion 1 requires scenarios) | Multiple |
| Drive-by unrelated changes | Rule 2 (criterion 6 requires scope focus) | Multiple |
| Placeholder "will do later" evidence | Rule 2 (PM gating rule rejects placeholders) | Multiple |

## Related

- [How-to: validate merge readiness](../howto/validate-merge-readiness.md) —
  step-by-step guide for running the 6-check validation
- [How-to: edit the engineer system prompt](../howto/edit-the-engineer-system-prompt.md) —
  how to modify engineer behavior through prompt edits
- [Reference: engineer workflow and merge-ready contract](../reference/engineer-workflow-merge-contract.md) —
  normative specification of both rules
- [Concept: prompt-driven TDD discipline](prompt-driven-tdd-discipline.md) —
  why behavioral rules are enforced through prompts
- [Concept: prompt-driven brain iteration](prompt-driven-brain-iteration.md) —
  how prompt edits take effect without rebuilds
