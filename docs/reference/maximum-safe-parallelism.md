---
title: Maximum safe parallelism reference
description: How the OODA daemon fills spare machine capacity with concurrent engineers on distinct work items, bounded by the AIMD safety cap — and how a parallelizable goal is decomposed so coverage can parallelize it.
last_updated: 2026-06-26
owner: simard
doc_type: reference
status: reference
related:
  - ./concurrent-engineer-dispatch.md
  - ./goal-coverage-allocation.md
  - ./adaptive-scaling-api.md
  - ./ooda-coverage-parallelism-ceiling.md
  - ./ooda-decide-prompt.md
  - ./simard-cli.md
  - ../concepts/adaptive-scaling.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ../howto/configure-adaptive-scaling.md
---

# Maximum safe parallelism reference

> **Goal:** When the machine has spare capacity **and** there is parallelizable
> work, Simard fills the free slots with concurrent engineers — each on a
> **distinct** bounded work item — up to the AIMD safety cap. No engineer slot
> sits idle while parallelizable work remains, and the cap stays a hard,
> resource-aware ceiling that backs off under load.

This page explains the end-to-end mechanism, why "one engineer per goal" is the
unit of parallelism, and how a single multi-issue goal is decomposed so the
existing coverage allocator can run it in parallel.

Modules: `simard::ooda_loop::{orient, decide, coverage, adaptive_scaling}`,
`simard::ooda_actions::advance_goal`.
Prompts: `prompt_assets/simard/goal_session_objective.md`,
`prompt_assets/simard/ooda_decide.md` (+ `recipes/ooda-decide.yaml`).

## The problem

A single OODA cycle ran roughly **one** engineer even when the host had spare
CPU/RAM and a goal bundled lots of independent, parallelizable work. For
example, the goal `find-and-fix-recent-rysweet-filed-amplihack-rs-…` covered six
open issues (`#804`, `#807`, `#808`, `#809`, `#810`, `#815`) that could all be
fixed at once, yet only one engineer was working — the other five issues waited
behind a serial triage loop. The operator expectation is the opposite: *spawn
enough engineers to fill the machine as long as there is more independent work
to do, and stop adding them when the machine is under pressure.*

There were two distinct causes. First, a single umbrella goal bundling many
issues yields only **one** engineer (the unit of parallelism is the goal), so it
must be **decomposed** into distinct per-issue goals — the prompt-driven #2405
fix this page covers. Second, even once several distinct goals are planned, the
Act phase **dispatched them serially**: it held a global lock and the single
shared LLM session across each goal-action `run_turn` (~30–90 s), so the
planned-parallel actions serialized and only ~1 engineer started per round. That
second cause is fixed by
[concurrent engineer dispatch](./concurrent-engineer-dispatch.md).

## How per-cycle parallelism actually works

Each OODA cycle runs Observe → Orient → Decide → **Coverage** → Act. Parallelism
is the number of **distinct `AdvanceGoal` actions** that survive into Act — each
one becomes one spawned engineer.

| Phase | What it produces | Parallelism contribution |
|-------|------------------|--------------------------|
| **Orient** (`orient` / `orient_with_brain`) | **One `Priority` per active goal** (plus a few synthetic priorities) | The number of priorities is the number of goals — Orient does not fan a single goal into many. |
| **Decide** (`decide_with_brain`) | **One `PlannedAction` per priority** — the brain returns a single `DECISION: <variant>` per call | Caps the action list at `scaler.adjust()` (the AIMD cap). Cannot express "spawn N for this one goal" — the output schema is one decision per priority. |
| **Coverage** (`ensure_goal_coverage`) | Guarantees **one engineer per incomplete unassigned goal**, preserves extra parallelism behind coverage, truncates to `cap` | Parallelizes across *distinct goals*, up to the cap. See [goal coverage allocation](./goal-coverage-allocation.md). |
| **Act** (`dispatch_actions_bounded` → `concurrent::dispatch_advance_concurrent`) | Spawns/heartbeats the engineer for each action | **Dispatches the unassigned spawn-path `AdvanceGoal` actions concurrently** — each with its own LLM session, bounded by the AIMD `cap` — so all planned engineers start in **one** round. Heartbeat actions (`assigned_to` set) stay serialized; spawning is de-duplicated per goal (`find_live_engineer_for_goal`) and per round (atomic claim). |

**Key consequence — the unit of parallelism is the goal.** A goal has a single
`assigned_to` slot, the Act phase heart-beats (rather than re-spawns) a goal that
already has a live engineer, and spawning is de-duplicated per goal. So a single
goal yields **one** engineer no matter how many `AdvanceGoal` actions name it.
**N concurrent engineers require N distinct goals.**

