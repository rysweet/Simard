---
title: Reliability-gate benchmark API reference
description: The fixed-corpus reliability-gate benchmark for the reasoner-reliability cognition axis — the frozen labeled corpus, the classification-accuracy score, its ScoreRecord and shared gym_history path, the `simard gym reliability-gate` operator command, and how it flows through the existing gym signal machinery to catch a silent regression in the distilled-fact gate's discrimination.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./recall-precision-hybrid-api.md
  - ../concepts/hybrid-cognition-measurement.md
  - ../architecture/episode-distillation.md
---

# Reliability-gate benchmark API reference

This page is the authoritative catalog for the **reliability-gate benchmark** —
a fixed-corpus, run-over-run comparable score for the reasoner-reliability axis
of Simard's cognition, wired into the same gym signal machinery the
[recall-precision benchmark](./recall-precision-hybrid-api.md) uses.

> **Modules:** `src/fact_reliability.rs` (the gate being benchmarked),
> `src/fact_reliability_bench.rs` (benchmark rail),
> `src/gym_history/mod.rs` (`ScoreHistory`, `default_db_path`, `generate_signals`),
> `src/operator_commands_gym/` + `src/operator_cli/gym.rs` (operator command).

## Why this exists

The distilled-fact **reliability gate**
(`fact_reliability::score_fact_reliability` / `fact_passes_gate`) is the
store-vs-quarantine decision every distilled fact passes through on **both**
write boundaries (the in-process sink and the IPC `StoreFactGated` handler). Its
discrimination — promoting grounded, informative facts while quarantining
ungrounded or no-information ones — is the reasoner-reliability axis of
cognition.

Before this benchmark, that gate had two observability surfaces:

- per-decision **unit tests** (binary pass/fail in `cargo test`), and
- a live per-pass **`distill_reliability_gate`** metric (the pass block-rate;
  see [episode distillation](../architecture/episode-distillation.md)).

What it lacked — and what the recall-quality axis already had via
`recall_precision_bench` — is a **fixed-corpus, run-over-run comparable
benchmark wired into the gym signal machinery**. So a silent regression in the
gate's *discrimination* (a weight/threshold change, or a change to the
informative-word content proxy) raised no gym `Regression` signal the way a
recall-precision regression does. This benchmark closes that gap. It is
**purely additive**: it changes neither the gate nor recall.

## Naming

Unlike recall precision — whose benchmark deliberately reuses the exact live
metric name as a shared join key so a hybrid correlation can line the benchmark
and live rails up — this benchmark measures gate **classification accuracy**, a
quantity with **no live twin** (the live `distill_reliability_gate` metric is a
block-*rate*, a different quantity). It therefore carries its own scenario id
and is a benchmark-only signal.

| Constant | Value | Defined at |
|---|---|---|
| `RELIABILITY_GATE_SUITE` | `"cognition"` | `fact_reliability_bench` (compile-time constant; same suite as recall precision) |
| `RELIABILITY_GATE_SCENARIO` | `"reliability_gate_accuracy"` | `fact_reliability_bench` (benchmark-only join key) |

`suite_id` and `scenario_id` are **compile-time constants**, never
request-derived, so no untrusted value ever reaches a SQL `WHERE` clause.

## The frozen corpus

The benchmark scores a small, hand-authored, **in-repo fixed corpus** of labeled
`(concept, content, grounded, expected_store)` cases through the **same**
`fact_reliability::fact_passes_gate` the live write boundaries use. Because the
corpus is frozen and the gate is a pure function, the score is reproducible and
comparable run-over-run — the property that makes it a *benchmark* rather than a
live sample.

Each case's expected disposition is derived directly from the
`score_fact_reliability` rubric (grounding `0.5` + content `≤0.3` + known-concept
`0.1`, threshold `0.5`):

| # | grounded | content | concept | score | expected |
|---|---|---|---|---|---|
| 1 | yes | ≥3 informative words | known | 0.9 | **store** |
| 2 | yes | ≥3 informative words | unknown | 0.8 | **store** |
| 3 | yes | 1–2 informative words | known | 0.75 | **store** |
| 4 | yes | 2 informative words | unknown | 0.65 | **store** |
| 5 | no | ≥3 informative words | known | 0.4 | quarantine |
| 6 | no | ≥3 informative words | unknown | 0.3 | quarantine |
| 7 | yes | no-information (`... ... ...`) | known | 0.0 (hard gate) | quarantine |
| 8 | yes | whitespace-only | known | 0.0 (hard gate) | quarantine |

The corpus is **discriminating**: it contains both store-expected and
quarantine-expected cases, so a degenerate always-store or always-quarantine
classifier cannot reach accuracy `1.0`. `corpus_is_discriminating()` is the
hollow-benchmark guard asserting this invariant.

## The score

The score is the gate's **classification accuracy**: the fraction of the frozen,
rubric-labeled cases the gate classifies as its documented rubric prescribes. It
is `1.0` by construction on a correct gate — the corpus encodes the rubric — so
its purpose is regression detection. Any future change that mis-scores a frozen
case drops the accuracy, and the **existing** gym signal machinery
(`generate_signals`) raises the same `Regression` signal every other gym
scenario does (a score DROP beyond the regression threshold).

## Public API

```rust
// src/fact_reliability_bench.rs

/// Gym suite id (compile-time constant): "cognition".
pub const RELIABILITY_GATE_SUITE: &str;
/// Gym scenario id (compile-time constant): "reliability_gate_accuracy".
pub const RELIABILITY_GATE_SCENARIO: &str;

/// Number of cases in the fixed corpus (surfaced as `samples`).
pub fn reliability_gate_corpus_size() -> usize;

/// Hollow-benchmark guard: true iff the corpus has >= 1 store-expected AND
/// >= 1 quarantine-expected case.
pub fn corpus_is_discriminating() -> bool;

/// Deterministic classification accuracy in [0,1] (1.0 on a correct gate).
pub fn score_reliability_gate_corpus() -> f64;

/// Append one ScoreRecord{suite, scenario, score, timestamp, commit_hash} to
/// the shared gym history and return it. Errors as SimardError::GymHistoryDb.
pub fn run_reliability_gate_bench(
    history: &ScoreHistory,
    commit_hash: Option<String>,
) -> SimardResult<ScoreRecord>;
```

The persisted `ScoreRecord` uses `RELIABILITY_GATE_SUITE` / `RELIABILITY_GATE_SCENARIO`,
so it flows through the **existing** `gym_history::generate_signals` unchanged —
no bespoke signal path is forked.

## Operator command

```text
simard gym reliability-gate
```

Mirrors `simard gym recall-precision`. It opens the shared gym history at
`gym_history::default_db_path()` (the same DB the OODA gym step uses), runs the
benchmark stamping the current `SIMARD_GIT_HASH` as the commit, and prints:

```text
cognition/reliability_gate_accuracy: score=1.0000 signal=stable samples=8
```

The gym signal needs a prior run to compare against; on the first run the
scenario has a single record and the printed signal is `stable`.

## Tests and scenario

- Unit tests: `src/fact_reliability_bench_tests.rs` — determinism,
  discriminating-corpus guard, baseline accuracy `1.0`, contract `ScoreRecord`,
  and flow through `generate_signals` (`Stable` on two identical scores).
- qa-scenario: `tests/qa-scenarios/reliability-gate-benchmark.yaml` drives those
  five hermetic in-crate tests via `cargo test --locked --lib`.
- CLI wiring: `src/operator_cli/gym.rs` dispatches `reliability-gate`
  (test `test_gym_reliability_gate_rejects_extra_args` locks the wiring).
