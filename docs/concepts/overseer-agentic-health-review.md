---
title: Overseer agentic health-review
description: >
  Why the Simard Overseer now reviews its OWN process health AGENTICALLY on every
  due tick behind a thin deterministic rail, instead of a Rust failure counter. A
  reasoning recipe reads the observable state the daemon already emits
  (journalctl --user -u simard-ooda, simard status, simard goal list), detects
  crash-loops and clusters a shared failure signature across goals into a
  systemic-vs-per-goal root cause, and drives remediation through the EXISTING
  capabilities LaunchRecipe (one systemic fix) and EscalateBlockedGoal (a
  plain-English operator notification on both channels). Deliberately WITHOUT
  record_step_failure plumbing or an N-identical-failure threshold counter: the
  journal already contains every failure, and an agent reading it sees them all.
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./agentic-recipes-first-principle.md
  - ./overseer-goal-board-health.md
  - ./overseer-root-cause-why.md
  - ./agentic-merge-queue-reasoning.md
  - ./automated-disk-health.md
---

# Overseer agentic health-review

> **Status: implemented.** This page describes the shipped health-review pass in
> present tense. It gives the Overseer the same self-diagnosis reflex the
> operator had to perform by hand behind the 286× actor-binding crash-loop.

## The problem: a crash-loop no code counted

Simard's Overseer watches HOW Simard performs and drives improvements from
OUTSIDE her own OODA loop. But when the OODA daemon fell into a crash-loop — the
same actor-binding failure re-firing across **7 goals more than 286 times** — the
Overseer never self-healed. No Rust sensor was counting "this identical failure
just fired for the Nth time," so nothing tripped.

The operator fixed it AGENTICALLY, in a handful of reads, exactly as
[the agentic-recipes-first principle](./agentic-recipes-first-principle.md)
describes:

```
journalctl --user -u simard-ooda    # every failure, whatever module raised it
simard status                       # process health + telemetry
simard goal list                    # per-goal state
```

From those reads the operator *reasoned*: the same signature across many goals is
one **systemic** defect, not seven independent ones — fix it once. That judgment,
not a counter, is what was missing.

## The wrong fix (deliberately not built)

The tempting imperative "fix" is to wire a `record_step_failure` call into every
failure-origin site, keep a consecutive-failure counter, and trip remediation at
an N-identical-failure **threshold**. We do **not** do this, for two reasons:

1. **It is redundant.** The journal already contains every failure, tagged with
   its module and signature. An agent that reads the journal sees them all —
   there is nothing a per-site counter knows that the journal does not.
2. **It hard-codes judgment as a constant.** "Is this a crash-loop? Is the cause
   systemic or per-goal? Fix it or escalate to a human?" is exactly the reasoning
   the operator did — and it does not reduce to a threshold. Encoding a magic `N`
   in the sensor freezes a guess where an agent should reason.

## The design: a thin rail + an agentic brain

Health-review follows the two established precedents in the codebase:
[`ecosystem-observe`](./agentic-merge-queue-reasoning.md) (an agentic recipe run
inside `Overseer::run_cycle` whose output becomes gated interventions) and
[`disk-health-check`](./automated-disk-health.md) (a thin Rust rail parsing text
markers from a recipe run). Health-review blends both:

- **The brain — `overseer-health-review.yaml`.** One agent step, each due tick,
  reads the journal + `simard status` + `simard goal list` itself, reasons about
  crash-loops and shared failure signatures, and emits a small set of typed
  DECISION markers. All judgment lives here.
- **The rail — `src/overseer/health_review.rs`.** A thin, fail-closed Rust
  seam that schedules the recipe, parses the markers, and routes each decision
  into the SAME gate every other Overseer action uses. Rust never reads the
  journal, counts a failure, or encodes a threshold.

### The typed decisions

The agent emits plain-text marker lines; the rail parses them:

