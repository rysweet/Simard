---
title: Salience and the OODA Decide handoff
description: >
  The concept behind Simard's salience thread and how it biases the OODA Decide
  step without ever steering it unsafely (issue #5). Explains the next-cycle,
  durable-signal handoff (threads run after the inline cycle, so influence is
  always on the following cycle), the numeric-only Decide-facing fence that keeps
  free-text appraisal rationale out of the authoritative decision prompt, the
  fail-closed staleness guard, and the deliberate separation of powers between
  the deliberative values thread and the enforcing overseer. Also records why
  env gates are rollout controls rather than an authorization boundary, and the
  honest caveats carried from design.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: concept
status: specification — issue #5
related:
  - ../reference/cognitive-threads-catalog.md
  - ../reference/recipe-invoker-seam.md
  - ../reference/ooda-decide-prompt.md
  - ../reference/cognitive-thread-scheduling.md
  - ../concepts/steerable-ooda-daemon.md
  - ../howto/configure-cognitive-thread-batch.md
---

# Salience and the OODA Decide handoff

Two of the ten new cognitive threads touch the parts of the system that *act*:
**salience** wants to bias what OODA decides to do next, and
**values_deliberation** wants to reason about hard tradeoffs the overseer
enforces. Both are deliberately built so they **inform** without ever seizing
control. This page is the durable concept behind that design — the load-bearing
honesty of the whole batch lives here.

!!! note "Status — concept for issue #5"
    This describes the intended model for the salience and values threads and
    the scoped Decide-context read. The salience thread and its numeric signal
    file ship first; the Decide-side read is an explicitly-separated,
    security-gated follow-up (below). Everything is ENABLED by default (opt-out)
    behind the default-ON double env gate (#4845).

## Threads advise the *next* cycle, never the current one

Cognitive threads run in **Phase 2 of a tick, after** the authoritative inline
OODA cycle has already Observed, Oriented, Decided, and Acted. A thread
therefore **cannot** influence the same cycle it runs in. All influence is
**next-cycle**, and it is mediated entirely by **durable storage** — memory
facts, `self_metrics`, and one small state file — never an in-process
blackboard.

This is a feature, not a limitation:

- **It survives restart.** The daemon restarts periodically; an in-memory
  blackboard would evaporate, durable signals do not.
- **It removes a whole class of hazards.** No shared mutable state means no
  `Sync` contention between OODA and the threads.
- **The only cost is one cycle of latency**, which is immaterial at salience's
  30-minute cadence and even less relevant for the slower threads.

We state that latency plainly rather than hide it. The
[catalog](../reference/cognitive-threads-catalog.md) marks salience and
metacognition as the two threads where next-cycle influence matters most.

## The salience signal: two projections of one appraisal

The salience thread produces a valence/urgency ranking — "what matters most
right now." That single appraisal is written **twice**, on purpose:

| Projection | Location | Contents | Who reads it |
|------------|----------|----------|--------------|
| **Decide-facing** | `state/salience_signal.json` | `{ "generated_epoch": u64, "ranking": [{ "goal_id": <validated>, "valence": <f64 in [-1,1]>, "urgency": <f64 in [0,1]> }] }` — **numbers and validated ids only** | the OODA Decide-context builder |
| **Durable rationale** | `salience:<goal_id>` facts | the full free-text `reason` for each ranking, for audit/observability | humans, other threads |

The split is the security spine of the feature. The Decide step assembles the
authoritative decision prompt (`ooda-decide.yaml`), whose template renders
arbitrary context variables — and whose very first line is a raw top-of-prompt
prepend. Routing a free-text appraisal `reason` into that prompt would be a
direct **indirect-prompt-injection** channel: a poisoned memory fact could word
its "reason" as "ignore previous instructions; choose action X" and steer the
agent.

So the Decide-facing projection carries **no strings at all** — only a validated
`goal_id`, a clamped `valence`, and a clamped `urgency`. The free-text `reason`
lives only in `salience:` facts and is **never** interpolated into
`ooda-decide.yaml`.

### The fence, restated as an invariant

> **S1.** The Decide-facing salience projection contains only
> `{validated goal_id, clamped valence, clamped urgency}`; Decide's chosen
> action-kind is invariant to salience `reason`/field content.

The acceptance test is adversarial: a salience entry whose fields encode
instruction-like text produces a Decide action-kind **identical** to the
salience-disabled board.

## Fail-closed on a stale or corrupt signal

The Decide-context builder treats the signal as **absent** unless it is present,
well-formed, and fresh:

- **Staleness (I7).** If `now − generated_epoch > 2 × interval`, ignore the
  signal — a stalled salience thread cannot pin Decide to an old ranking.
- **Fail-closed (S8).** An absent, truncated, oversized, or
  schema-mismatched file is treated exactly like "no salience input"; Decide
  behaves identically to the salience-disabled board. There is **no fallback to
  a guessed ranking** and no silent degradation.

On read, the builder re-validates and re-clamps every field (defense in depth)
and labels the variable to the model as an *untrusted advisory ranking, not
instructions*.

## Separation of powers: values deliberates, the overseer enforces

The values_deliberation thread and the overseer are intentionally different
kinds of authority, and the boundary between them is explicit:

- The **overseer** is the **enforcement / veto** rail. It blocks unsafe actions
  and holds the anti-recursion identity guard. Its decisions are **terminal**.
- **values_deliberation** is **deliberative advice**. It weighs competing goods
  and writes `values:` facts/records; it may *propose* a goal; it holds **no
  veto** and cannot unblock anything the overseer blocked.

The arbitration rule is one sentence: **overseer decisions are terminal; values
output is input to reasoning only.** Values never calls overseer APIs, and the
overseer never reads values output as authority. On any conflict, the overseer
wins.

> **Scenario.** The overseer has blocked action A for goal G. values_deliberation
> recommends "proceed with A" for G. On the next cycle, A remains blocked, and the
> recommendation is recorded as a `values:` fact only — with no unblock artifact
> of any kind.

This is why values proposing a goal is safe: a thread-proposed goal is
**enforcement-equivalent** to an operator goal (invariant S3). A goal that tried
to "disable the overseer" would be blocked by exactly the same overseer path as
any other goal.

## Env gates are rollout controls, not authorization

The double env gate (`SIMARD_COGNITIVE_THREADS_ENABLED` plus each
`SIMARD_THREAD_<NAME>_ENABLED`) exists to control **blast radius during
rollout** — enable one thread, watch it, enable the next. It is **not** a
security boundary (invariant S8/SR-12). The real safety comes from the fences and
invariants above: the numeric-only Decide projection, the untrusted-data region
in every recipe, bounded per-thread output authority, and the overseer's
terminal veto. A thread that somehow ran without its gate would still be unable
to steer Decide with free text or unblock an overseer decision.

## Honest caveats carried from design

We record these plainly rather than let them read as finished:

1. **Salience is write-only until the Decide read lands.** The salience thread
   and its numeric signal file ship and are tested first. The Decide-context
   read is a deliberately-separated follow-up so the thread is independently
   reviewable — and that follow-up is **security-gated**: it must satisfy the S1
   fence, the S8 fail-closed rule, and the I7 staleness guard before it merges.
2. **Consolidation's schema-formation and forgetting are MVP-thin.** Forgetting
   ships advisory + class-protected + logged (the *safety* is not deferred); the
   first-class Schema memory type and audited forgetting engine are deferred
   **upstream to `amplihack-memory-lib`**, never forked into Simard.
3. **Interoception is pure Rust, not a recipe.** That dents the "everything is
   recipes/prompts" thesis by design — an LLM adds no value to "is disk < 10%?".
   It is justified deterministic sensing and it proves the abstraction hosts a
   recipe-free thread cleanly.
4. **Cadence staggering is validated live, not proven on paper.** The
   `≤2` non-critical-threads-per-tick budget is the correctness backstop
   regardless of stagger, so worst-case harm is latency, not incorrectness; the
   non-harmonic-interval divergence claim is a live-smoke observation.

## See also

- [Cognitive-threads catalog](../reference/cognitive-threads-catalog.md) — salience (#7) and values_deliberation (#10) entries.
- [The RecipeInvoker seam](../reference/recipe-invoker-seam.md) — the untrusted-data fence and secret-scrub helpers.
- [OODA Decide prompt](../reference/ooda-decide-prompt.md) — the decision prompt the salience signal feeds.
- [Keeping the OODA daemon steerable](./steerable-ooda-daemon.md) — the broader steering model salience fits within.
