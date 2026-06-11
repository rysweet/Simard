---
title: "Reference: Engineer workflow and merge-ready contract"
description: Normative specification for the mandatory workflow rule and 6-check merge-ready gate.
last_updated: 2026-06-11
review_schedule: as-needed
owner: simard
---

# Reference: Engineer Workflow and Merge-Ready Contract

This is the normative specification for the two mandatory rules enforced
on every Simard engineer cycle. The rules are embedded in:

- `prompt_assets/simard/engineer_system.md` — the `⛔ MANDATORY RULES`
  section (read by every engineer session)
- `prompt_assets/simard/goal_session_objective.md` — the PM-side gating
  rule and evidence table (read by every goal-session cycle)

For design rationale, see
[Concept: mandatory workflow and merge-ready gates](../concepts/mandatory-workflow-merge-gates.md).
For a practical walkthrough, see
[How-to: validate merge readiness](../howto/validate-merge-readiness.md).

---

## Rule 1: Workflow Contract

### Statement

> The FIRST tool action of every engineer cycle that will modify code
> MUST be one of:
>
> - `Skill(skill="dev-orchestrator")` (interactive)
> - `amplihack recipe run smart-orchestrator ...` (non-interactive)
>
> Direct `edit`/`create` of source files outside the workflow is
> forbidden.

### Source

`prompt_assets/simard/engineer_system.md`, section `## ⛔ MANDATORY
RULES — Read Before Any Work`, Rule 1; and section `## Workflow Contract
(MUST)`.

### Entry points

| Mode | Command | Notes |
|------|---------|-------|
| Interactive (Claude Code, Copilot CLI) | `Skill(skill="dev-orchestrator")` | Auto-launches smart-orchestrator recipe |
| Non-interactive / scripted | `amplihack recipe run smart-orchestrator -c task_description="..." -c repo_path=.` | Direct recipe invocation |

### Required environment

| Variable | Purpose | Default |
|----------|---------|---------|
| `AMPLIHACK_HOME` | Root of amplihack installation | Auto-detected from cwd |
| `AMPLIHACK_AGENT_BINARY` | Which agent binary nested sessions use | Set by launcher |
| `CLAUDECODE` | Must be **unset** so nested sessions can launch | — |

### Adaptive strategy (infrastructure failure only)

If `smart-orchestrator` fails at the **infrastructure level** (not
because the task seems simple), the engineer MAY invoke a direct workflow
recipe — but MUST announce the adaptation explicitly:

| Classification | Direct recipe |
|----------------|---------------|
| Investigation only | `amplihack recipe run investigation-workflow ...` |
| Development | `amplihack recipe run default-workflow ...` |

"The task seems simple" is NOT an infrastructure failure and is NOT a
permitted bypass reason.

### Exceptions

Direct `edit`/`create` without the recipe runner is permitted ONLY for:

| Exception | Condition |
|-----------|-----------|
| Documentation typo | Single-line, no semantic change to behavior or examples |
| Commit message edit | `git commit --amend`, `git rebase -i` reword only |
| Scratch files | Under `/tmp`, never committed |

Everything else — including bug fixes, dependency bumps, prompt tweaks,
README edits >1 line, and test additions — MUST go through the workflow.

### Violation consequences

Cycles that bypass the workflow trigger `reclaim_and_redispatch` from
the OODA brain. The cycle's outputs are discarded and the work is
re-dispatched as a new engineer cycle with a corrective task description.

---

## Rule 2: Merge-Ready Contract

### Statement

> Before merging any PR, the engineer MUST verify all six merge-ready
> criteria are satisfied with concrete evidence in the PR description.

### Source

`prompt_assets/simard/engineer_system.md`, section `⛔ MANDATORY RULES`,
Rule 2; section `## Merge-Ready Contract`; and section `## Definition of
Done`.

`prompt_assets/simard/goal_session_objective.md`, section `# Priority
Order`, item 1 (evidence table and gating rule).

### The six criteria

| # | Criterion | Evidence standard | Tool |
|---|-----------|-------------------|------|
| 1 | **QA-team** | Scenarios written, validated (`gadugi-test validate`), run (`gadugi-test run`). Output pasted or linked. | `gadugi-test` |
| 2 | **Documentation** | User-facing docs updated for APIs, config, CLI, deployment changes. OR explicit internal-only justification. | Manual review |
| 3 | **Quality-audit** | ≥3 SEEK→VALIDATE→FIX cycles. Final cycle clean (0 critical/high, 0 medium correctness/security). Cycle count and commit SHAs cited. | `amplihack quality-audit` |
| 4 | **CI** | All CI checks green, 0 failures. Link to green run. | `gh pr checks` |
| 5 | **PR description** | All six headings populated with concrete evidence (not placeholders). | Manual review |
| 6 | **Scope** | Diff contains no unrelated changes. Summary confirming focus. | `gh pr diff` |

