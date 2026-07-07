---
title: How to diagnose a deferred or serialized engineer spawn
description: >
  Operator runbook for "coverage planned this goal, so why didn't an engineer
  start?" — read the engineer-admission decision, tell a real overlap from a
  false one, resolve a deferred/serialized spawn, confirm the exact-path rail,
  and (recovery only) disable the admission gate.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/dependency-overlap-aware-scheduling.md
  - ../reference/engineer-admission-api.md
  - ../reference/ooda-engineer-admission-recipe.md
  - ../reference/concurrent-engineer-dispatch.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ../operations/engineer-admission-kill-switch.md
---

# How to diagnose a deferred or serialized engineer spawn

> **Status: implemented.** This runbook describes the shipped
> behaviour in present tense. The
> engineer-admission gate (`decide_engineer_admission`, the overlap module, the
> `engineer_admission_decision` metric, and the `SIMARD_ENGINEER_ADMISSION`
> kill-switch) it references lives in
> [`src/ooda_actions/advance_goal/admission.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/admission.rs)
> (wired into `dispatch_spawn_engineer` in
> [`src/ooda_actions/advance_goal/spawn.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs))
> — see [dependency/overlap-aware engineer scheduling](../concepts/dependency-overlap-aware-scheduling.md).

Coverage planned an `AdvanceGoal` for a goal and the AIMD cap had room, but **no
engineer started** this cycle — or one started with a "rebase onto goal X" note.
This is usually the admission gate doing its job: a live engineer on a
**different** goal is touching the **same files**, so Simard **deferred**
(retry next cycle) or **serialized** (spawn + rebase hint) instead of starting a
second engineer that would collide at merge.

If your goal isn't starting because it is `Blocked`, at the AIMD cap, or already
assigned, that is a different path — see
[how OODA spawns engineer agents](../howto/spawn-engineers-from-ooda-daemon.md)
and [concurrent engineer dispatch](../reference/concurrent-engineer-dispatch.md).
This page is for the case where admission **deferred/serialized** a genuinely
new, unassigned spawn.

## 1. Read the admission decision

Every admission emits its reasoning. Read the most recent decision for the goal
from the metrics stream (drop `--user` for a system-level install):

```bash
simard metrics query --name engineer_admission_decision | tail -n 5
```

Each entry's `context` carries the **decision**, the **overlapping goal(s)**, and
the brain's **rationale**, for example:

```json
{"metric_name":"engineer_admission_decision","value":0,
 "context":"decision=defer blocked_by=[render-goals-status] — live engineer render-goals-status is rewriting src/operator_commands_ooda/goals_status.rs, the only file this goal edits; parallel PRs would collide (cf. #2698/#2696)"}
```

You can also read it from the cycle report, where the `EngineerAdmission` brain
phase is recorded alongside Act/Decide/Orient:

```bash
cat ~/.simard/cycle_reports/cycle_*.json \
  | jq '.brain_judgments[] | select(.phase=="engineer_admission")' | tail
```

A judgment with `"fallback": true` means the decision came from a **rail**, not
the brain: either the **exact-path rail** (a certain collision, deferred
deterministically) or the **fail-open rail** (the brain errored, so the gate
admitted anyway). The rationale string says which.

## 2. Interpret the decision

| Decision | What it means | What happens next |
| --- | --- | --- |
| `admit` | No blocking overlap — the work is independent. | Engineer spawned normally. |
| `defer` | A live engineer holds files this goal needs. | **No spawn this cycle.** Re-evaluated next OODA round — a natural retry once the other engineer's PR lands. No failure counted. |
| `serialize_after` | Overlap exists but is workable if rebased. | Engineer **spawned**, with a "rebase onto goal `<after_goal_id>` before editing `<overlap_files>`" hint appended to its task. |

> **`defer` is backpressure, not a failure.** A deferred spawn does **not**
> increment `goal_failure_counts`, does **not** mark the goal `Blocked`, and does
> **not** file an issue. It simply waits for the next cycle. If you see a goal
> deferring cycle after cycle, the blocker is a long-running engineer on the
> overlapping goal — that is the signal to look at, not the deferral itself.

## 3. Confirm the overlap is real

