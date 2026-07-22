---
title: How the governed-repo roster goal was triaged and course-corrected before escalating
description: Worked record of the escalation-triage run that unblocked the recurring "move the governed-repo roster out of framework code and into Simard's identity" goal (id move-the-governed-repo-roster-out-of-framework-a8f57a50). The triage brain diagnosed an unmeasurable multi-part finish line plus a worker wedged on a stale worktree holding a cognitive-store lock, chose rewrite-done-gate, ratified the Governed-Repo Roster Charter, re-pointed the goal's done-criteria at the charter's single machine-checkable acceptance test, cleared the stale worktree/lock so a fresh worker can act, and sent one plain-English Signal update without paging a human.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../atlas/escalation-flow/README.md
  - ../../Specs/GOVERNED_REPO_ROSTER.md
  - ../concepts/identity-scoped-cognition.md
  - ../../prompt_assets/simard/overseer/escalation_triage.md
---

# How the governed-repo roster goal was triaged and course-corrected

> **Status: implemented.** This is the durable record of a completed
> escalation-triage run driven by
> [`prompt_assets/simard/overseer/escalation_triage.md`](../../prompt_assets/simard/overseer/escalation_triage.md).
> It documents the *finished* outcome for the recurring governed-repo roster
> goal so a future operator or OODA cycle that re-surfaces the goal gets the
> resolution instead of restarting the loop. It is the worked companion to the
> [Escalation-Triage Atlas](../atlas/escalation-flow/README.md) and follows the
> same pattern as the earlier coverage-goal triage run.

## The goal that kept restarting

| Field | Value |
|---|---|
| Goal id | `move-the-governed-repo-roster-out-of-framework-a8f57a50` |
| Symptom | Marked blocked; re-read the same open-ended description without shipping anything |
| Behaviour | The assigned worker wedged on a dead worktree that still held a memory-store lock |
| Decision | `rewrite-done-gate` |
| Human paged? | **No** — the block was fixable agentically |

In plain English: Simard could not automatically tell when this goal was
finished. Its finish line was a multi-part wish — "move the roster into the
identity, make it changeable at runtime, make it survive a deploy, and remove
the old code coupling" — with nothing she could check to certify completion. So
every cycle re-read the same description, found no observable finish condition,
and either restarted or got stuck. On the most recent attempt the worker got
wedged on a leftover worktree that still held a memory-store lock, and the
safeguard marked the goal blocked.

## What the triage decided

The triage brain rejected both the "just mark it complete" path and the "page a
human" path and chose to **rewrite the done-gate** instead. The reasoning:

- **Not `complete-delivered-goal`.** No merged PR delivers the roster move; the
  work has not shipped, so marking it complete would be an unverified claim.
- **Not `ask-operator-one-question`.** The intent is clear from the goal and the
  operator rationale; no human scope call is required. Asking a question would
  just re-park the goal.
- **`rewrite-done-gate` is grounded and low-risk.** The unmeasurable finish line
  can be replaced with one machine-checkable acceptance test the daemon can
  certify — a specific guard test module that stays green and a specific PR it
  can observe `MERGED`. The identity model needed to satisfy it already exists
  (`IdentityManifest.target_repos`, identity-scoped cognition), so the rewrite
  re-points the finish line at real, checkable conditions rather than inventing
  new scope.

## What was actually done

The triage performed a small, additive, CI-green course-correction — no Rust
source behavior change, no CI-gate change, no escalation-seam change.

### 1. Ratified the charter

A new machine-checkable charter,
[`Specs/GOVERNED_REPO_ROSTER.md`](../../Specs/GOVERNED_REPO_ROSTER.md), records
the disambiguation (§1), the single measurable acceptance test (§2), and the
deterministic next-target procedure (§3). Its `State` is `RATIFIED` — adopted as
the goal's done-gate via `rewrite-done-gate`.

### 2. Re-pointed the goal's done-criteria

The charter's `Governs goal slug` field absorbs the recurring slug
`move-the-governed-repo-roster-out-of-framework-a8f57a50`, so any future
resurfacing of the goal resolves to this charter's machine-checkable gate
instead of re-opening an open-ended planning cycle. The committed charter plus
this record *are* the re-point; no out-of-repo goal-store edit is claimed as the
evidence.

### 3. Cleared the wedge so a fresh worker can act

The stale engineer worktree that held the memory-store lock was removed and its
`cognitive.open.lock` files cleared, so a fresh worker can pick the goal up
against the clarified, checkable target instead of colliding with a dead lock.

## The machine-checkable finish line

A future cycle now certifies the goal against §2 of the charter instead of an
open-ended prose wish. The goal is **DONE** when a single PR is observed
`MERGED` that makes all four roster properties hold, each observable from a file
or command output, with the guard test module green:

```bash
cargo test -p simard governed_repo_roster
```

1. **Seeded from the identity** — a fresh identity's `target_repos` resolves the
   roster, with no read of a git-tracked framework file.
2. **Mutable at runtime** — Simard can add/remove a stewarded repo agentically
   and the change persists to identity-scoped state.
3. **Survives a self-deploy** — the install / prompt_assets re-install path does
   not overwrite the runtime roster.
4. **Exactly one source of truth** — `ecosystem_observe` and the referencing
   prose read the identity-curated roster; the old framework-file coupling is
   retired to a one-time seed.

## What the operator saw

The operator received a single plain-English Signal update — no marker tokens,
no jargon. In the shape of the triage output contract:

```json
{
  "problem": "Simard could not automatically tell when the 'move the repo roster into my identity' goal was finished, so it kept restarting without shipping anything — and the worker on it got stuck on a leftover workspace holding a lock.",
  "next_step": "I gave the goal a clear, checkable finish line and cleared the stuck workspace so a fresh attempt can run.",
  "root_cause": "The goal's finish line was a multi-part wish with nothing measurable, and the last attempt wedged on a stale worktree that still held a memory-store lock.",
  "decision": "rewrite-done-gate",
  "action_taken": "Wrote a charter with one machine-checkable finish line (the roster is seeded from my identity, changeable at runtime, and survives a redeploy), re-pointed the goal at it, and cleared the stuck workspace and its lock.",
  "escalate": null
}
```