Multi-*goal* parallelism therefore already works: with several incomplete goals,
coverage spawns one engineer per goal, up to the cap. The missing piece is the
single-goal-with-many-issues case.

> **Dispatch is concurrent, not serial.** Planning N distinct `AdvanceGoal`
> actions is necessary but not sufficient — the Act phase must also *dispatch*
> them concurrently, and it does (see
> [concurrent engineer dispatch](./concurrent-engineer-dispatch.md)). Earlier the
> dispatcher held a global lock and the single shared LLM session across each
> goal-action `run_turn`, so even a planned-parallel batch serialized and only
> ~1 engineer started per round. Spawn-path `AdvanceGoal` actions now run
> concurrently, each with its own session, bounded by the same AIMD cap.

## The AIMD safety cap (do not remove or weaken)

The cap is the [`AdaptiveScaler`](./adaptive-scaling-api.md), enabled with
`SIMARD_SCALING=auto`. It is the mechanism that already implements *"fill the
machine while there is headroom, back off under load."* Once per cycle,
`adjust()` samples pressure and applies the AIMD rule:

- **Additive increase** (`current + 1`, clamped to `ceiling`) when system
  pressure `< 0.3` and there are no recent 429s — climb to use spare capacity.
- **Multiplicative decrease** (`current × 0.5`, clamped to `floor`) when
  pressure `> 0.8` **or** a 429 / "rate limit" error landed in the last 300 s.
- **Hold** otherwise.

Pressure is `max(cpu_pressure, mem_pressure)` from `/proc/stat` and
`/proc/meminfo`; 429/rate-limit signals come from `report_error` /
`report_reason`.

**Production bounds.** `OodaConfig::default()` constructs the scaler as
`AdaptiveScaler::new(base, 1, ceiling)` where `base` is resolved from the
environment (preferring `SIMARD_OODA_MAX_CONCURRENT`, falling back to the legacy
`SIMARD_MAX_CONCURRENT_ACTIONS`, else the default **24**; range `1..=64`,
fail-closed) and `ceiling = base` (default **24**). Once per cycle the daemon
logs the cap it used in the coverage line `[simard] OODA cycle: coverage — …
(cap N)`, where `N` is the scaler's `current_max()` (or `max_concurrent_actions`
when `SIMARD_SCALING` is off). That cap **starts at `base` and adapts** — it
holds at the ceiling under low pressure, halves under CPU/memory/429 pressure,
and recovers `+1` per calm cycle. The default ceiling of **24** lets a single
cycle cover up to 24 genuinely-independent goals; raise or lower it with
`SIMARD_OODA_MAX_CONCURRENT`. The additive-increase + pressure/error backoff
behavior is unchanged by this — only the base and ceiling values moved
(from 5/20 to 24/24). See
[OODA coverage parallelism ceiling](./ooda-coverage-parallelism-ceiling.md).

> **The ceiling is not a spawn guarantee.** 24 is the number of goals coverage
> may *plan* per cycle; the resource-admission gate (disk / memory / load) and
> the overlap/dependency gate still bound how many engineers actually spawn. A
> full board at cap 24 under disk pressure still defers.

> The cap is a **hard ceiling**: coverage never emits more than `cap` actions,
> and the scaler shrinks the cap under CPU/memory/429 pressure. Filling the
> machine is always resource-aware — it cannot thrash the host.

## Filling spare capacity: decompose a parallelizable goal

Because the unit of parallelism is the goal and the Decide schema is one
decision per priority, the way to fill spare capacity for a single multi-issue
goal is to turn it into **multiple distinct goals**, which coverage then
parallelizes. The decomposition itself is a prompt-driven behavior of the
goal-action brain (`goal_session_objective.md`) — it changes neither the AIMD
cap nor the dispatch core, so it hot-reloads from
`~/.simard/prompt_assets/simard/` without a binary rebuild. (Dispatching the
resulting per-issue goals *concurrently* is a separate Rust change to the
dispatch core that **does** require a binary rebuild — see
[concurrent engineer dispatch](./concurrent-engineer-dispatch.md).)

When a goal is an umbrella over several **independent** open issues and live
engineers are below the cap, the goal-action brain spawns one engineer whose
bounded task is to:

1. Enumerate the independent, still-open, `rysweet`-filed issues the umbrella
   covers (the rysweet-only gate in Priority Order tier 0 still applies).
