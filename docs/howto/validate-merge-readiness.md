---
title: "How-To: Validate merge readiness"
description: Step-by-step guide for running the 6-check merge-ready validation before merging any PR.
last_updated: 2026-06-11
review_schedule: as-needed
owner: simard
---

# How-To: Validate Merge Readiness

Before merging any PR in the Simard ecosystem, you must verify all six
merge-ready criteria. This guide walks through each check and shows how
to produce the evidence the PM and merge-readiness judge expect.

## TL;DR

1. Run QA scenarios → paste output in PR description.
2. Check docs → update or justify "internal-only".
3. Run ≥3 quality-audit cycles → cite commit SHAs.
4. Confirm CI green → link the run.
5. Update PR description with evidence under six headings.
6. Confirm diff scope → summarize focus.

Then: `gh pr merge --squash --delete-branch <PR>` (or `simard merge-pr <PR>`
if available).

## Prerequisites

- A PR that is ready for final validation (feature complete, rebased on
  main).
- The `gadugi-test` CLI installed (for QA scenarios).
- Access to the CI dashboard (GitHub Actions).
- The `amplihack` CLI installed (for quality-audit).

## Step 1: QA-team scenarios

Write outside-in test scenarios that exercise the PR's changes from a
user perspective, then validate and run them:

```bash
# Write scenarios (if not already written during the workflow)
gadugi-test write --pr <PR_NUMBER>

# Validate scenario syntax and coverage
gadugi-test validate

# Run the scenarios
gadugi-test run
```

**Evidence required:** Paste or link the `gadugi-test run` output showing
scenario results. All scenarios must pass. If any fail, fix them before
proceeding.

### PR description heading

```markdown
## QA-team evidence

gadugi-test validate: ✅ 4/4 scenarios valid
gadugi-test run: ✅ 4/4 scenarios passed

<details>
<summary>Full output</summary>

(paste gadugi-test run output here)

</details>
```

## Step 2: Documentation

Check whether the PR changes any user-facing surface:

- CLI flags or subcommands
- Configuration options or environment variables
- API endpoints or response formats
- Deployment procedures
- Prompt assets that affect operator-visible behavior

If yes: update the corresponding docs under `docs/` and link the changes.

If no user-facing surfaces changed: provide an explicit justification.

### PR description heading

```markdown
## Documentation

No user-facing surfaces changed. This PR modifies internal prompt assets
only — no CLI, API, config, or deployment changes.
```

Or:

```markdown
## Documentation

Updated:
- `docs/howto/validate-merge-readiness.md` — new how-to for the 6-check gate
- `docs/reference/engineer-workflow-merge-contract.md` — normative spec
```

## Step 3: Quality-audit

Run at least three SEEK→VALIDATE→FIX cycles. Each cycle:

1. **SEEK** — scan the diff for issues (bugs, security, correctness).
2. **VALIDATE** — confirm each finding against the actual code.
3. **FIX** — address confirmed findings and commit.

The final cycle must be clean: zero critical/high findings and zero
medium correctness/security findings.

```bash
# Run quality-audit (the amplihack skill handles the cycles)
amplihack recipe run smart-orchestrator \
  -c task_description="quality-audit PR #<NUMBER>" \
  -c repo_path=.
```

**Evidence required:** Cite the number of cycles completed and the
commit SHAs for each fix cycle.

### PR description heading

```markdown
## Quality-audit

3 SEEK→VALIDATE→FIX cycles completed:
- Cycle 1: Found 2 medium issues → fixed in abc1234
- Cycle 2: Found 1 low issue → fixed in def5678
- Cycle 3: Clean — 0 critical, 0 high, 0 medium correctness/security
```

## Step 4: CI

All CI checks must be green with zero failures. Do not merge a PR with
any failing or pending checks.

```bash
# Check CI status
gh pr checks <PR_NUMBER>
```

**Evidence required:** Link to the green CI run.

### PR description heading

```markdown
## CI

All checks green: https://github.com/rysweet/Simard/actions/runs/12345678
```

## Step 5: PR description

By this point you should have filled in headings for QA-team,
Documentation, Quality-audit, and CI. Review the PR description to
confirm:

- All four headings above have substantive evidence (not placeholders).
- The Scope heading (Step 6) is filled.
- The Verdict heading reflects the current state.

### PR description heading

```markdown
## PR description

All six evidence headings populated with concrete evidence.
```

## Step 6: Scope

Review the diff to confirm it contains only changes related to the PR's
stated purpose. No unrelated "drive-by" edits.

```bash
# Review the diff
gh pr diff <PR_NUMBER> | head -100
```

### PR description heading

```markdown
## Scope

Diff contains 2 files changed (prompt_assets/simard/engineer_system.md,
prompt_assets/simard/goal_session_objective.md). Both directly related
to the stated goal. No unrelated changes.
```

## Step 7: Verdict and merge

Add a verdict heading with an explicit call:

```markdown
## Verdict

**Ready to merge.** All six criteria verified with evidence above.
```

Then merge:

```bash
# Preferred: use Simard's merge-pr command (if available)
simard merge-pr <PR_NUMBER>

# Fallback: direct gh merge
gh pr merge --squash --delete-branch <PR_NUMBER>
```

## When a criterion fails

Do NOT merge. Instead:

1. Note which criterion failed and why.
2. Fix the issue (add missing scenarios, update docs, run another
   quality-audit cycle, wait for CI, remove unrelated changes).
3. Re-run the failing check.
4. Update the PR description with the new evidence.
5. Return to Step 7.

## PM validation

When Simard (as PM) reviews a PR for merge authorization, she checks the
same six headings. If any heading contains placeholders ("TBD", "will do
later", empty) or lacks substantive evidence, the PM will instruct the
engineer to run the merge-ready process — not merge.

This creates a two-layer gate:

1. **Engineer** produces evidence during the workflow.
2. **PM** verifies evidence before authorizing merge.

## Common mistakes

| Mistake | Why it fails | Fix |
|---------|-------------|-----|
| "QA-team: ✅" with no output | No evidence — just a checkmark | Paste actual `gadugi-test run` output |
| "Docs: N/A" without justification | Reviewers can't verify the claim | State which surfaces were checked and why none changed |
| Quality-audit with only 1 cycle | Minimum is 3 cycles | Run 2 more cycles |
| CI link to a different PR's run | Evidence doesn't match | Link the run for THIS PR |
| "Scope: looks good" | Not specific enough | List the files changed and confirm focus |

## Related

- [Concept: mandatory workflow and merge-ready gates](../concepts/mandatory-workflow-merge-gates.md) —
  design rationale for both rules
- [Reference: engineer workflow and merge-ready contract](../reference/engineer-workflow-merge-contract.md) —
  normative specification
- [How-to: edit the engineer system prompt](edit-the-engineer-system-prompt.md) —
  how the rules are embedded in the prompt
