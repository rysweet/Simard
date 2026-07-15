---
title: The terminal no-progress stall never parks a goal with empty evidence
description: Why the bottom rung of the no-progress ladder no longer stamps a stalled goal with a generic, evidence-free `why=GENUINELY-STUCK evidence=[(none)]` (which hit 12–13 of 20 live goals on 2026-07-15). The terminal rung reuses the existing guided-engineer (independent recipe-runner) investigation on the first stall, and on the terminal rung either escalates WITH concrete evidence or — when the investigation produced none — surfaces a fail-visible investigation gap instead of a bare block. Evidence-less already-blocked goals are folded into the re-investigation population, and a per-signature inflight guard prevents duplicate concurrent overseer investigations.
last_updated: 2026-07-15
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./no-progress-root-cause-resolution.md
  - ./steerable-ooda-daemon.md
  - ../reference/no-progress-root-cause-resolution-api.md
  - ../howto/diagnose-a-no-progress-block.md
  - ../howto/unblock-stuck-ooda-goals.md
---

# The terminal no-progress stall never parks a goal with empty evidence

> **Status: implemented (issue #16).** The terminal-rung guard and the surfaced
> investigation gap live in `src/goal_curation/no_progress_breaker.rs`
> (`resolution_for_why`, `NoProgressResolution::SurfaceInvestigationFailure`, and
> the `needs_reinvestigation` / `is_evidenceless_no_progress_block` population
> predicates). The side effects (surface, un-block, retry) live in the
> curate-phase adapter `src/ooda_loop/no_progress.rs`
> (`apply_resolution_side_effects`, `reinvestigate_bare_blocked_goals`). The
> overseer inflight dedup guard lives in `src/overseer/mod.rs`
> (`inflight_investigations`, `recipe_dedup_key`,
> `reconcile_inflight_investigations`).

## The defect

The [root-cause ladder](./no-progress-root-cause-resolution.md) classifies a
stalled goal and self-resolves the machine-fixable causes. Its bottom rung —
`UNCLEAR-CRITERIA` / `GENUINELY-STUCK` — spawns **one** guided engineer (an
independent recipe-runner investigation) on the first stall, then escalates to a
human on the second. The escalation embedded the classification's evidence.

But the deterministic reasoner's `stuck_evidence(goal)` returns only the goal's
still-open tracked issues/PRs. A goal that never produced a tracked artifact —
the six `simard-identity-*` goals, the coverage/coin/parity goals — has **empty**
evidence, so the terminal escalation rendered:

```text
🔒 [OODA-SAFEGUARD] OODA goal made no shippable progress for 3 consecutive
no-action cycles; why=GENUINELY-STUCK evidence=[(none)]
```

A generic, non-actionable stamp. On the live daemon (2026-07-15) **12–13 of 20**
active goals were parked this way. The contrast is one goal (issue-17/WS2) that
carried a rich, specific diagnosis: a concrete WHY backed by evidence. That is
the bar every blocked goal must meet.

## The rule: never `evidence=[(none)]`

A goal is **never** parked with empty evidence. The terminal rung now has three
honest outcomes, never a bare stamp:

1. **Concrete next action.** The first stall dispatches the independent
   recipe-runner investigation via the existing guided-engineer spawn
   (`SpawnEngineer` → `dispatch_spawn_engineer`, the same seam the Act phase
   uses — no parallel path). Its semantic result flows back as an ordinary
   `ActionOutcome`; a real advance resets the no-action counter next cycle.

2. **Evidence-backed blocker.** If the terminal rung is reached and the
   investigation produced concrete evidence, the goal is escalated WITH that WHY
   + evidence attached (the issue-17 quality bar) — a real diagnosis, never
   `(none)`.

3. **Surfaced investigation gap.** If the terminal rung is reached and the
   investigation produced **no** evidence, that is itself a failure, not a
   silent generic block. `resolution_for_why` returns
   `NoProgressResolution::SurfaceInvestigationFailure`: the adapter records the
   goal in `NoProgressBreakerReport::investigation_errors` (fail visible), takes
   **no** terminal action, and leaves the goal retriable (fail closed) so the
   next investigation can recover real evidence. A surfaced gap is **not** a
   firing.

No wall-clock timeout kills the investigation; the recipe-runner's own
idle/liveness handling governs it.

## The stranded already-blocked population

The [re-investigation pass](./no-progress-root-cause-resolution.md#re-investigating-already-blocked-goals-issue-17)
originally re-examined only **bare** blocks (`[OODA-SAFEGUARD] … needs human
review`, no class token). An `evidence=[(none)]` block *carries* a class token,
so it was **not** bare and the pass skipped it — which is exactly why the ~12–13
stranded goals were never re-examined.

`needs_reinvestigation(reason)` now selects **both** populations: legacy bare
blocks (`is_bare_no_progress_block`) and evidence-less `(none)` blocks
(`is_evidenceless_no_progress_block`). A `(none)` block is re-investigated and
driven away from `(none)` — to a concrete WHY, a spawned fixer, or a surfaced
investigation gap — and the WHY-rewrite is its own idempotency guarantee (once
it carries real evidence it no longer matches, so the pass never re-processes
it).

## No duplicate concurrent investigations

The live daemon also ran **two** recipe-runner processes (PIDs 1074394 and
1095553) investigating the identical `overseer-obs:goal:blocked:…` signature at
once: a recurring signature re-observed each cycle re-launched a fresh recipe
while the prior one was still running (`sequence_group` is `None` for these, so
the conflict sequencer never dedups them).

The overseer now holds an in-flight dedup registry
(`inflight_investigations: HashMap<signature, WorkstreamHandle>`), keyed by
`recipe_dedup_key` (the `overseer-obs:…` token in the launch's task
description). A `LaunchRecipe` whose signature is already in flight is **held**
in `gate`; a successful launch registers the signature in `act`. The registry
self-reconciles at the top of `run_cycle`
(`reconcile_inflight_investigations`): a workstream that `poll` reports is no
longer `Running` frees its slot, so the guard is "at most one **in flight**",
never a permanent one-shot. A poll error leaves the entry in place (fail closed
— better to skip a duplicate than double-launch on a transient error). A
*different* signature is unaffected.