2. Create **exactly one** concrete goal per distinct issue with an explicit
   done-when criterion, via
   `simard goal add <priority> [--repo <slug>] "<issue-scoped scope>; done when …"`
   (see the [`simard goal add` CLI reference](./simard-cli.md)).
3. Stop — the umbrella engineer does **not** fix the issues itself.

On the next cycle, coverage sees the new per-issue goals and spawns one engineer
for each, in parallel, up to the cap. The umbrella then **delegates**: it
records progress and prefers `NO ACTION` while the per-issue goals run, and
completes once every issue it covered is closed.

```
Cycle N     umbrella goal ──(1 engineer)──► creates goals: fix-#804 … fix-#815
Cycle N+1   coverage covers fix-#804, fix-#807, fix-#808, … (one engineer each,
            up to cap; AIMD raises the cap additively while pressure is low)
Cycle N+k   each per-issue goal completes when its fix merges; umbrella → done
```

## Collision-safety (distinct work, never duplicate)

Parallel engineers must work **distinct** items so they never duplicate effort
or re-triage the same state:

- **One issue per goal.** The decomposition creates exactly one goal per issue
  and never two goals for the same issue.
- **The umbrella delegates, it does not duplicate.** Once decomposed, the
  umbrella engineer stops working the issues; each per-issue goal owns its issue.
- **Per-goal de-duplication is preserved.** `find_live_engineer_for_goal` and the
  single `assigned_to` slot still prevent two engineers on the same goal.
- **Loop-awareness (issue #2404) is preserved.** Decomposing *is* the loop-break
  for a "find-and-fix-N-issues" umbrella that would otherwise re-triage the same
  list every cycle — it does not spawn parallel engineers that all re-triage the
  same thing.

## Output contracts (unchanged)

The Rust parsers are untouched; the prompts keep their existing shapes:

- **Decide** (`ooda_decide.md`, `recipes/ooda-decide.yaml`): first non-blank line
  is still `DECISION: <variant>` (embedded path) / the first whitespace token is
  still the action keyword (recipe path). The added *Parallelism* section is
  body guidance only — it introduces **no new variant** and routes each distinct
  goal to `advance_goal`.
- **Goal action** (`goal_session_objective.md`): still the two response shapes —
  a prose **Spawn an engineer** paragraph (the decomposition task uses this) or
  `NO ACTION` on its own line, with an optional `PROGRESS: NN` marker. The
  decomposition is a normal spawn-engineer paragraph; no new shape is added.

## Invariants

- **Cap is never exceeded.** Coverage emits at most `cap` actions per cycle; the
  AIMD scaler bounds and shrinks `cap` under pressure.
- **Resource-aware.** Additive increase only while pressure `< 0.3` and no recent
  429s; multiplicative decrease under high pressure or rate-limit errors.
- **Distinct engineers, distinct items.** One engineer per goal; one issue per
  decomposed goal; no duplicate work.
- **No idle capacity while parallelizable work remains** — given enough distinct,
  bounded goals on the board, coverage fills every free slot up to the cap.
- **Hot-reloadable decomposition.** The goal-*decomposition* fill behavior is
  prompt-only; it requires no binary rebuild and does not alter the AIMD cap or
  the dispatch core. (Dispatching the planned actions *concurrently* is a
  separate dispatch-core change that does need a rebuild — see
  [concurrent engineer dispatch](./concurrent-engineer-dispatch.md).)

## Related reading

- [Concurrent engineer dispatch](./concurrent-engineer-dispatch.md) — how the
  Act phase dispatches the planned spawn-path `AdvanceGoal` actions concurrently
  (per-goal LLM sessions, atomic claim, semaphore cap) so N planned engineers
  actually start in one round.
- [Goal coverage allocation](./goal-coverage-allocation.md) — the per-cycle
  allocator that guarantees one engineer per incomplete goal and parallelizes
  distinct goals up to the cap.
- [Adaptive scaling API](./adaptive-scaling-api.md) — the AIMD scaler that
  supplies the resource-aware safety cap.
- [OODA Decide prompt](./ooda-decide-prompt.md) — the routing brain whose
  *Parallelism* section explains why distinct goals are the unit of fan-out.
- [`simard goal add` CLI reference](./simard-cli.md) — the mechanism the
  decomposition uses to create one per-issue goal per distinct issue.
- [Configure adaptive scaling](../howto/configure-adaptive-scaling.md) — how to
  widen the ceiling via `SIMARD_OODA_MAX_CONCURRENT`.
- [OODA coverage parallelism ceiling](./ooda-coverage-parallelism-ceiling.md) —
  the default-24 ceiling, its env override, and why it never bypasses the
  resource-admission or overlap gates.
