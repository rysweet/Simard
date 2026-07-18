---
title: Operator update — "stop the blocked-goal signal flood" goal
description: Plain-language Signal/operator notification about the blocked-goal signal-flood goal being finished and no longer stuck.
last_updated: 2026-07-18
doc_type: operations
owner: simard
---

# Operator update — the "stop the blocked-goal signal flood" goal

This is the plain-language message sent to the operator over Signal when the goal
**"stop the blocked-goal signal flood; make the Overseer course-correct before
escalating"** was triaged and course-corrected on 2026-07-18.

## The Signal messages (as sent, one per step)

> **Step 1 — what I found.** Quick update on the goal about the system spamming
> you whenever one of its own goals gets stuck. I looked at it and it's stuck for
> an ironic reason: the system can't automatically tell that this particular goal
> is finished, so it keeps re-checking it forever without ever shipping anything —
> exactly the kind of loop the goal was meant to stop.

> **Step 2 — the fix.** Good news: the actual work is already done. When a goal
> gets stuck, the system no longer pages you over and over — repeat alerts are now
> spaced further and further apart automatically, and before it ever bothers you
> it tries to untangle the problem itself and explains it in plain English instead
> of dumping raw error codes. The only thing missing was an automatic way to
> confirm this goal was finished.

> **Step 3 — done.** I've added that missing piece: a single command that re-runs
> the exact tests proving the alerts are throttled and the system fixes stuck
> goals itself before paging you. The system can now confirm this goal on its own
> and mark it finished — and it'll flag it again automatically if that protection
> ever breaks. Nothing needed from you.

## What changed (for the record)

- The anti-flood behaviour the goal asked for is **already delivered**:
  - a back-off cadence rail spaces out repeated blocked-goal escalations
    (`blocked_goal_gate: WhisperGate::with_backoff` in `src/overseer/mod.rs`);
  - a genuinely blocked goal is handed to the **agentic escalation-triage**
    recipe (`act_escalate_blocked_goal` →
    `prompt_assets/simard/overseer/escalation_triage.md`), which restates the
    block in plain English and repairs it before anyone is paged — this very
    triage is an instance of that behaviour;
  - the operator notification is **plain English**, never raw diagnostic markers.
  - (A separate merged change, `#4301`, further collapses all blocked goals into a
    single alert per tick, reinforcing the cadence rail on the deployed branch.)
- The correct course-correction was therefore to **make the finish condition
  machine-checkable** so the done-gate can certify the goal, not leave it blocked.
- Added `Specs/blocked-goal-signal-flood-done-gate.md` — a short spec that spells
  out, in checkable terms, what "finished" means for this goal (criteria
  `SF-1..SF-8`) and lists the exact tests that prove it.
- Added `scripts/check-blocked-goal-signal-flood-done-gate.sh` — one command that
  confirms the delivered cadence rail and agentic-triage seam still exist and
  re-runs their tests. It exits successfully only while the flood stays throttled
  and the Overseer course-corrects before escalating.
- Because the work is **already delivered**, the done-gate certifies the goal as
  **complete**: it stays green while the anti-flood protection holds and turns red
  the moment it regresses.

## Why it was stuck (in plain English)

Simard couldn't automatically tell that this goal was finished. Its finish line
had no test attached, so every time the system checked, it saw the goal as "not
confirmed done" and kept re-investigating without shipping anything — even though
the anti-flood work had already been built and merged. Tying the goal to a single
command that re-runs the proof tests lets the system certify it on its own.
