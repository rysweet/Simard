---
title: "Concept: hybrid cognition measurement (benchmark + live)"
description: Why a cognition metric is only trusted once it improves on a FIXED benchmark AND trends the same direction in LIVE production — the G1 hybrid measurement surface, wired end-to-end for recall precision@k, with the measurement primitive de-forked into amplihack-memory-lib (G2).
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ../reference/recall-precision-hybrid-api.md
  - ../howto/measure-recall-precision-hybrid.md
  - ../concepts/rust-expertise-gym.md
  - ../reference/telemetry-metrics.md
  - ../concepts/perpetual-goal-no-progress-exemption.md
---

# Concept: hybrid cognition measurement (benchmark + live)

Simard has a standing, perpetual goal:
`continuously-research-and-improve-your-own-cognition`. A perpetual goal is
worthless without a **trustworthy** way to tell whether cognition actually got
better. A single number is not enough — a metric can look great on a fixed test
and mean nothing in production, or drift in production while the test set stays
frozen.

**Guideline G1** ("Simard needs to iterate toward a hybrid of benchmarked vs
self-measurement") makes the requirement concrete: a claimed cognition
improvement must be **proven on a fixed benchmark AND observed live**, not one or
the other. This page is the durable rationale for the hybrid measurement surface
that satisfies G1, wired end-to-end for the first metric,
**`recall_precision_at_k`** (ranked-recall precision@k).

## The problem with one measurement

| Measurement style | Strength | Failure mode on its own |
|---|---|---|
| **Fixed benchmark** (gym) | Deterministic, comparable across runs, no ground-truth labels needed | Can be gamed / overfit; a frozen corpus stops resembling production |
| **Live self-measurement** (telemetry) | Reflects real workloads as they shift | Non-stationary; a "gain" can be an easier week, not a smarter Simard |

Either rail alone produces a confident-looking number that can be wrong. The
hybrid rail crosses them: an improvement is only **confirmed** when the fixed
benchmark improves **and** the live trend moves the same way.

## The two rails, one metric

Both rails score the **same** quantity with the **same** primitive, so their
numbers are directly comparable.

```
                       amplihack-memory-lib
                    ┌───────────────────────────┐
                    │  measurement::precision_at_k  (G2: single source)  │
                    └───────────────┬───────────┘
                                    │ (Simard delegates — no fork)
              ┌─────────────────────┴─────────────────────┐
              ▼                                             ▼
   BENCHMARK rail (fixed corpus)                 LIVE rail (production)
   recall_precision_bench                        recall_facts_ranked (OODA)
   → ScoreRecord{suite:"cognition",              → observe_recall_precision(...)
      scenario:"recall_precision_at_k"}          → flush once per OODA cycle
   → gym_history.db (ScoreHistory)               → metrics.jsonl
              │                                             │
              └───────────────────┬─────────────────────────┘
                                  ▼
                    HYBRID / correlation (query time)
             GET /api/cognition/recall-precision
             latest benchmark score + live trend + verdict
                                  ▼
                     Dashboard Overview tab · System Status card (#2494)
```

- **Benchmark rail** — `recall_precision_bench` scores a small, hand-authored,
  **in-repo fixed corpus** (query → ranked facts → expected precision) and writes
  one comparable [`ScoreRecord`](../reference/recall-precision-hybrid-api.md#scorerecord)
  to the shared gym score history. Because the corpus is frozen and the scorer is
  deterministic, the score is reproducible in CI and comparable run-over-run. It
  feeds the existing gym signal machinery (`generate_signals`), so a benchmark
  regression raises the same `Regression` signal every other gym scenario does.
- **Live rail** — the ranked fact-recall path (`recall_facts_ranked`) already
  folds one precision@k observation per recall and drains a single aggregated
  `recall_precision_at_k` sample to `metrics.jsonl` **once per OODA cycle**. This
  rail is unchanged in behaviour; it now sources its precision math from the
  upstream primitive so the two rails cannot silently diverge.
- **Hybrid** — a read-only, query-time join on the shared metric name
  `recall_precision_at_k` (the benchmark's `scenario_id` **is** the live metric
  name) returns the latest benchmark score, the recent live trend, and a
  **correlation verdict**.

## The correlation verdict

The verdict is the whole point: it states, in one enum, whether a cognition claim
holds on **both** rails. It compares the benchmark run-over-run delta against the
live first→latest trend delta, both against the same `0.01` threshold the gym
already uses for regression.

| Verdict | Benchmark | Live trend | Meaning |
|---|---|---|---|
| `confirmed` | ↑ | ↑ | Improvement proven on the fixed corpus **and** observed live. The claim holds. |
| `benchmark-only` | ↑ | flat | Improved on the frozen test but production held flat — possible overfit or an unrepresentative corpus. |
| `live-only` | flat | ↑ | Production improved but the benchmark held flat — possible drift; the fixed corpus may be missing the improved case. |
| `diverging` | ↑ / ↓ | ↓ / ↑ | The rails **disagree in direction** — one improved while the other regressed. The strongest signal that a "gain" is illusory. |
| `regressed` | ↓ / flat | flat / ↓ | A drop on at least one rail with no offsetting rise on the other (both down, or one down while the other holds flat). |
| `stable` | flat | flat | Neither moved beyond threshold. |
| `insufficient` | — | — | Not enough history on one or both rails to judge (needs ≥2 benchmark runs and ≥2 live samples). |

Only `confirmed` backs a "cognition improved" claim. `diverging` is the loudest
alarm — the two rails contradict each other, so any apparent gain is illusory.
`benchmark-only` and `live-only` are the softer diagnostics — they tell Simard
*which* rail to distrust rather than letting a half-true number through.

The verdicts are **total**. Each rail is classified as *up*, *flat*, or *down*
against the same `±0.01` threshold the gym uses for regression, and every one of
the nine direction combinations (plus `insufficient` when a rail lacks history)
maps to exactly one verdict — the correlation can never return "nothing." The
[reference](../reference/recall-precision-hybrid-api.md#correlation-verdict) gives
the full 3×3 matrix and the ordered rules.

## G2: the measurement primitive lives upstream

**Guideline G2**: any memory-measurement capability (recall scoring, fact-yield)
belongs in **`amplihack-memory-lib`**, not forked into Simard. Before this work,
`precision_at_k` lived only in Simard's `cognitive_memory/metrics.rs` — a standing
G2 violation and exactly the kind of fork that lets a benchmark and a live rail
drift apart.

This surface **de-forks** it: the scoring primitive moves to
`amplihack-memory::measurement`, Simard's `precision_at_k` becomes a thin adapter
that delegates upstream, and Simard's pin (`Cargo.toml` git-rev +
`Cargo.lock`) is bumped in lockstep to an upstream-`main` commit. Both rails now
call one primitive, so "the benchmark and live measure the same thing" is a
compile-time fact, not a convention. See
[Self-maintain dependency pins](../howto/self-maintain-dependency-pins.md) for the
lockstep-bump discipline.

## Where it surfaces

The correlation panel renders on the dashboard **Overview** tab, alongside the
existing **System Status** card — the status surface intended to host the
per-domain competency scorecard (roadmap
[#2491](https://github.com/rysweet/Simard/issues/2491), measurement issue
[#2494](https://github.com/rysweet/Simard/issues/2494)). No new tab is
introduced. Operators read the benchmark score, the live trend, and the
verdict in one place; the [how-to](../howto/measure-recall-precision-hybrid.md)
walks through running the benchmark and reading the result.

## Scope

This is **one coherent increment**: the measurement surface plus **one** metric
(`recall_precision_at_k`) wired end-to-end across benchmark, live, and
correlation. Adding a second metric (distillation fact-yield, reasoner
reliability), a full multi-domain scorecard, or new gym harnesses are explicit
follow-ups, not part of this slice. Each new metric follows the same pattern:
land the primitive in `amplihack-memory-lib`, add a fixed-corpus benchmark that
writes a comparable `ScoreRecord`, record the same metric live, and let the
shared-name join produce a verdict.

## See also

- [Reference: recall-precision hybrid measurement API](../reference/recall-precision-hybrid-api.md)
  — the primitive, the benchmark, the operator command, the endpoint, the schema.
- [How to measure recall precision on both rails](../howto/measure-recall-precision-hybrid.md)
  — run the benchmark and read the correlation verdict.
- [Concept: the Rust domain-expertise gym](../concepts/rust-expertise-gym.md)
  — the sibling acquire → retain → measure slice under #2491.
- [Telemetry metrics reference](../reference/telemetry-metrics.md)
  — the live-metric plumbing (`self_metrics` / `metrics.jsonl`).