| Marker | Meaning | Routed to |
| --- | --- | --- |
| `HEALTHY` | nothing wrong | nothing (fabricates no work) |
| `LAUNCH_RECIPE=<json>` | one **systemic** fix | `Intervention::LaunchRecipe` → the same gated `smart-orchestrator` launch path every fix uses |
| `ESCALATE_GOAL=<json>` | one **per-goal** block a human must decide on | `Intervention::EscalateBlockedGoal` → the escalation-triage recipe + a plain-English operator notification on BOTH channels (email + Signal) |
| `HEALTH_REVIEW_COMPLETE=<summary>` | REQUIRED terminal marker | gates the pass — without it the run is treated as degraded and takes NO action |

`ESCALATE_GOAL` carries the same operator-facing split the goal-board health path
uses: `problem` and `next_step` are **plain English** for the human, while
`reason`/`why` are internal jargon for telemetry.

### Systemic vs per-goal

The core reasoning: when the SAME signature (same panic, same missing actor
binding, same parse failure, same exit code) appears across MULTIPLE goals, the
cause is almost certainly one **systemic** defect — remediate it ONCE with a
single `LAUNCH_RECIPE`, not per-goal. A failure confined to ONE goal (an
unmeasurable done-gate, a goal-specific block) is **per-goal** — `ESCALATE_GOAL`
that one goal. Preferring one systemic fix over seven duplicate escalations is
precisely what would have broken the 286× loop.

## Reasoning is broad; authorization stays narrow

Broadening the Overseer's self-observation never widens what it is allowed to
DO. Every parsed decision flows through the UNCHANGED gate and act loop:

- A `LAUNCH_RECIPE` is subject to the same budget, launch-cap, sequencer,
  in-flight dedup, and anti-recursion author guard as every other launch.
- An `ESCALATE_GOAL` is HELD (present in the plan, not acted) when goal-board
  health is opted out (`SIMARD_OVERSEER_GOAL_HEALTH`), and requires the distinct
  steward identity to dispatch — exactly like every other escalation.

The rail is **fail-closed** end to end: an unwired reviewer, a disabled rail, an
off cadence, a `HEALTHY` verdict, a malformed/missing-field decision, a missing
terminal marker, or a failed recipe run all leave the plan unchanged — never a
fabricated launch or escalation.

## Configuration

| Env var | Effect | Default |
| --- | --- | --- |
| `SIMARD_OVERSEER_HEALTH_REVIEW` | opt-out for the whole rail | ON with the acting Overseer; an explicit falsey value (`0`/`false`/`no`/`off`) disables it |
| `SIMARD_OVERSEER_GAP_SCAN` | shared throttle for ALL agentic Overseer scans | ON; also disables health-review when off |
| `SIMARD_OVERSEER_GAP_SCAN_EVERY_N` | cadence divisor (run once every N ticks) | `1` (every tick), floored at `1` |
| `SIMARD_OVERSEER_HEALTH_REVIEW_UNIT` | systemd `--user` unit whose journal to read | `simard-ooda.service` |

A disabled acting Overseer forces the rail off regardless of the flag — the
review only makes sense while the Overseer runs.

## Where it runs

The pass runs inside `Overseer::run_cycle`, right after the ecosystem-observe
pass, on the shared gap-scan cadence plus its own every-N knob. `build_overseer`
wires a production `SpawnHealthReviewRecipeRunner` (resolving the recipe hot-copy
first, then in-tree); if `recipe-runner-rs` or the recipe is unavailable the rail
is simply not wired this build (the pass is skipped) rather than aborting the
tick. When absent — the bare constructor, tests — the Overseer behaves exactly
as before.

## Related

- [The agentic-recipes first principle](./agentic-recipes-first-principle.md) —
  why self-diagnosis is an agentic recipe, not imperative code.
- [Overseer goal-board health](./overseer-goal-board-health.md) — the
  self-heal + escalate capabilities this rail drives.
- [Overseer root-cause ("WHY") principle](./overseer-root-cause-why.md) — the
  always-on rule that remediation targets the root cause, not the symptom.
- [Automated disk health](./automated-disk-health.md) — the thin-rail /
  marker-parsing precedent.
