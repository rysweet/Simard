---
title: Operator update — "lbug lock contention mistaken for corruption" goal
description: Plain-language Signal/operator notification about the lock-contention goal being finished and no longer stuck.
last_updated: 2026-07-18
doc_type: operations
owner: simard
---

# Operator update — the "lock contention mistaken for corruption" goal

This is the plain-language message sent to the operator over Signal when the
goal **"stop lbug lock contention from being mistaken for catalog corruption"**
was triaged and course-corrected on 2026-07-18.

## The Signal message (as sent)

> Quick update on the memory-store goal about lock clashes being mistaken for
> corruption — good news, it's actually already fixed. The problem was that when
> two processes opened the memory store at the same time, the storage engine
> misread the clash as the file being corrupted and rebuilt it empty, which wiped
> the saved memory. That's now closed off: the second opener is made to wait
> briefly, and if it still can't get in it stops with a clear error instead of
> ever rebuilding and wiping anything. This shipped in a change that's already
> merged, with tests that reproduce the old wipe and prove it can't happen
> anymore. It only kept showing up on the "stuck" list because there was no
> automatic test tied to the goal to confirm it was done. I've now added that: a
> single command that re-runs those exact tests. From here on the system can
> confirm this goal on its own, and it'll flag it again automatically if the
> protection ever breaks. Nothing needed from you.

## What changed (for the record)

- The underlying fix already shipped in **merged PR #4317** ("serialize opens so
  lock-contention never wipes memory"). No behaviour change was needed here.
- Added `Specs/lbug-lock-contention-done-gate.md` — a short spec that spells out,
  in checkable terms, what "finished" means for this goal and lists the exact
  regression tests that prove it.
- Added `scripts/check-lbug-lock-contention-done-gate.sh` — one command that
  re-runs the open-serialization guard tests and the "two concurrent opens never
  wipe records" regression test. It exits successfully only while a contended
  open fails safely instead of wiping memory.
- This is a **standing** (regression-protection) goal, so the done-gate is kept
  live rather than the goal being closed: it stays green while the protection
  holds and turns red the moment it regresses.
