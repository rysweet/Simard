# Engineer Session Checkpoint & Resume — Done-Gate Specification

## Purpose

The goal **"engineers must support session checkpoint and resume"** (slug
`engineers-must-support-session-checkpoint-and-r-aad52503`) stayed `Blocked`
cycle after cycle with the same diagnosis: **no tracked PR/issue the done-gate
could verify** (why = `UNCLEAR-CRITERIA`). The blocker was **not** technical —
the feature already shipped in **merged PR #4311** ("resume interrupted sessions
from checkpoint (idempotent)"). The blocker was that the goal's finish condition
had **no machine-checkable definition**, so every cycle re-observed it as
unfinished and produced `NO ACTION`.

This spec fixes that WHY. Because the work is **already delivered**, the correct
course-correction is to **complete the goal**, and to make that completion
**machine-verifiable** so the done-gate can certify it instead of re-stalling.
It binds the goal's finish condition to a **single command a daemon can run and
score automatically**:

```
scripts/check-engineer-session-checkpoint-resume-done-gate.sh
```

The command exits `0` only when the delivered checkpoint-and-resume behaviour is
present and its shipped tests still pass; otherwise it exits non-zero and prints
the failing check. This turns "engineers must support session checkpoint and
resume" from a prose judgement into a check the done-gate can confirm, so the
goal is certified complete rather than left blocked.

## What the goal asked for

When an engineer is interrupted mid-work — a deploy binary-swap, a crash, or a
restart — the in-progress **session** (the goal it is advancing, its worktree,
and its accumulated phase state) must survive. Before PR #4311, engineers already
*checkpointed* their session at each phase boundary
(Intake / Preparation / Planning / Execution) but never *resumed*: a fresh
engineer process always restarted the goal from scratch, re-spawning the
expensive, non-idempotent agent session and risking a **duplicate PR**.

## What delivered it (merged PR #4311)

PR #4311 wired resume-on-startup into `run_local_engineer_loop`. A fresh process
loads the checkpoint for its objective, resumes the **same** session identity,
reuses every phase result already recorded, and re-runs only the phases after the
last completed one — so a completed agent session is **never re-spawned** (no
double-PR, no duplicate work).

| Layer | Location |
|-------|----------|
| Resume decision (`should_resume` — objective match + resumable phase) | [`src/engineer_loop/mod.rs`](../src/engineer_loop/mod.rs) |
| Resume wiring in the loop (`SessionCheckpoint::load` → resume same identity) | [`src/engineer_loop/mod.rs`](../src/engineer_loop/mod.rs) `run_local_engineer_loop` |
| Idempotency linchpin — reuse recorded `(action, verification)` | [`src/engineer_loop/types.rs`](../src/engineer_loop/types.rs) `SessionCheckpoint::is_resumable` / `resumable_execution` |
| Resume decision + end-to-end tests (agent reused, not re-run; idempotent) | [`src/engineer_loop/tests_resume.rs`](../src/engineer_loop/tests_resume.rs) |
| Reference doc | [`docs/reference/engineer-session-checkpoint-resume.md`](../docs/reference/engineer-session-checkpoint-resume.md) |

## Measurable done-criteria

The goal is DONE when every criterion below passes. Each is asserted by a test
shipped in merged PR #4311, re-run by
`scripts/check-engineer-session-checkpoint-resume-done-gate.sh`.

| ID | Criterion | Checked by |
|----|-----------|-----------|
| CR-1 | **resume-on-match** — a checkpoint whose objective matches and is at a resumable phase is resumed | `engineer_loop::tests_resume::should_resume_true_for_matching_objective_and_resumable_phase` |
| CR-2 | **no cross-goal contamination** — a checkpoint left by a different goal is ignored | `engineer_loop::tests_resume::should_resume_false_for_mismatched_objective` |
| CR-3 | **terminal phases are not resumed** — a checkpoint past Execution is not resumed | `engineer_loop::tests_resume::should_resume_false_for_terminal_phases` |
| CR-4 | **idempotency linchpin** — a recorded `(action, verification)` is reused only when Execution completed | `engineer_loop::tests_resume::resumable_execution_requires_recorded_action_and_verification` |
| CR-5 | **agent is reused, not re-run** — resuming from an Execution checkpoint skips the agent spawn | `engineer_loop::tests_resume::resume_from_execution_checkpoint_skips_agent_spawn` |
| CR-6 | **repeated dispatch is idempotent** — resuming twice produces no duplicate work / duplicate PR | `engineer_loop::tests_resume::resume_is_idempotent_across_repeated_dispatch` |

## Definition of "done" (the done-gate)

The goal is **done** when this single command exits `0`:

```
scripts/check-engineer-session-checkpoint-resume-done-gate.sh
```

It confirms the delivered resume seam still exists (`should_resume` in
`src/engineer_loop/mod.rs`, `resumable_execution` in `src/engineer_loop/types.rs`)
and re-asserts the CR-* criteria above via `cargo test`. This is the concrete
artifact the goal's done-criteria points at — the done-gate can run it and
certify the goal as complete now that checkpoint-and-resume has shipped.
Optionally, `--full` additionally confirms the reference doc asset is present.

## Progress log

- **2026-07-18** — The feature shipped in merged PR #4311; this spec binds the
  goal's finish condition to the machine-checkable tests it delivered via
  `scripts/check-engineer-session-checkpoint-resume-done-gate.sh`. The done-gate
  can now observe and certify the goal as **complete** instead of re-stalling on
  unmeasurable criteria.
