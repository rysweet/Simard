---
title: Completion-gate merged-PR reconciliation
description: Why a genuinely-completed goal re-blocked with "PR not merged" on every OODA cycle, and how the completion gate recovers merge evidence via an issue-based fallback after wip-ref liveness pruning strips the goal's `pr` ref. The status board and the merged-PR gate are reconciled so a goal reaches 'completed' only when its PR is actually merged, and never re-blocks once it is.
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./deploy-aware-done-gate.md
  - ./reconcile-and-self-deploy.md
  - ../reference/completion-gate-issue-fallback-api.md
  - ../reference/completion-evidence-gate-api.md
  - ../reference/wip-ref-liveness-reconcile-api.md
  - ../howto/diagnose-perpetual-completion-recuration.md
  - ../../src/goal_curation/completion_gate.rs
  - ../../src/ooda_loop/cycle.rs
---

# Completion-gate merged-PR reconciliation

The completion gate certifies a goal *done* only with hard evidence: a **merged
PR**, a **closed linked issue**, and — for changes to Simard's own running code
— a **verified deploy** (see [deploy-aware done-gate](./deploy-aware-done-gate.md)).
This note explains a systemic reconciliation defect between the goal board's
`completed` status and the gate's merged-PR check, and the additive fix that
resolves it.

## The symptom: perpetual re-block churn

Issue #12 surfaced a clear, repeating fault in the OODA journal. Nine goals —

- `simard-example-identity-gastronome-culinary-men-84186abe`
- `rysweet/agent-kgpacks-rs` issues **#12, #18, #19, #20, #21, #22, #23, #25**

— reported `STATUS=completed` in `simard goal list`, yet **every** OODA cycle
the completion curate step re-blocked each of them:

```text
[simard] OODA curate: completion BLOCKED for goal <id> — missing PR not merged
```

Over a 6-hour window that was **17 of 17 cycles × 9 goals = 153 identical
emissions**. The goal board said *completed*; the gate said *PR not merged*.
The two diverged, and the divergence never healed — pure churn that misreported
delivery status and never let the goals archive.

## What it was *not*

The requirement listed three candidate causes. Investigation discharged all
three:

- **Not false-completion.** The goals really were done; their PRs really were
  merged. The status was not set without work.
- **Not a `repo_slug` resolution bug.** Cross-repo routing to
  `rysweet/agent-kgpacks-rs` resolved correctly.
- **Not a stalled self-merge.** No PR was stuck awaiting merge.

## The real cause: prune-then-gate ordering

The defect is an **interaction between two individually-correct behaviours**,
evaluated in the wrong order within a single cycle:

1. **Pruning (wip-ref liveness Prong 2).** Each cycle,
   [`reconcile_merged_prs`](../reference/wip-ref-liveness-reconcile-api.md) drops
   any `pr` wip_ref whose PR is no longer *open*. A merged PR is not open, so
   its `pr` ref is correctly pruned — it is no longer live in-flight work.
2. **Gating.** Immediately after, the completion gate calls
   `GhCliEvidenceSource::any_pr_merged`. Under the old logic, *no* `pr` wip_ref
   meant `Ok(false)` with **no recovery path** — the gate could only read merge
   state from a `pr` ref that pruning had already removed.

```mermaid
flowchart LR
    A[Merged PR closes issue] --> B[Prong 2: prune `pr` wip_ref\n(PR not open ⇒ removed)]
    B --> C[Gate: any_pr_merged]
    C -->|old: None ⇒ Ok false| D[Blocked: PrNotMerged\nre-emitted every cycle]
    C -->|new: issue fallback| E[gh graphql: issue closed by merged PR?]
    E -->|merged: true| F[Complete ⇒ archives]
    E -->|error| G[CouldNotVerify\nfail-closed]
```

Pruning was right. The gate's old read was right in isolation. Their *ordering*
was the bug: pruning removed the only evidence the gate knew how to read, and
nothing recovered it.

## Why not just reorder?

Running the gate *before* pruning would regress the no-progress breaker's
kind-based liveness guard, which depends on `wip_refs` being liveness-reconciled
first — a **breaking** change to a separate invariant. So the fix is
**gate-internal recovery**, not reordering. The pruning prong keeps its
behaviour; the gate learns a second, independent way to see a merge.

## The fix: an issue-based merged-PR fallback

`any_pr_merged` gains a fallback that fires **only** when the `pr` wip_ref is
absent (so in-flight goals pay nothing):

- If a `pr` ref exists → read its state directly (**unchanged** fast path).
- Else, if an `issue` ref exists → ask `gh api graphql` whether the issue is
  **closed by a merged PR** (`closedByPullRequestsReferences { merged }`),
  scoped to the goal's **own** repo (cross-repo aware).
- Else → cheap `Ok(false)`.

Any verification error maps to `CouldNotVerify` (fail-closed) — a transient
GitHub outage blocks the goal for one cycle rather than misreporting it. Only an
authoritative `merged: true` yields `Ok(true)`. See the
[issue-fallback API reference](../reference/completion-gate-issue-fallback-api.md)
for the exact resolution order, validation, and observability.

## The reconciliation invariant

After the fix, the board and the gate agree:

> A goal reaches (and stays) `completed`/archived **only** when its PR is
> actually merged — verified either from a live `pr` wip_ref *or*, once that ref
> is pruned, from the merged PR that closes its linked issue.

Concretely, for the 9 goals in issue #12: once the gate recovers the merged-PR
evidence via the issue fallback, they pass the gate, **archive**, and leave the
active board. The BLOCKED line stops firing — not because the message was
silenced, but because there is no longer a blocked, un-archivable goal to emit
it for. The 153-emission churn ends.

## Non-breaking by construction

The fallback lives in the production `GhCliEvidenceSource` impl. The
`EvidenceSource` trait signature is unchanged, so the blanket `&T` impl and
every `FakeEvidence` test double compile as before. The change is additive,
observable via structured tracing/OTel, and touches no existing public
semantics.

## See also

- [Issue-fallback merged-PR recovery API](../reference/completion-gate-issue-fallback-api.md) — the API specification.
- [Deploy-aware done-gate](./deploy-aware-done-gate.md) — the three-clause completion contract.
- [wip-ref Liveness Reconcile API](../reference/wip-ref-liveness-reconcile-api.md) — the pruning prong.
- [How to diagnose perpetual completion re-curation](../howto/diagnose-perpetual-completion-recuration.md) — the operator playbook.
