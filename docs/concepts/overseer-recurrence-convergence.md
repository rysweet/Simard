---
title: "Overseer recurrence convergence — the 2× middle remediation rung"
description: >
  How the acting Overseer closes the loop on signals that recur but never
  escalate. Cognitive-memory recall folds a recurrence count into every
  problem's WHY; a persistent-but-sub-escalation signal (seen exactly 2× — above
  the RECURRING_SIGNATURE_THRESHOLD noise floor but below the
  RECURRENCE_ESCALATION_THRESHOLD bar) previously had no remediation rung and
  re-appeared every cycle. This concept adds a single middle rung at 2×: a
  backlog-coverage gap that has recurred converges from notify-only into a
  BOUNDED auto-launch (one gap per cycle, behind every existing budget /
  concurrency / recursion gate). The blocked-goal ladder it sits alongside
  already exists; this feature does not change it. The two threshold constants
  are unchanged; only a new intermediate action is added.
last_updated: 2026-07-15
review_schedule: as-needed
owner: simard
doc_type: concept
status: design — not yet implemented
related:
  - ../design/overseer.md
  - ../reference/overseer-recurrence-convergence-api.md
  - ../howto/configure-overseer-recurrence-convergence.md
  - ./overseer-root-cause-why.md
  - ./overseer-goal-board-health.md
  - ./no-progress-root-cause-resolution.md
  - ../reference/overseer-workstream-gap-scan.md
  - ../reference/overseer-root-cause-why-api.md
  - ../reference/overseer-memory-recall-api.md
---

# Overseer recurrence convergence — the 2× middle remediation rung

The acting **Overseer** already asks *why* every problem occurred (the
[root-cause principle](./overseer-root-cause-why.md)) and already recalls prior
same-signature occurrences from cognitive memory, folding a **recurrence count**
into each problem's WHY. What it did **not** have was a remediation rung for a
signal that is *persistent but not yet escalation-worthy*.

## The dead zone this closes

Two thresholds bracket the Overseer's response to recurrence, and a third names
the new middle rung between them:

| Constant | Value | Meaning | Module |
|---|---|---|---|
| `RECURRING_SIGNATURE_THRESHOLD` | `2` | Noise floor — below this a repeat is not "recurring". | `src/overseer/signal.rs` |
| `WORKSTREAM_COVERAGE_LAUNCH_THRESHOLD` | `2` | **Middle rung (new)** — at or above this a recurring `WorkstreamCoverage` gap converges from notify-only to a bounded auto-launch. | `src/overseer/root_cause.rs` |
| `RECURRENCE_ESCALATION_THRESHOLD` | `3` | Escalation bar — at or above this the **root cause** is escalated to a human instead of being re-patched. | `src/overseer/root_cause.rs` |

A signal **seen exactly 2×** falls in the gap between them: it clears the noise
floor (it is genuinely recurring) but sits below the escalation bar (it is not
yet a human problem). With no rung in between, the Overseer would **observe and
flag** the same backlog-coverage gap every cycle, or **re-park** the same
blocked goal every cycle, producing the recurring-signature drift you can see in
cognitive memory:

```
overseer-obs:goal:blocked:fix-agent-kgpacks-rs-issue-17-… (seen 2×)
workstream-gap (seen 2×)
resource:engineer_spawn (drift)
```

Nothing was broken — the noise floor and escalation bar both did exactly their
job — but the *middle* was a passive loop.

## The middle rung

Recurrence convergence adds **one intermediate action** at `recurrence == 2`,
without touching either constant (they gate many tests and reconciling their
values risks broad breakage). The rung applies to one problem family:

### WorkstreamGap → bounded auto-launch

The [workstream gap-scan](../reference/overseer-workstream-gap-scan.md) was
**notify-only**: it surfaced uncovered work but never launched. Convergence keeps
that behaviour for a *first* sighting and upgrades it once a gap **recurs**:

- **recurrence `< 2`** — unchanged. `Intervention::FlagWorkstreamGaps` notifies
  the operator once (deduped) and files nothing.
- **recurrence `≥ 2`** — the Decide arm returns `Intervention::LaunchRecipe`
  for the **single top-ranked gap** (`pick_top_gap`), converting it into a
  bounded workstream. A recurring gap that nobody picked up stops being a
  perpetual ping and becomes actual work.

