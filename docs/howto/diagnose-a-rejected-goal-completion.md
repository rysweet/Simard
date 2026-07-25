---
title: How to diagnose a rejected goal completion
description: Operator runbook for "why won't this goal archive?" — read the completion blocker, find the missing evidence (unmerged PR, open issue, undeployed self-change), and resolve or override the deploy-aware done-gate.
last_updated: 2026-06-27
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/deploy-aware-done-gate.md
  - ../concepts/cross-repo-completion-reconciliation.md
  - ../reference/completion-evidence-gate-api.md
  - ../reference/cross-repo-merged-pr-evidence.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../howto/diagnose-rejected-progress-claims.md
  - ../operations/progress-evidence-kill-switch.md
---

# How to diagnose a rejected goal completion

> **Status: implemented.** The deploy-aware done-gate
> (`CompletionEvidenceGate`, the completion-blocked annotations, and the
> `SIMARD_COMPLETION_EVIDENCE` kill-switch) described here lives in
> [`src/goal_curation/completion_gate.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
> (see [deploy-aware-done-gate](../concepts/deploy-aware-done-gate.md)). The
> production daemon annotates blocked goals' `current_activity` with the missing
> evidence, which `simard goal-curation read` surfaces.

A goal you expected to be **done** is still on the active board. The deploy-aware
done-gate keeps a goal active until it has hard evidence of completion; this
guide shows how to read the blocker and resolve it. For the design, see
[deploy-aware-done-gate](../concepts/deploy-aware-done-gate.md).

## Read the blocker

When the gate rejects a completion it annotates the goal's `current_activity`
with the missing evidence and records a `CompletionBlocked` entry. Inspect the
active goals:

```bash
simard goal-curation read
```

A blocked goal shows its outstanding evidence, for example:

```text
goal: Restore cognitive-memory backups | status=active
  blocked: completion rejected — missing [PrNotMerged, IssueOpen]
```

The blocker is one or more of:

| Missing evidence | Meaning | Resolve by |
| --- | --- | --- |
| `PrNotMerged` | no merged PR is linked to this goal | merge the PR, then re-link it in the goal's `wip_refs` |
| `IssueOpen` | the linked issue is still open | close the issue (usually automatic on PR merge) |
| `NotDeployed` | a self-affecting change is merged but not running | run / await a self-deploy (see below) |
| `CouldNotVerify` | a `gh`/git/drift query failed this cycle | retry next cycle; check network and `gh auth status` |

## Resolve `PrNotMerged` / `IssueOpen`

These are the ordinary cases. Merge the PR and close the issue. Confirm the goal
links the merged PR — the gate matches a PR in the goal's `wip_refs` (kind `pr`)
or a merged PR that references the goal's issue. Once both hold, the goal
archives on the next curation cycle.

## Resolve `NotDeployed` (self-affecting changes)

`NotDeployed` means the change is merged but Simard is still running the old
binary — clause 3 of the done-gate. Confirm the drift and let the self-deploy
close it:

```bash
simard self-health --json | jq '.probes.version_advanced'
```

If `version_advanced.healthy` is `false`, a [safe self-deploy](../howto/verify-and-roll-back-a-self-deploy.md)
needs to run. Once the new binary is running and verified, the `NotDeployed`
blocker clears and the goal archives. This is the intended coupling: a
self-affecting goal is not "done" until its code is **running**.

## Reproduce the original false-completion (sanity check)

The gate exists because a goal was once archived "complete" with no merged PR and
an open issue (the cognitive-memory backup goal). To confirm the gate is active,
a goal in that exact shape — `status=Completed`, no merged PR, issue open — must
**not** archive; it stays active with `[PrNotMerged, IssueOpen]`. This is covered
by a regression test (see
[completion-evidence-gate API](../reference/completion-evidence-gate-api.md#archive-integration)).

## Cross-repo re-block loop

If a goal that `simard goal list` shows as **completed** re-emits

```text
OODA curate: completion BLOCKED for goal '<id>' — missing PR not merged
```

on **every** cycle even though its PR is merged, its merged PR most likely lives
in another ecosystem repo (e.g. `rysweet/agent-kgpacks-rs`). The merged-PR check
is repo-relative: it resolves against the goal's own target repo and reads the
persisted PR linkage (numeric `ref_id` **or** `url`). Confirm both are present:

- The goal carries a **qualified** `goal.repo` (`owner/repo`) *or* a `pr`
  `WipRef` whose `url` is a full GitHub PR URL
  (`https://github.com/<owner>/<repo>/pull/<num>`).
- The `debug` trace shows the gate querying the **expected repo and PR number**.

If neither the `ref_id` nor a parseable PR URL is present, re-link the merged PR
in the goal's `wip_refs` (with its URL) so the gate can resolve it. For the
design, see
[cross-repo completion reconciliation](../concepts/cross-repo-completion-reconciliation.md)
and the
[cross-repo merged-PR evidence API reference](../reference/cross-repo-merged-pr-evidence.md).

## Override the gate (recovery only)

If the gate itself is defective and is wrongly blocking a genuinely complete
goal, you can temporarily disable it:

```bash
SIMARD_COMPLETION_EVIDENCE=off simard goal-curation read
```

With the gate off, the legacy unguarded archive behaviour returns. Use this only
to recover from a gate defect — never as normal operation — and re-enable the gate
(unset the variable) immediately afterwards. This mirrors the
[progress-evidence kill-switch](../operations/progress-evidence-kill-switch.md).

## See also

- [deploy-aware-done-gate concept](../concepts/deploy-aware-done-gate.md)
- [Cross-repo completion reconciliation](../concepts/cross-repo-completion-reconciliation.md) — the cross-repo re-block loop and its fix.
- [Completion-evidence gate API reference](../reference/completion-evidence-gate-api.md)
- [How to verify and roll back a self-deploy](../howto/verify-and-roll-back-a-self-deploy.md)
- [How to diagnose rejected progress claims](../howto/diagnose-rejected-progress-claims.md) — the sibling percent-increase gate.
