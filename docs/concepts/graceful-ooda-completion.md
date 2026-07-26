---
title: "Concept: graceful OODA completion and the bounded reflection safeguard"
description: Intended behavior for issue #1025 — once a goal's deliverable PR is verifiably green and its success criteria are met, the OODA reflection loop reaches a terminal ACHIEVED state and stops re-reflecting, with a bounded max-reflection safeguard that prevents unbounded LLM/compute spend while preserving Simard's perpetual-by-default posture.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: concept
issues: ["#1025"]
related:
  - ../reference/ooda-graceful-completion-api.md
  - ../howto/configure-graceful-ooda-completion.md
  - ./deploy-aware-done-gate.md
  - ../reference/completion-evidence-gate-api.md
  - ./closed-loop-outcome-verification.md
  - ./perpetual-goal-no-progress-exemption.md
  - ../howto/run-ooda-daemon.md
---

# [PLANNED - Implementation Pending] Concept: graceful OODA completion and the bounded reflection safeguard

This document describes the intended feature behavior for issue #1025.

Before this feature, the OODA reflection loop kept invoking the
reflect/verify-goal step even after a goal's deliverable PR was already merged
and green. The completion predicate never treated "deliverable PR green +
success criteria met" as a *terminal* state, so the loop re-reflected
indefinitely — a continuous, avoidable drain on the LLM budget and host
compute.

Graceful completion closes that gap. When — and only when — the existing
[deploy-aware done-gate](./deploy-aware-done-gate.md) has already returned a
verified `CompletionVerdict::Complete` for a goal, the reflection layer now
recognises that goal as **ACHIEVED** and breaks its loop instead of scheduling
another reflection cycle. A separate, bounded **max-reflection safeguard**
caps how many *no-progress* reflection cycles a goal may burn before the loop
yields, so a goal that can never satisfy its gate still cannot spin forever.

## What this feature is, and is not

This feature is a thin **decision layer** on top of evidence that already
exists. It does **not**:

- introduce a new evidence source or a new "done" signal;
- let a model self-report "I'm done" and have that terminate the loop;
- change the [deploy-aware done-gate](./deploy-aware-done-gate.md) clauses
  (merged PR + closed issue + verified deploy for self-affecting change); or
- alter the PRD's perpetual-operation posture by default.

It **only** consumes the gate's existing verified
[`CompletionVerdict`](../reference/completion-evidence-gate-api.md) and turns a
`Complete` verdict — together with the goal's recorded success criteria — into
a terminal loop decision.

## The terminal predicate

The predicate that decides whether a goal is achieved is pure and side-effect
free. It answers a single question:

> Are **all** of this goal's success criteria satisfied by **gate-verified**
> evidence?

A goal is `ACHIEVED` when the done-gate's verdict for the goal `is_complete()`
— i.e. its deliverable PR is merged/green, its linked issue is closed, and, for
self-affecting changes, the change is verifiably deployed.

> **Note on "success criteria".** `ActiveGoal` has no separate
> `success_criteria` collection; a goal's criteria are its `description`, and the
> done-gate is the component that evaluates them into the `CompletionVerdict`.
> The terminal predicate therefore consumes **only** `verdict.is_complete()` and
> must **not** perform a second, independent criteria check — doing so would
> re-derive evidence outside the gate, violating the evidence-only rule below.

If the verdict is not `Complete`, the goal is **not** achieved and the loop
continues, so the loop can never terminate on optimism — it terminates on
gate-verified evidence.

## The three loop-control decisions

Each reflection tick maps to exactly one decision:

| Decision | When | Effect on the loop |
| --- | --- | --- |
| `Continue` | Goal not yet achieved and reflection budget not exhausted | Run the next reflection cycle normally |
| `GracefulComplete` | Terminal predicate holds (gate-verified achieved) | Mark goal ACHIEVED, break the loop cleanly, emit a terminal `tracing` span |
| `BoundExceeded` | Goal still not achieved after `max_reflection_cycles` **no-progress** cycles | Yield the loop with a recorded blocker; do **not** claim success |

`GracefulComplete` is a success termination. `BoundExceeded` is a safety
termination that never fabricates completion — it stops the *spin* and surfaces
*why* the goal is still open, exactly as the
[no-progress breaker](./no-progress-root-cause-resolution.md) does.

## The no-progress streak, and why it resets

The safeguard counts **consecutive no-progress reflection cycles**, not total
cycles. Any cycle that produces shippable progress (a new commit, a PR state
change, a criterion newly satisfied, a blocker resolved) **resets the streak to
zero**. Only an unbroken run of purely reflective, evidence-unchanged cycles
advances the counter toward `max_reflection_cycles`.

This distinction matters: a healthy goal that is actively moving toward its
deliverable is never penalised, no matter how many cycles it takes. Only a goal
that is genuinely stuck — reflecting without changing any evidence — trips the
bound.

## Perpetual-by-default is preserved

Simard is a perpetual daemon. Graceful *goal* completion must never silently
turn her into a run-once process. Therefore:

- Graceful completion applies **per goal**, not to the daemon. When a goal is
  ACHIEVED the daemon frees that goal and carries on with the rest of the goal
  board and its standing research goal.
- Whether reaching all-ACHIEVED lets the *daemon loop itself* idle is gated by
  `SIMARD_OODA_STOP_WHEN_ACHIEVED`, which defaults to **off**. With the default,
  an all-ACHIEVED board keeps the daemon alive and steerable — consistent with
  the [standing research goal never idling](./research-goal-never-idle.md).
- The [perpetual-goal no-progress exemption](./perpetual-goal-no-progress-exemption.md)
  still applies: perpetual/standing goals are exempt from the `BoundExceeded`
  hard-yield, because "no shippable PR yet" is their normal steady state.

See the [configuration guide](../howto/configure-graceful-ooda-completion.md)
for the exact defaults and how to opt in to daemon-level idling.

## Observability

Every terminal decision is a structured `tracing` event (no `print!`/`println!`
anywhere in the new code path), carrying the goal id, the decision variant, the
no-progress streak, and — for `GracefulComplete` — the verified evidence that
satisfied the gate. Operators reading the OODA daemon log see a single, clear
"goal … ACHIEVED (gate-verified), reflection loop closed" line instead of an
unbounded stream of re-reflection ticks.

## Why the split (predicate vs. daemon wiring)

The terminal predicate and the reflection-bounds policy live in a pure module
with no daemon coupling, so they are unit-testable as a truth table and a
decision matrix. The daemon's `run_ooda_daemon` loop merely *consumes* the
decision. This mirrors the
[steerable-daemon rails split](./steerable-ooda-daemon.md): judgment stays in a
small, verifiable core; the daemon owns only orchestration and state mutation.

## Acceptance behavior

- A goal whose deliverable PR is merged/green with all success criteria met is
  marked ACHIEVED and its reflection loop exits (terminal path).
- A goal whose criteria are not yet met keeps reflecting (running path).
- A stuck, non-perpetual goal yields after the configured no-progress bound with
  a recorded blocker — never a false "complete".
- With defaults unchanged, the daemon stays perpetual even when the whole board
  is ACHIEVED.
