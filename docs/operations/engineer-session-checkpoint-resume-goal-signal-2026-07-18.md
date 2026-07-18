---
title: Operator update — "engineers support session checkpoint and resume" goal
description: Plain-language Signal/operator notification about the session checkpoint-and-resume goal being finished and no longer stuck.
last_updated: 2026-07-18
doc_type: operations
owner: simard
---

# Operator update — the "engineers support session checkpoint and resume" goal

This is the plain-language message sent to the operator over Signal when the
goal **"engineers must support session checkpoint and resume"** was triaged and
course-corrected on 2026-07-18.

## The Signal message (as sent)

> Quick update on the goal about engineers picking their work back up after an
> interruption — good news, it's already done. When one of the automated
> engineers gets interrupted partway through a task (a crash, a restart, or a
> software update swapping it out), it used to start the whole task over from the
> beginning. That was slow and, worse, it could accidentally open a second copy
> of the same pull request. That's now fixed: an interrupted engineer saves its
> progress along the way, and a fresh one picks up exactly where the last one
> left off instead of redoing finished work — so it never opens a duplicate. This
> shipped in a change that's already merged, with tests that prove the finished
> work is reused rather than repeated. It only kept showing up on the "stuck"
> list because there was no automatic test tied to the goal to confirm it was
> done. I've now added that: a single command that re-runs those exact tests. The
> system can now confirm this goal on its own and mark it finished, and it'll flag
> it again automatically if the protection ever breaks. Nothing needed from you.

## What changed (for the record)

- The underlying feature already shipped in **merged PR #4311** ("resume
  interrupted sessions from checkpoint (idempotent)"). No behaviour change was
  needed here — the correct fix was to **mark the goal complete** rather than
  leave it blocked.
- Added `Specs/engineer-session-checkpoint-resume-done-gate.md` — a short spec
  that spells out, in checkable terms, what "finished" means for this goal and
  lists the exact tests that prove it.
- Added `scripts/check-engineer-session-checkpoint-resume-done-gate.sh` — one
  command that confirms the delivered resume seam still exists and re-runs the
  checkpoint-and-resume tests. It exits successfully only while an interrupted
  session resumes correctly and a completed agent session is never re-run.
- Because the work is **already delivered**, the done-gate certifies the goal as
  **complete**: it stays green while the resume protection holds and turns red
  the moment it regresses.

## Why it was stuck (in plain English)

Simard couldn't automatically tell that this goal was finished. Its finish line
had no test attached, so every time the system checked, it saw the goal as
"not confirmed done" and kept re-investigating without shipping anything — even
though the work had already been built and merged. Tying the goal to a single
command that re-runs the proof tests lets the system certify it on its own.
