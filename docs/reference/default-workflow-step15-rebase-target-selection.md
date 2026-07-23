---
title: default-workflow Step 15 rebase-target selection
description: Cross-repo reference (lives in rysweet/amplihack-rs) for how default-workflow Step 15 chooses the base each temporary/workstream branch is rebased onto — its creation branch / PR target / merge-base — instead of an unrelated upstream, and fails loudly via structured tracing when the base is indeterminate rather than producing conflicted trees (rysweet/amplihack-rs#978).
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/reconcile-and-self-deploy.md
  - ./cross-repo-merge-authority.md
  - ./pr-finalization-pipeline.md
  - ./no-bridge-naming-guard.md
---

# default-workflow Step 15 rebase-target selection

> **Status: implemented — cross-repo.** This behaviour lives in the
> **`rysweet/amplihack-rs`** repository (the shared `default-workflow`
> recipe that all governed repos route through via `smart-orchestrator`),
> **not** in `rysweet/Simard`. It is documented here because Simard is a
> governed consumer of `default-workflow` and a bad Step 15 rebase affects
> every repo's workstreams. Source of truth: the `default-workflow` recipe
> definition and its Step 15 rebase logic in `rysweet/amplihack-rs`
> (rysweet/amplihack-rs#978).

`default-workflow` is the shared workflow every governed repository executes
through `smart-orchestrator`. **Step 15** finalizes each temporary/workstream
branch by rebasing it before hand-off/merge. The defect (`#978`) was that
Step 15 selected an **unrelated upstream** as the rebase base, so branches were
replayed onto commits they never descended from — producing conflicts and, in
the worst case, splicing unreviewed commits into a workstream. Because
`default-workflow` is shared, this had a repo-wide blast radius.

This reference specifies the finished Step 15 base-selection contract.

## Contents

- [Base-selection ladder](#base-selection-ladder)
- [Fail-loud on indeterminate base](#fail-loud-on-indeterminate-base)
- [Never force-push shared branches](#never-force-push-shared-branches)
- [Regression coverage](#regression-coverage)
- [Observability](#observability)

## Base-selection ladder

Step 15 resolves the rebase base for a workstream branch from a **verified
source**, in order, and uses the first that is determinable:

1. **PR target branch.** If the workstream branch has an open PR, rebase onto
   the PR's base (target) branch — the branch it is actually going to merge
   into.
2. **Creation branch.** Otherwise, rebase onto the branch the workstream was
   created from (its recorded parent / start point).
3. **Merge-base.** Otherwise, compute the `git merge-base` between the
   workstream branch and its intended integration branch and rebase onto that.

The selected base is **always a branch the workstream branch descends from or
targets** — never an arbitrary or "latest upstream" branch chosen for
recency.

## Fail-loud on indeterminate base

If none of the ladder steps yields a verified base, Step 15 **stops and fails
loudly** rather than guessing:

- it emits a `tracing::error!` structured event naming the workstream branch and
  the reason the base could not be determined;
- it does **not** rebase onto a fallback/unrelated upstream;
- it does **not** produce or push a conflicted tree.

Silent degradation into a conflicted merge is prohibited — the operator sees an
explicit failure and the branch is left untouched for manual resolution.

## Never force-push shared branches

Rebasing rewrites history, so Step 15's push discipline is:

- it only rewrites the **temporary/workstream** branch, never a shared or
  protected branch;
- it never runs `--force` / `--force-with-lease` against `main` or any
  protected integration branch;
- selecting the base from a verified source (above) guarantees the rebase never
  splices unreviewed commits from an unrelated upstream into the workstream.

## Regression coverage

A regression test covers Step 15 base selection and asserts:

| Scenario | Expected base |
| --- | --- |
| workstream branch has an open PR | the PR target branch |
| no PR, known creation branch | the creation branch |
| no PR, no recorded creation branch | `merge-base` with the integration branch |
| none determinable | **no rebase**; loud `tracing::error!`; branch unchanged |

The test explicitly rejects the pre-fix behaviour (rebasing onto an unrelated
upstream) and the silent-conflict path.

## Observability

Step 15 is instrumented with **structured `tracing` + OTel only — no
`print!`/`println!`**. It records the chosen base, the ladder step that
produced it, and — on the indeterminate path — a `tracing::error!` with the
branch name and cause. `rysweet/amplihack-rs#978` is closed by this change.
