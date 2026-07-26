---
title: Configure the cognitive-thread batch (the ten reflective threads)
description: >
  Operator + developer guide for enabling, tuning, observing, and testing the
  ten new cognitive threads (issue #5) — metacognition, consolidation,
  reflection, prospection, salience, operator_model, analogy,
  values_deliberation, interoception, and narrative. Covers the double env gate
  (SIMARD_COGNITIVE_THREADS_ENABLED plus each SIMARD_THREAD_<NAME>_ENABLED),
  the recommended one-at-a-time rollout, per-thread cadence tuning and the
  global SIMARD_THREAD_INTERVAL_SCALE multiplier, the non-critical per-tick
  budget, what to watch in telemetry and the durable stores, and how the
  built-in fakes make every thread offline-testable. All ten are OFF by default.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/cognitive-threads-catalog.md
  - ../reference/recipe-invoker-seam.md
  - ../concepts/salience-and-decide.md
  - ./configure-cognitive-thread-scheduling.md
  - ./add-a-new-cognitive-thread.md
  - ./configure-creative-ideas-thread.md
  - ../reference/telemetry-metrics.md
---

# Configure the cognitive-thread batch (the ten reflective threads)

This guide is for operators and developers turning on the **ten new cognitive
threads** that mature Simard's single mind (issue #5). For what each thread *is*
— its vision, inputs, outputs, and recipe — see the
[Cognitive-threads catalog](../reference/cognitive-threads-catalog.md). For the
shared brick they run on, see
[The RecipeInvoker seam](../reference/recipe-invoker-seam.md).

!!! note "Status — OFF by default, roll out one at a time"
    Every thread ships **OFF by default** behind a **double env gate** and is
    additive: with the master gate unset, nothing registers and there are zero
    side effects. The recommended posture is to enable **one thread at a time**,
    watch its telemetry and durable output for a few cadences, then enable the
    next. Threads write facts, metrics, issues, and (rarely) goal-board
    proposals — but never merge, never act outside the existing OODA/overseer
    path, and never file point-in-time report docs.

## When to use this

- You want Simard to **reflect** on its own reasoning, consolidate memory,
  learn lessons from finished goals, anticipate risk, prioritize by urgency,
  model the operator, transfer patterns across domains, deliberate hard
  tradeoffs, watch its own health, or keep a coherent self-story.
- You are rolling the batch out incrementally in a live daemon and need to know
  which knob does what.
- You are testing a thread and want the offline, credential-free path.

## The double env gate

Nothing registers unless **both** gates are truthy:

1. **Master gate** — `SIMARD_COGNITIVE_THREADS_ENABLED` (also required by the
   existing cognitive-thread scheduler; see
   [Configure cognitive-thread scheduling](./configure-cognitive-thread-scheduling.md)).
2. **Per-thread gate** — `SIMARD_THREAD_<NAME>_ENABLED`.

Truthy set: `{1, true, TRUE, yes, on}`. Anything else (including unset) is OFF.

Belt-and-suspenders: each thread's `enabled()` **also** reads its config flag, so
even a thread that somehow registered without its gate would `skipped()` every
tick.

| Thread | Per-thread gate |
|--------|-----------------|
| metacognition | `SIMARD_THREAD_METACOGNITION_ENABLED` |
| consolidation | `SIMARD_THREAD_CONSOLIDATION_ENABLED` |
| reflection | `SIMARD_THREAD_REFLECTION_ENABLED` |
| prospection | `SIMARD_THREAD_PROSPECTION_ENABLED` |
| salience | `SIMARD_THREAD_SALIENCE_ENABLED` |
| operator_model | `SIMARD_THREAD_OPERATOR_MODEL_ENABLED` |
| analogy | `SIMARD_THREAD_ANALOGY_ENABLED` |
| values_deliberation | `SIMARD_THREAD_VALUES_ENABLED` |
| interoception | `SIMARD_THREAD_INTEROCEPTION_ENABLED` |
| narrative | `SIMARD_THREAD_NARRATIVE_ENABLED` |

!!! note "One gate name is abbreviated on purpose"
    Every gate is `SIMARD_THREAD_<UPPER_NAME>_ENABLED` except **values_deliberation**,
    which is deliberately shortened to `SIMARD_THREAD_VALUES_ENABLED` (used
    consistently by the loop below and by the thread's own `enabled()` check). Read
    the gate from this table; do not derive `values_deliberation`'s name by rule.

!!! warning "Gates are rollout controls, not authorization"
    The env gates control **blast radius**, not security. The real safety comes
    from the fences and invariants (numeric-only salience→Decide projection,
    untrusted-data fencing in every recipe, bounded per-thread output authority,
    the overseer's terminal veto). See
    [Salience and the OODA Decide handoff](../concepts/salience-and-decide.md).

## Enable one thread (recommended rollout)

Start with a cheap, self-contained thread — metacognition is a good first pick
because it only writes self-metrics and advisory facts:

```bash
export SIMARD_COGNITIVE_THREADS_ENABLED=1
export SIMARD_THREAD_METACOGNITION_ENABLED=1
# start / restart the daemon as usual
```

Watch for one cadence (metacognition ticks hourly), confirm its live acceptance
signal (below), then enable the next thread. To enable the whole batch at once
(e.g., in a disposable test daemon):

```bash
export SIMARD_COGNITIVE_THREADS_ENABLED=1
for t in METACOGNITION CONSOLIDATION REFLECTION PROSPECTION SALIENCE \
         OPERATOR_MODEL ANALOGY VALUES INTEROCEPTION NARRATIVE; do
  export SIMARD_THREAD_${t}_ENABLED=1
done
```

## Cadence, priority, and the per-tick budget

Each thread runs on a fixed `Interval`, clamped to a 60-second floor. Intervals
are deliberately **non-harmonic** so the threads diverge after the first tick
rather than re-colliding.

| Thread | Priority | Default interval |
|--------|----------|------------------|
| salience | **Normal** | 1800 s (30 m) |
| interoception | **Normal** | 3300 s (55 m) |
| metacognition | Low | 3600 s (1 h) |
| prospection | Low | 4500 s (75 m) |
| reflection | Low | 5400 s (90 m) |
| operator_model | Low | 7200 s (2 h) |
| analogy | Low | 9000 s (2.5 h) |
| values_deliberation | Low | 10800 s (3 h) |
| consolidation | Low | 21600 s (6 h) |
| narrative | Low | 43200 s (12 h) |

The `Mind` runs OODA (`Critical`, budget-exempt) first, then at most
`SIMARD_MIND_MAX_NONCRITICAL_PER_TICK` (default **2**) non-critical threads per
tick, `Normal` before `Low`. This budget is the hard backstop: even on a
pathological first tick where every thread is due, at most two run and the rest
drain over subsequent ticks. OODA is never among the budgeted threads.

### Guarded threads skip cheaply

`reflection` and `values_deliberation` are **Interval + guard**: they wake on
their interval but immediately `skipped()` (near-zero cost) unless their trigger
condition is present — a newly completed/failed goal for reflection, a
hard-tradeoff marker for values. This substitutes for an event-driven trigger
without touching the pure scheduler.

## Tuning intervals

There are two ways to slow (or speed) the threads:

- **Per-thread**, via each thread's `from_env` interval override (documented per
  thread in the catalog). Use this to slow one noisy thread.
- **Globally**, via `SIMARD_THREAD_INTERVAL_SCALE` (float, default `1.0`),
  applied to every thread's interval in `from_env` and re-clamped to the 60 s
  floor. This is the cheap knob to slow all ten at once for cost control:

```bash
# Halve the frequency of every cognitive thread (double every interval).
export SIMARD_THREAD_INTERVAL_SCALE=2.0
```

## What to watch

Each thread's health and output are observable through existing surfaces — never
a report doc:

- **Telemetry / `metrics.jsonl`** — thread heartbeats plus thread-specific
  metrics: `confidence_calibration_error` and `decision_quality`
  (metacognition); `interoception_disk_free_ratio` and
  `interoception_store_size` (interoception). See
  [Telemetry metrics](../reference/telemetry-metrics.md).
- **Durable memory** — `search_facts("<prefix>:")` for `metacog:`, `schema:`,
  `postmortem:`, `foresight:`, `salience:`, `operator:`, `analogy:`, `values:`,
  `interocept:`, `narrative:identity`.
- **Goal board** — proposals from metacognition, prospection, values, and
  interoception appear like any other goal (capacity-checked, enforcement-
  equivalent).
- **Issues** — interoception files a **deduplicated** issue on a threshold breach
  (summarized status only, never raw command/env output).
- **`state/salience_signal.json`** — the numeric-only Decide-facing signal (see
  the [salience concept](../concepts/salience-and-decide.md)).

### Per-thread live acceptance signal

Use these to confirm a freshly-enabled thread is actually running:

| Thread | After one due tick, expect… |
|--------|-----------------------------|
| metacognition | a `metrics.jsonl` line with `metric_name == "confidence_calibration_error"` |
| consolidation | episodes marked distilled + ≥1 `schema:` fact via `search_facts("schema:")` |
| reflection | complete a goal first; then a `postmortem:` fact exists |
| prospection | `list_all_prospective` returns a new trigger |
| salience | `state/salience_signal.json` exists, is fresh, lists validated goal ids only |
| operator_model | `search_facts("operator:")` returns ≥1 fact (no seeded token echoed) |
| analogy | `search_facts("analogy:")` returns ≥1 fact |
| values_deliberation | mark a dilemma; then a `values:` fact exists (no veto artifact) |
| interoception | a `metrics.jsonl` line matching `interoception_*` |
| narrative | `search_facts("narrative:identity")` returns **exactly one** fact |

## Testing without a live agent

Every recipe-backed thread is offline-testable through the
[`FakeRecipeInvoker`](../reference/recipe-invoker-seam.md#offline-test-double-fakerecipeinvoker):
inject a canned `InvokeResult`, inject the clock, run one `tick`, and assert the
writes (and dedup on a second identical tick). Interoception needs no recipe at
all — stub its deterministic probes and it is fully offline by construction. No
subprocess, no network, no credentials, no sleeps.

## Turning a thread back off

Unset (or set to a non-truthy value) that thread's `SIMARD_THREAD_<NAME>_ENABLED`
and restart. Because threads are additive and write only durable, prefixed
artifacts, disabling a thread simply stops new writes; nothing needs unwinding.
To disable the entire batch, unset `SIMARD_COGNITIVE_THREADS_ENABLED`.

## See also

- [Cognitive-threads catalog](../reference/cognitive-threads-catalog.md) — per-thread reference.
- [The RecipeInvoker seam](../reference/recipe-invoker-seam.md) — the shared brick and its fakes.
- [Salience and the OODA Decide handoff](../concepts/salience-and-decide.md) — the salience/Decide model and overseer-vs-values separation of powers.
- [Configure cognitive-thread scheduling](./configure-cognitive-thread-scheduling.md) — the master gate and scheduler knobs.
- [Add a new cognitive thread](./add-a-new-cognitive-thread.md) — build your own.