### PR description template

Every PR opened by an engineer cycle MUST contain these six headings:

```markdown
## QA-team evidence
(scenarios + validate + run results)

## Documentation
(surfaces touched + doc updates, or internal-only justification)

## Quality-audit
(≥3 SEEK→VALIDATE→FIX cycles ending clean, with commit SHAs)

## CI
(link to the green run for every required check)

## Scope
(diff summary confirming no unrelated edits)

## Verdict
(explicit "ready to merge" / "draft" / "blocked" with rationale)
```

### PM-side gating rule

From `goal_session_objective.md`:

> You MUST NOT instruct an engineer to merge a PR unless you have
> reviewed the PR description and confirmed that ALL SIX criteria above
> have substantive evidence (not placeholders, not "will do later"). If a
> PR is CI-green but missing merge-ready evidence, tell the engineer to
> run the merge-ready process on it — do NOT merge without evidence and
> do NOT tell the engineer to merge without evidence.

### Merge command

Once all criteria are verified:

```bash
# Preferred
simard merge-pr <PR_NUMBER>

# Fallback (if simard binary lacks merge-pr)
gh pr merge --squash --delete-branch <PR_NUMBER>
```

If using the fallback, the engineer MUST note the deviation under the
PR's **Verdict** heading.

### Violation consequences

- **Opening a PR without all six headings:** triggers
  `reclaim_and_redispatch`.
- **PM instructing merge without verified evidence:** violates the
  gating rule; the engineer should refuse and request evidence
  completion.
- **Merging a PR with placeholder evidence:** the merge-readiness judge
  will flag it; if merged anyway, a corrective issue is filed.

---

## Definition of Done (DoD)

Every code-producing engineer cycle is NOT complete until all of the
following have happened:

1. **Commit** — descriptive subject, informative body, issue references,
   `Co-authored-by` trailer.
2. **Push** — feature branch pushed with pre-push hooks intact (no
   `--no-verify`).
3. **PR opened** — with all six evidence headings filled.
4. **Drive to merge** — once CI is green and all six headings have
   evidence, merge via `simard merge-pr` or `gh pr merge --squash
   --delete-branch`.

### Allowed DoD exceptions

A cycle MAY end without a merged PR only if `engineer_summary` records
which case applied:

| Exception | What to record |
|-----------|---------------|
| Pure exploration / investigation | What was learned, files inspected, hypotheses confirmed/falsified |
| Refactor not yet ready | Why not ready, specific blocker, next step to unblock |
| Work already done | Existing PR number or commit SHA, one-line confirmation it satisfies the ask |

---

## Enforcement matrix

| Rule | Engineer prompt | PM objective | OODA brain | PostToolUse hook |
|------|----------------|--------------|------------|-----------------|
| Workflow (Rule 1) | `⛔ MANDATORY RULES` Rule 1 | — | `reclaim_and_redispatch` on direct edits | 3-call warning without workflow evidence |
| Merge-ready (Rule 2) | `⛔ MANDATORY RULES` Rule 2 + `Merge-Ready Contract` + `Definition of Done` | Evidence table + `⛔ GATING RULE` | `reclaim_and_redispatch` on missing headings | — |

---

## Prompt asset locations

| File | Runtime path | Purpose |
|------|-------------|---------|
| `prompt_assets/simard/engineer_system.md` | `~/.simard/prompt_assets/simard/engineer_system.md` | Engineer system prompt (contains both rules) |
| `prompt_assets/simard/goal_session_objective.md` | `~/.simard/prompt_assets/simard/goal_session_objective.md` | PM goal-session objective (contains PM-side gate) |
| `prompt_assets/simard/merge_readiness_judge.md` | `~/.simard/prompt_assets/simard/merge_readiness_judge.md` | Merge-readiness judge recipe prompt |

Prompt edits take effect on the next cycle without a rebuild — see
[How-to: edit the engineer system prompt](../howto/edit-the-engineer-system-prompt.md).

---

## Version history

| Date | Change | Issue/PR |
|------|--------|----------|
| 2026-06-11 | Initial: embedded `⛔ MANDATORY RULES` in engineer prompt, 6-row evidence table in PM objective | #2267 / PR #2266 |

---

## Related

- [Concept: mandatory workflow and merge-ready gates](../concepts/mandatory-workflow-merge-gates.md) —
  design rationale
- [How-to: validate merge readiness](../howto/validate-merge-readiness.md) —
  step-by-step validation guide
- [How-to: edit the engineer system prompt](../howto/edit-the-engineer-system-prompt.md) —
  how to modify the prompt assets
- [Reference: runtime contracts](runtime-contracts.md) —
  broader contract reference
- [Reference: OODA engineer lifecycle recipe](ooda-engineer-lifecycle-recipe.md) —
  how the OODA brain enforces lifecycle decisions
