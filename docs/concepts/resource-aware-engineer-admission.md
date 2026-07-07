---
title: Resource-aware engineer admission — agentic admission under the count cap
description: Why Simard weighs disk, build-cache size, and system load — not just engineer count — before spawning another engineer, and how a structured-reasoning brain step decides ADMIT / DEFER / RECLAIM-FIRST each cycle.
last_updated: 2026-07-07
owner: simard
doc_type: concept
related:
  - ../reference/resource-aware-admission-api.md
  - ../reference/ooda-resource-admission-recipe.md
  - ../howto/configure-resource-aware-admission.md
  - ./adaptive-scaling.md
  - ./automated-disk-health.md
  - ../reference/concurrent-engineer-dispatch.md
---

# Resource-aware engineer admission — agentic admission under the count cap

## The incident: count-control is not resource-admission

The [AIMD adaptive scaler](adaptive-scaling.md) bounds how many actions the OODA
cycle dispatches concurrently, and the [concurrent-engineer
dispatcher](../reference/concurrent-engineer-dispatch.md) holds a semaphore that
caps how many engineers run at once. Both are **count controls**: they answer
the question *"how many?"* — never *"can this host afford one more?"*

On a busy self-improvement host, that gap bit hard. The scaler happily kept the
engineer count under its ceiling while **40+ engineer worktrees** accumulated,
each with its own `cargo` build cache. Parallel builds piled up, disk climbed to
**91%**, and the next allocation tripped `ENOSPC` (No space left on device) —
which kills recipes mid-flight and corrupts in-progress work.

The AIMD scaler did nothing wrong. It was never told about disk. A count cap
cannot see a full disk, a thrashing load average, or a build cache that has
quietly grown to tens of gigabytes. **Bounding the count is necessary but not
sufficient. Admission also has to weigh resources.**

## The idea: agentic admission, not another threshold pile

