---
title: Engineer session checkpoint & resume
description: How an engineer periodically checkpoints its in-progress session and how a fresh engineer process idempotently RESUMES that session after a crash, restart, or deploy binary-swap instead of re-running the goal from scratch.
---

# Engineer session checkpoint & resume

When an engineer is interrupted mid-work — a deploy binary-swap, a crash, or a
restart — the in-progress **session** (the goal it is advancing, its worktree,
and its accumulated phase state) must not be thrown away. This is distinct from
**goal-requeue** in the deploy-drain path: requeue keeps the *goal* safe so it
is not lost; checkpoint & resume keeps the in-progress *session* safe so no
work already done is repeated.

## What is checkpointed

`run_local_engineer_loop` (the repo-grounded `engineer-loop-run` surface behind
`simard operator probe … engineer-loop-run`) writes a
[`SessionCheckpoint`](../../src/engineer_loop/types.rs) to
`<state_root>/session_checkpoint.json` after each major phase boundary:

| Completed phase | Checkpoint additionally records |
| --------------- | ------------------------------- |
| `Intake`        | workspace inspection |
| `Preparation`   | loaded memory / handoff context |
| `Planning`      | the formed execution plan |
| `Execution`     | the executed agent action **and** its verification |

Each checkpoint carries the original session identity, the objective it is
advancing, the completed phase, and the phase traces so far. Writing is
**best-effort**: a failed write is logged and the loop continues, and a
checkpoint is never allowed to corrupt goal state.

Because the checkpoint is refreshed at every phase boundary, it doubles as the
graceful-quiesce checkpoint: the deploy-drain path marks `draining.flag` and
waits for in-flight engineers (it never kills a *producing* engineer), so an
engineer that is quiesced always has a current phase checkpoint on disk.

## How resume works

On startup / dispatch, `run_local_engineer_loop` calls
`SessionCheckpoint::load` and decides whether to resume with `should_resume`:

- The checkpoint's **objective must match** the objective being dispatched — a
  checkpoint left by a *different* goal is ignored (and is overwritten by this
  run's own Intake checkpoint). This prevents cross-goal contamination.
- The checkpoint must be at a resumable phase (`Intake`..=`Execution`).

When resuming, the loop:

1. Restores the **same** `SessionRecord` identity (it is the same session, not a
   new one) and hydrates the recorded phase traces, prepending an auditable
   `resume` phase trace naming the phase it picked up from.
2. **Skips** every phase whose result was already recorded, reusing the recorded
   inspection, memory context, execution plan, action, and verification.
3. Runs only the remaining phases (Reflection → Summarize → Persistence) to
   finish the session, then clears the checkpoint on success.

## Idempotency: no double-PR, no duplicate work

The critical guarantee is that a **completed agent session is never spawned
again**. `SessionCheckpoint::resumable_execution` returns the recorded
`(action, verification)` only when the `Execution` phase completed *and* both
artifacts were persisted. When it returns a value, resume reuses that result and
does not re-spawn the agent — so a resumed session cannot open a duplicate pull
request or redo expensive, non-idempotent work.

If a process died *before* the Execution checkpoint (e.g. between the Planning
and Execution checkpoints), no completed action exists, so re-running the agent
is the correct behaviour — the prior attempt produced nothing to duplicate.

Resume is safe to run repeatedly: once a resumed session completes it clears the
checkpoint, so a subsequent dispatch starts fresh.

## Observing a resume

The `resume` phase trace is surfaced by the engineer-loop probe's phase-trace
listing, so an operator running `engineer-loop-run` after an interruption can
see exactly which phase the fresh process resumed from and confirm the agent was
reused rather than re-run.

## Related

- [Concurrent engineer dispatch](./concurrent-engineer-dispatch.md)
- [Engineer worktree isolation](./engineer-worktree-isolation.md)
- [Safe self-update](../safe-self-update.md) — the deploy-drain graceful-quiesce
  path this feature complements.