The admission ctx records exactly which files drove the decision. Find the live
engineer named in `blocked_by` / `after_goal_id` and confirm it is really
touching those files:

```bash
# List the live engineer worktrees the daemon sees.
ls -1 ~/.simard/engineer-worktrees/

# For the blocking engineer's worktree, what is it actually changing?
git -C ~/.simard/engineer-worktrees/<blocking-goal-dir> diff --name-only \
  "$(git -C ~/.simard/engineer-worktrees/<blocking-goal-dir> merge-base HEAD origin/main)"...HEAD
git -C ~/.simard/engineer-worktrees/<blocking-goal-dir> diff --name-only  # working tree
```

Compare that set against the candidate goal's predicted scope in the rationale.
If they truly intersect on a shared module (the `goals_status.rs` class of
collision), the deferral/serialization is correct — let it ride.

## 4. Resolve it

- **The overlap is real and the blocker is progressing.** Do nothing. When the
  blocking engineer's PR merges, its worktree claim clears, the overlap
  disappears, and the deferred goal is admitted on a subsequent cycle. This is the
  intended serialization.
- **The overlap is real but the blocker is wedged.** The *blocker* is the problem,
  not the deferral. Diagnose the stuck engineer via the
  [engineer-lifecycle runbook](../howto/diagnose-and-recover-ooda-step-failures.md)
  (reclaim/redispatch it); once it clears, the deferred goal proceeds.
- **`serialize_after` fired — verify the rebase hint landed.** The spawned
  engineer's task should contain the "rebase onto goal `<after_goal_id>`" line.
  Confirm from the agent log:

  ```bash
  simard agent logs <engineer-session-id> | grep -i 'rebase onto goal'
  ```

- **The overlap is FALSE (predicted scope was wrong).** Predicted scope is a
  best-effort heuristic from `wip_refs` + prior PRs. If a goal is deferring on a
  file it will not actually touch, the fix is to improve the *signal*, not to
  loosen the gate: give the goal a concrete `wip_ref`/branch so its real footprint
  is visible, or let the first cycle's engineer establish the footprint. Do **not**
  reach for the kill-switch for a single mis-prediction — the gate is fail-open, so
  a wrong `defer` costs only one cycle of latency and self-corrects when the
  overlapping engineer finishes.

## 5. Confirm the exact-path rail (sanity check)

The one **hard** guarantee is that a *certain* collision is blocked regardless of
the brain: if the candidate's exact target paths are already held by a single live
engineer, the spawn is deterministically deferred. To confirm the rail is active,
a candidate whose predicted scope is a **subset** of a live engineer's changed
files must **not** spawn — even if the (stub) brain says `admit`. This is covered
by a regression test (T5 in the
[test matrix](../reference/engineer-admission-api.md#test-matrix)).

## 6. Override the gate (recovery only)

If the admission gate itself is defective and is wrongly deferring genuinely
independent work, you can disable scheduling:

```bash
SIMARD_ENGINEER_ADMISSION=off simard daemon
```

With the gate off, `dispatch_spawn_engineer` skips gather/reason/rails and admits
every candidate — the pre-#2690 collision-blind behaviour. Because the gate is
already **fail-open**, this is an incident lever, not a routine one: a broken
scheduler already degrades to admitting, so you rarely need this. Use it only to
rule the gate out during an investigation, and re-enable (unset the variable)
immediately afterward. The degradation is audited at boot. See the
[engineer-admission kill-switch page](../operations/engineer-admission-kill-switch.md).

## See also

- [Dependency/overlap-aware engineer scheduling (concept)](../concepts/dependency-overlap-aware-scheduling.md)
- [Engineer-admission API reference](../reference/engineer-admission-api.md)
- [OODA engineer-admission recipe & prompt schema](../reference/ooda-engineer-admission-recipe.md)
- [Concurrent engineer dispatch](../reference/concurrent-engineer-dispatch.md) — the per-round dispatcher this gate guards.
- [How OODA spawns engineer agents](../howto/spawn-engineers-from-ooda-daemon.md) — the spawn path the gate sits in front of.
- [Engineer-admission kill-switch](../operations/engineer-admission-kill-switch.md)