The naive fix is a wall of hardcoded `if disk > X && load > Y && cache > Z`
heuristics in Rust. Simard deliberately does **not** do that. Thresholds rot,
interact badly, and never capture the judgment call ("disk is high but the cache
is reclaimable and load is low, so reclaim then retry").

Instead, admission follows the same pattern already proven in the [engineer
lifecycle brain](../reference/ooda-engineer-lifecycle-recipe.md): **the
intelligence lives in a structured-reasoning prompt that the brain executes
repeatedly**, and only a thin deterministic rail guards the one irreversible,
safety-critical outcome.

Before admitting a **fresh** engineer, the daemon:

1. **Gathers a resource picture** — disk usage %, worktree/build-cache size,
   1-minute load average, CPU count, and in-flight engineer count.
2. **Asks the admission brain to reason** over that picture and choose one of
   three outcomes.
3. **Applies the decision**, subject to one hard rail.

```
                         fresh-engineer spawn request
                                    │
                     (already under the AIMD count cap)
                                    │
                    ┌───────────────▼────────────────┐
                    │ gather_resource_admission_ctx() │  best-effort probes
                    │  disk% · cache bytes · load ·   │  (any field → None on error)
                    │  cpu count · in-flight count    │
                    └───────────────┬────────────────┘
                                    │
                    ┌───────────────▼────────────────┐
                    │  admission brain reasons        │  ← intelligence (prompt)
                    │  ADMIT / DEFER / RECLAIM-FIRST  │
                    └───────────────┬────────────────┘
                                    │
                    ┌───────────────▼────────────────┐
                    │  HARD RAIL (thin, deterministic)│  ← safety (code)
                    │  disk known AND ≥ ceiling →     │
                    │  force DEFER regardless         │
                    └───────────────┬────────────────┘
                                    │
              ┌─────────────────────┼─────────────────────┐
              ▼                     ▼                     ▼
           ADMIT                 DEFER              RECLAIM-FIRST
     allocate worktree,     benign skip: no       run disk-health
       spawn engineer       worktree, no          reclaim recipe,
                            failure-count bump      then DEFER
```

## The three outcomes

| Outcome | Meaning | Effect |
|---|---|---|
| **ADMIT** | Resources are healthy enough to add an engineer. | Continue to worktree allocation and spawn — the normal path. |
| **DEFER** | Resources are tight; adding an engineer now is unwise. | **Benign skip**: no worktree allocated, the goal is retried next cycle, and the goal's failure counter is **not** incremented. |
| **RECLAIM-FIRST** | Disk pressure is reclaimable (stale worktrees, build caches). | Invoke the existing [disk-health reclaim recipe](automated-disk-health.md), then DEFER this cycle and re-evaluate next cycle. |

DEFER is the critical design point. A resource defer is **not a failure** — no
progress was attempted and none was lost. It must never look like a stuck goal.
See ["Why DEFER is a benign skip"](#why-defer-is-a-benign-skip) below.

## Repeated structured thought

Admission is not a one-time gate. The brain reasons **at every fresh-engineer
admission**, so the decision tracks the live resource picture as it changes cycle
to cycle. This is the same "repeated execution of structured thought" model the
rest of the OODA brain uses: gather structured context → reason → apply → observe
→ repeat. Nothing is memoized; each admission is judged on current conditions.

## The one hard rail: an ENOSPC floor the brain cannot override

Agentic reasoning is the right tool for the *judgment* ("is now a good time?"),
but an irreversible `ENOSPC` crash is not a judgment call — it is a floor.

So exactly one deterministic rail sits below the brain: a configurable **disk
admission ceiling** (`SIMARD_DISK_ADMISSION_CEILING_PCT`, default **90%**). If
the disk usage is successfully read **and** is at or above the ceiling, admission
is refused (downgraded to DEFER) **regardless of what the brain decided**. An
`ADMIT` from the model can only ever be made *more* conservative by the rail,
never less. The model cannot talk the daemon into filling the disk.

This is the *only* hardcoded threshold in the whole feature. Everything else is
prompt-driven.

### Fail-open on unknown

The rail engages **only** on a *successful, over-ceiling* disk read. If the disk
probe fails (transient `df` error, unusual platform), disk usage degrades to
`None` — "unknown" — and the rail does **not** fire. The unknown is instead
passed to the brain, which reasons about it. A spurious probe failure must never
be able to deadlock all spawning; the layered protection (this rail plus the
[≥95% emergency cleanup tier](automated-disk-health.md)) means unknown-disk is
safe to let through to reasoning.

The invariant, stated precisely:

> **known-and-over-ceiling ⇒ DEFER. unknown ⇒ reasoner. Never known-over-ceiling ⇒ ADMIT. Never deadlock on unknown.**

## Why DEFER is a benign skip

The OODA cycle bumps a goal's failure counter whenever an action outcome reports
`success = false`, and three strikes blocks the goal. A resource defer must sit
entirely outside that machinery:

- DEFER returns a **success outcome** (`success = true`) with
  `detail = "deferred: resource pressure"`.
- No worktree is allocated (so a deferred cycle cannot itself grow disk).
- The goal's `goal_failure_counts` entry is untouched.
- The goal is simply retried on the next cycle.

Deferring is neither progress nor failure — it is a no-op skip driven by host
conditions, not by anything wrong with the goal. Conflating it with failure
would let a temporarily-full disk permanently block healthy goals, which is
exactly the kind of silent degradation Simard forbids.

## What this feature deliberately does **not** change

- **The AIMD scaler is untouched.** Resource admission is *additive*: it runs
  underneath the count cap, only for fresh-engineer spawns. If the count cap
  already said no, admission never runs.
- **No new reclaim logic.** RECLAIM-FIRST reuses the existing
  [disk-health](automated-disk-health.md) reclaim recipe. There is one reclaim
  implementation, not two.
- **Live re-attach is exempt.** Admission gates only the *fresh spawn* path.
  Re-attaching to an already-running engineer allocates no new resources, so it
  is never gated.

## Where it lives in the loop

The gate sits inside `dispatch_spawn_engineer`
(`src/ooda_actions/advance_goal/spawn.rs`), in the fresh-spawn path: after the
live-engineer lifecycle branch, after the subordinate-depth recursion guard,
under the AIMD count cap, and **before** the engineer worktree is allocated.
Placing it before allocation is what guarantees a deferred cycle grows nothing.

Both dispatch call sites — the concurrent dispatcher (under its AIMD permit) and
the direct `advance_goal` path — inherit the gate automatically because it lives
inside the shared dispatch function.

## See also

- [Resource-aware admission API reference](../reference/resource-aware-admission-api.md)
  — the `OodaAdmissionBrain` trait, `ResourceAdmissionCtx`, `AdmissionDecision`,
  the pure gate, and the hard rail.
- [OODA resource-admission recipe & prompt schema](../reference/ooda-resource-admission-recipe.md)
  — where the intelligence lives.
- [Configure resource-aware admission (how-to)](../howto/configure-resource-aware-admission.md)
  — tuning the ceiling, reading decisions, tutorial.
- [Adaptive scaling](adaptive-scaling.md) — the count control this layers on top of.
- [Automated disk-health management](automated-disk-health.md) — the reclaim path RECLAIM-FIRST invokes.
