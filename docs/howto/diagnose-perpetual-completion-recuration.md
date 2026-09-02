---
title: Diagnose perpetual completion re-curation
description: Operator playbook for the "completed goal re-blocks with PR not merged every OODA cycle" signature. Confirm the churn in the journal, verify the goal's PR really merged, check whether its `pr` wip_ref was pruned, and confirm the issue-based merged-PR fallback recovers the evidence so the goal archives. Includes the fail-closed CouldNotVerify path and cross-repo checks.
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ./diagnose-a-rejected-goal-completion.md
  - ../concepts/completion-gate-merged-pr-reconciliation.md
  - ../reference/completion-gate-issue-fallback-api.md
  - ../reference/completion-evidence-gate-api.md
  - ../reference/wip-ref-liveness-reconcile-api.md
  - ../../src/goal_curation/completion_gate.rs
  - ../../src/ooda_loop/cycle.rs
---

# Diagnose perpetual completion re-curation

Use this playbook when a goal shows `STATUS=completed` but the OODA curate step
re-blocks it with **"PR not merged"** on **every** cycle — the systemic
reconciliation churn from issue #12. For a rejected-once completion (not a
repeating loop) see
[Diagnose a rejected goal completion](./diagnose-a-rejected-goal-completion.md)
instead.

## Symptom

The journal emits the same line, cycle after cycle, for a goal that already
reports completed:

```text
[simard] OODA curate: completion BLOCKED for goal <id> — missing PR not merged
```

```console
$ simard goal list | grep <id>
<id>    completed    …
```

Board says `completed`; the gate says `PrNotMerged`; the emission repeats and
the goal never archives.

## 1. Confirm the churn (not a one-off)

Count the emissions over a window. A handful is a transient; dozens of identical
lines is the systemic signature.

```console
$ simard journal --since 6h | grep -c "completion BLOCKED for goal <id>"
17
```

If the count roughly equals your cycle count for the window (e.g. 17 lines over
17 cycles), you have the churn.

## 2. Verify the PR actually merged

The fix assumes the goal is *genuinely* done. Confirm the merge before anything
else — use the goal's **own** repo for cross-repo goals (e.g.
`rysweet/agent-kgpacks-rs`):

```console
$ gh pr list --repo rysweet/agent-kgpacks-rs --state merged --search "closes #18"
$ gh issue view 18 --repo rysweet/agent-kgpacks-rs --json state,closed
```

- If a PR **is merged** and it closes the linked issue → this is the reconcile
  defect; continue to step 3.
- If **no** PR is merged → this is *not* the reconcile churn. The gate is
  correctly blocking a genuinely-unmerged goal (fail-closed behaviour is
  working); investigate why the goal was marked `completed` without a merge.

## 3. Check whether the `pr` wip_ref was pruned

The defect is prune-then-gate: wip-ref liveness Prong 2 drops the merged PR's
`pr` ref because a merged PR is not *open*.

```console
$ simard goal show <id> --json wip_refs
```

- **No `pr` ref, but an `issue` ref present** → classic signature. The gate's
  fast path finds no `pr` ref; recovery now runs via the issue fallback (step 4).
- **A `pr` ref still present** → not this defect. The gate reads the PR state
  directly; investigate `gh pr view` connectivity instead.

## 4. Confirm the issue-based fallback recovers the evidence

With the fix in place, when the `pr` ref is absent and an `issue` ref exists,
`GhCliEvidenceSource::any_pr_merged` asks GitHub whether the issue is closed by
a **merged** PR — scoped to the goal's own repo. Reproduce that query manually:

```console
$ gh api graphql -F owner=rysweet -F name=agent-kgpacks-rs -F number=18 -f query='
  query($owner:String!,$name:String!,$number:Int!){
    repository(owner:$owner,name:$name){
      issue(number:$number){
        closedByPullRequestsReferences(first:10, includeClosedPrs:true){
          nodes { number merged }
        }
      }
    }
  }'
```

- At least one node with `"merged": true` → the fallback returns `Ok(true)`, the
  gate certifies `Complete`, and the goal **archives** on the next cycle. The
  BLOCKED line stops because there is no longer a blocked goal to emit it for.
- Issue closed but **no** merged closing PR → the fallback returns `Ok(false)`;
  the goal stays blocked (completion legitimately requires a real merge).

> Note: `gh issue view --json` does **not** expose
> `closedByPullRequestsReferences.merged`. You must use `gh api graphql` (as
> above) to see the merged state of a closing PR — the same surface the gate
> uses.

## 5. Interpret a `CouldNotVerify` instead of `PrNotMerged`

If, after the fix, the goal blocks with **`CouldNotVerify`** rather than
`PrNotMerged`, the fallback hit a verification error and **failed closed** by
design (a `gh` outage, a rate-limit, or a malformed ref). This is correct
behaviour — the gate never archives on unverifiable evidence. Re-run the step-4
query directly:

- If it now succeeds → transient; the goal will recover next cycle.
- If it consistently errors → check `gh auth status`, `GH_TOKEN`, and GitHub
  rate limits. A malformed `issue`/`repo` ref (leading `-`, metacharacters) also
  maps to `CouldNotVerify` and is logged with the sanitized reason.

## 6. Emergency bypass (last resort)

If a gate defect is actively blocking delivery reporting and you need to restore
the legacy unguarded archive, use the kill-switch — then re-enable once fixed:

```console
$ SIMARD_COMPLETION_EVIDENCE=off simard daemon …
```

This bypasses the whole [completion-evidence gate](../reference/completion-evidence-gate-api.md#kill-switch),
not just the fallback. Use it only to recover from a genuine gate defect, never
as a standing configuration.

## See also

- [Completion-gate merged-PR reconciliation](../concepts/completion-gate-merged-pr-reconciliation.md) — why the churn happened.
- [Issue-fallback merged-PR recovery API](../reference/completion-gate-issue-fallback-api.md) — the resolution order and fail-closed mapping.
- [Diagnose a rejected goal completion](./diagnose-a-rejected-goal-completion.md) — for one-off (non-repeating) rejections.
- [wip-ref Liveness Reconcile API](../reference/wip-ref-liveness-reconcile-api.md) — the pruning prong.
