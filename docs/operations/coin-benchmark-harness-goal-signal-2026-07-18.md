---
title: Operator update — "build a local COIN benchmark harness" goal
description: Plain-language Signal/operator notification about the local COIN benchmark harness goal being finished and no longer stuck.
last_updated: 2026-07-18
doc_type: operations
owner: simard
---

# Operator update — the "local COIN benchmark harness" goal

This is the plain-language message sent to the operator over Signal when the
goal **"build a local COIN benchmark harness and a self-improvement loop"** was
triaged and course-corrected on 2026-07-18.

## The Signal message (as sent)

> Quick update on the goal about building a local practice range for scoring how
> well our coding agents do — good news, it's already done. The tool is built and
> merged: it runs the benchmark locally, scores the result the same way the public
> leaderboard does, and even has a built-in self-check that grades itself and a
> practice loop that tries new tactics and keeps only the ones that genuinely help
> (throwing away the ones that only looked good by luck). It kept showing up on the
> "stuck" list for one boring reason: there was no automatic test tied to the goal
> to confirm it was finished, so the system kept re-checking it forever without
> ever ticking it off. I've now added that missing piece — a single command that
> re-runs the tool's own tests and confirms all its parts are in place. It passes
> today (119 tests, all green). The system can now confirm this goal on its own and
> mark it finished, and it'll flag it again automatically if anything ever breaks.
> Nothing needed from you.

## What changed (for the record)

- The harness itself already shipped on `main` under `src/coin_gym/`, and its
  measurable self-check landed in **merged PR #4171** (`coin-gym verify`). No
  behaviour change was needed here — the correct fix was to give the goal a
  finish condition the system can check, then certify it as complete.
- Added `Specs/coin-benchmark-harness-done-gate.md` — a short spec that spells
  out, in checkable terms, what "finished" means for this goal and lists the
  exact evidence that proves it.
- Added `scripts/check-coin-benchmark-harness-done-gate.sh` — one command that
  confirms the harness, its acceptance self-check, and its self-improvement loop
  still exist on `main` and re-runs the harness's 119-test suite. It exits
  successfully only while all three are present and green.
- Because the work is **already delivered**, the done-gate certifies the goal as
  **complete**: it stays green while the harness holds and turns red the moment
  it regresses.

## Why it was stuck (in plain English)

Simard couldn't automatically tell that this goal was finished. Its finish line
had no test attached, so every time the system checked, it saw the goal as
"not confirmed done" and kept re-investigating without shipping anything — even
though the tool had already been built and merged. Tying the goal to a single
command that re-runs the tool's own proof tests lets the system certify it on its
own.