The launch is **bounded by construction**: at most **one gap per cycle**, behind
the Overseer's existing in-flight dedup gate, per-cycle launch cap, fail-closed
`RecursionGuard`, and `AutonomyGate` admission. It opens **no** new authority —
`LaunchRecipe` is `RiskClass::Routine` (investigation / coverage work only;
never merge, deploy, or destructive authority).

### Related: the blocked-goal ladder (pre-existing)

A goal that is re-parked every cycle with a bare "needs human review" marker is
the antipattern the [root-cause principle](./overseer-root-cause-why.md) and the
[no-progress resolution](./no-progress-root-cause-resolution.md) already fight.
The blocked-goal Decide path (`decide_blocked_goal`) **already** threads the same
`recurrence` count and one-line WHY, routing a repeatedly re-parked goal down the
self-resolving ladder (self-heal a false park, escalate a genuine block with its
analysis). Recurrence convergence sits alongside that existing rung and does
**not** modify it — it is listed here only for context, so the WorkstreamGap rung
is understood as a peer, not a replacement.

## Escalation still owns the top rung (Issue-17)

The dominant recurring blocker in the observed drift —
`fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed` — is an **external-repo
payload**, not in-repo code. It is not something the Overseer can fix by
launching an in-repo recipe. Once it crosses `RECURRENCE_ESCALATION_THRESHOLD`
(3×), it takes the top rung: an explicit **escalation** filed through the
existing `IssueFiler` (`gh issue create`, argv invocation, sanitized body) with
reproduction context, so a human sees the root cause exactly once instead of the
Overseer patching a symptom it does not own.

## `engineer_spawn` is characterised, not silenced

The recurring `resource:engineer_spawn` signature is classified in the problem's
WHY as either **benign membership drift** (a capacity ceiling doing its job — the
admission gate deferring a spawn under budget/concurrency pressure) or a genuine
**spawn failure** (something to mitigate). Naming which it is in the WHY stops it
from reading as an unexplained anomaly; a benign ceiling is documented as such,
not escalated.

## Why the constants do not move

It is tempting to "fix" the dead zone by raising the floor or lowering the bar.
The design deliberately does **neither**:

- `RECURRING_SIGNATURE_THRESHOLD` and `RECURRENCE_ESCALATION_THRESHOLD` gate a
  large body of tests (`tests_memory_recall.rs`, `tests_root_cause.rs`).
  Reconciling their values would ripple broadly.
- The 2× behaviour we want is a **new action**, not a **new boundary**. Adding a
  middle rung is surgical and backward-compatible; the `< 2` path returns the
  identical intervention it does today.

## How it fits the OODA loop

Convergence is **additive** and lives entirely inside the existing
Observe→Orient→Decide→Act loop:

- **Observe / Orient** — unchanged. Memory recall already populates
  `problem.why.recurrence` (see the
  [memory-recall API](../reference/overseer-memory-recall-api.md)).
- **Decide** — the `WorkstreamCoverage` arm now branches on
  `problem.why.recurrence` (the blocked-goal path already did). `decide()` stays
  a **pure** function (no I/O, no signature change); it only reads the
  already-populated recurrence count.
- **Act** — unchanged plumbing. A launch rides the existing `LaunchRecipe` path
  and its counter; an escalation rides the existing `IssueFiler`.

Every text field that originates in the multi-writer cognitive-memory graph is
`sanitize_recalled`-cleaned before it reaches a recipe brief, an issue body, or a
log line, and the Overseer uses its own `overseer-obs:` marker so recalled text
can never forge a dedup key.

## See also

- [Recurrence convergence API reference](../reference/overseer-recurrence-convergence-api.md)
  — thresholds, the Decide branch, `pick_top_gap` / `gap_to_brief`, bounding
  gates, security, and configuration.
- [Configure Overseer recurrence convergence](../howto/configure-overseer-recurrence-convergence.md)
  — operator guide: what launches, when, and how to turn the rung up/down/off.
- [Overseer root-cause (WHY) principle](./overseer-root-cause-why.md) — the WHY
  and its recurrence count this rung consumes.
- [Overseer workstream gap-scan](../reference/overseer-workstream-gap-scan.md) —
  the notify-only baseline convergence upgrades.
- [No-progress root-cause resolution](./no-progress-root-cause-resolution.md) —
  the self-resolving ladder blocked goals are routed down.
