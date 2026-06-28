---
title: Closing the procedural-learning loop
description: How Simard turns recurring successful episodes into reusable skills and recurring verified failures into Reflexion-style lessons, then recalls and applies both to condition future action — closing the episodic→procedural self-improvement loop. Covers the verified-signal gate (Verdict), the skill-reuse half (#2441), the failure-lesson half (#2458), lesson naming, recurrence gating, and the reuse/repeat-failure metrics.
last_updated: 2026-06-28
owner: simard
doc_type: concept
status: implemented
related:
  - ../memory.md
  - ../architecture/cognitive-memory.md
  - ../reference/procedural-learning-loop.md
  - ../reference/ooda-procedural-memory.md
  - ../reference/cognitive-memory-ranked-episodic-recall.md
  - ../reference/automatic-distillation-scheduler.md
  - ../reference/cognitive-memory-provenance.md
  - ../howto/inspect-the-procedural-learning-loop.md
---

# Closing the procedural-learning loop

> **Status: implemented**, with one honest boundary. The skill-reuse half
> (#2441) — usage-ranked recall, apply-time reinforcement, and the
> `brain_skill_reuse` / `brain_new_procedure` metrics — is live. The
> failure→lesson half (#2458) — reflection storage, recurrence-gated lesson
> distillation, and `brain_repeat_failure` — is implemented and tested, but its
> **production trigger requires an external failure signal (FU1)**: today no
> engineer-loop verdict reports `failed`, so the gate (which never fires on
> self-judged success) stays closed until FU1 lands. The executable contract,
> including the FU1 wiring, is
> [Procedural-learning loop API](../reference/procedural-learning-loop.md); the
> operator guide is
> [Inspect the procedural-learning loop](../howto/inspect-the-procedural-learning-loop.md).

Simard already *stored* procedural memory. What was missing was the **loop**:
a procedure had to be recalled, applied to condition the next action, and
reinforced when it helped — and a failure had to become a durable lesson that
a later, similar task actually consults. Storing without recall-and-apply is a
write-only memory. This feature closes both halves of that loop and gates
all learning on an **external verified signal** so Simard never trains on her
own optimistic self-assessment.

## The loop

```
            ┌──────────────────────── recall + apply ───────────────────────┐
            ▼                                                                │
   ┌─────────────────┐     verified success      ┌──────────────────────┐   │
   │ OODA preparation │ ───────────────────────▶ │  Procedural memory    │  │
   │ (orient/decide)  │ ◀──────────────────────── │  • skills  (ooda:…)   │  │
   └─────────────────┘     usage-ranked recall    │  • lessons (lesson:…) │  │
            │                                      └──────────────────────┘   │
            │ act                                            ▲                 │
            ▼                                                │ distill         │
   ┌─────────────────┐   Verdict::VerifiedSuccess   ────────┘ (recurring)      │
   │   OODA act +     │ ─────────────────────────────────────────────────────┘
   │  verification    │   Verdict::VerifiedFailure ─▶ reflection episode ─▶ lesson
   │  (external sig.) │   Verdict::Unverified      ─▶ learn nothing (fail-safe)
   └─────────────────┘
```

The loop has two halves that share one store, one recall path, and one gate.

### Half 1 — skill reuse (#2441)

1. **Distill on verified success.** When an action sequence completes with a
   `Verdict::VerifiedSuccess`, the OODA Act phase abstracts it into a named
   procedure (an `ooda:<kind>:<triggers>` *skill*).
2. **Usage-ranked recall.** OODA preparation recalls procedures whose name or
   steps match the current objective, ordered by `usage_count` (descending) so
   the procedures that have helped most often surface first.
3. **Apply + reinforce.** When a recalled procedure is surfaced into the
   cycle's prompt — i.e. *applied* to condition the next decision — its
   `usage_count` is reinforced and a `brain_skill_reuse` metric is recorded.
   Reinforcement is what makes a procedure that keeps working rise to the top
   of recall; this is the feedback edge that was previously missing.

### Half 2 — failure lessons (#2458)

1. **Reflect on verified failure.** When an action ends in a
   `Verdict::VerifiedFailure { error_class }`, Simard writes a short
   Reflexion-style **reflection** — what was attempted, the external verdict,
   and what to try differently — stored as an episodic note tagged
   `reflection` / `failure` and keyed by `(goal_type, error_class)`.
2. **Distill recurring failures into lessons.** A *one-off* failure is noise; a
   *recurring* one is a pattern. When the same `(goal_type, error_class)`
   reflection recurs at least `LESSON_RECURRENCE_THRESHOLD` times (default
   **2**), it is distilled into a `lesson:<goal_type>:<error_class>` procedure
   via the provenance write path, so the lesson links back to the reflections
   it came from.
3. **Recall + condition.** Because a lesson is just a procedure with a
   descriptive name, it co-ranks with skills through the *same* usage-ranked
   recall. A later attempt on the previously-failed goal-type surfaces the
   lesson into orient/decide, conditioning Simard to avoid the repeat.
4. **Watch for repeats.** If a `VerifiedFailure` recurs on a goal-type that
   *already* has a lesson, a `brain_repeat_failure` metric fires — the lesson
   did not take, which is a measurable self-improvement regression.

## The verified-signal gate

The single most important design decision is that **all** learning — skill
distillation, reflection, and lesson distillation — is gated on an injected
[`Verdict`](../reference/procedural-learning-loop.md#verdict), never on
Simard's self-reported `ActionOutcome.success`.

| `Verdict` | Source | Learning effect |
|---|---|---|
| `VerifiedSuccess` | external check passed (`VerificationReport.status`, real subprocess exit, gym eval) | distil/reinforce a **skill** |
| `VerifiedFailure { error_class }` | external check failed | write a **reflection**; maybe distil a **lesson** |
| `Unverified` | no external signal available | **learn nothing** (fail-safe) |

This closes a self-approval feedback hazard: a model that grades its own work
will, over many cycles, reinforce whatever it *believes* worked. Training a
durable memory on that signal compounds the error (cf. *Large Language Models
Cannot Self-Correct Reasoning Yet*, [arXiv:2310.01798](https://arxiv.org/abs/2310.01798)).
By gating on a real, external verdict and defaulting to `Unverified` ⇒ no
learning, Simard only ever durably remembers what an outside check confirmed.

`Unverified` is the safe default precisely *because* it is the most common
state today: when the full verification signal (FU1) is absent, the loop
records nothing rather than guessing.

## What is already there, and what will close the loop

Several pieces already exist; the proposed loop is what connects them.

| Capability | Status today | Role in the proposed loop |
|---|---|---|
| `store_procedure` on success | exists (#2280) | distil-on-success — to be **gated on `Verdict`** |
| Usage-ranked recall | exists (#2329/#2395) | will order skills *and* lessons by `usage_count` |
| Reinforce-on-use (`reinforce_access`) | exists (#2395) | the apply→reinforce feedback edge to wire in |
| Provenance writes (`store_procedure_with_provenance`) | exists (#2325/#2327) | will link lessons back to their reflections |
| **Failure → reflection → lesson** | **missing** | **Half 2 (#2458)** |
| **Verified-signal gate (`Verdict`)** | **missing** | **fail-safe gate (R10)** |
| **Reuse / repeat-failure metrics** | **missing** | **observability of the loop** |

The loop deliberately reuses the existing distillation and recall
infrastructure rather than introducing a parallel store. Lessons are ordinary
procedures distinguished only by a **naming convention**
(`lesson:<goal_type>:<error_class>`), so no schema change is required and they
will flow through recall, ranking, reinforcement, and provenance unchanged. See
[Why a naming convention, not a new column](../reference/procedural-learning-loop.md#lesson-naming).

## Boundaries

- The loop does **not** build the full external verification signal (FU1). It
  consumes whatever verified signal exists and treats its absence as
  `Unverified`.
- Lessons generalize over one `(goal_type, error_class)` pair at a time. Broad
  cross-goal generalization is out of scope for this pass.
- No new storage backend, schema overhaul, or live redeploy is involved.

## See also

- [Procedural-learning loop API](../reference/procedural-learning-loop.md) — the executable contract.
- [Inspect the procedural-learning loop](../howto/inspect-the-procedural-learning-loop.md) — operator guide.
- [OODA procedural memory](../reference/ooda-procedural-memory.md) — the store/recall substrate.
- [Memory architecture](../memory.md) — where this sits among the six memory types.
