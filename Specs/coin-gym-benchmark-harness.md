# LOCAL COIN Gym Benchmark Harness — Done-Gate Specification

## Purpose

The goal **"build a local coin benchmark harness and a self-improvement loop"**
(slug `build-a-local-coin-benchmark-harness-and-a-self-09e65e35`) stayed
`Blocked` cycle after cycle with the same diagnosis: **no tracked PR/issue the
done-gate could verify** (why = `UNCLEAR-CRITERIA`). The blocker was **not**
technical — the harness and the self-improvement loop were already built and
green. The blocker was that the goal's finish condition had **no machine-checkable
definition**, so every cycle re-observed it as unfinished and produced `NO ACTION`.

This spec fixes that WHY. It makes the done-criteria **measurable** by binding
the goal's finish condition to a **single command a daemon can run and score
automatically**:

```
scripts/check-coin-gym-done-gate.sh
```

The command exits `0` only when the LOCAL COIN Gym harness **and** its Phase-5
self-improvement loop pass their built-in acceptance self-check; otherwise it
exits non-zero and prints the failing criteria. This turns "built a local coin
benchmark harness and a self-improvement loop" from a prose judgement into a
check the done-gate can confirm — so the goal is certified complete the moment
the command passes, and never before.

## What "the LOCAL COIN Gym harness" is here

The COIN Gym is Simard's **local harness** for the
[COIN](https://coin-bench.github.io/) benchmark (COde → INput): it drives an
agent to produce an input that reaches a target line, grades the result, scores
reach/precision against the published leaderboard, and A/Bs a single-model
**baseline** against a multi-agent **team** — mirroring skwaq's failure-analysis
+ overfitting-reviewer gating.

| Layer | Location |
|-------|----------|
| CLI entry point | [`src/bin/coin_gym.rs`](../src/bin/coin_gym.rs) |
| Harness + self-improvement loop | [`src/coin_gym/`](../src/coin_gym/) |
| Built-in acceptance self-check (`coin-gym verify`) | [`src/coin_gym/mod.rs`](../src/coin_gym/mod.rs) `run_acceptance_checks` |
| Operator guide | [`docs/howto/run-the-coin-gym-harness.md`](../docs/howto/run-the-coin-gym-harness.md) |
| Outside-in scenarios | [`tests/gadugi/coin-gym-harness.sh`](../tests/gadugi/coin-gym-harness.sh), [`tests/gadugi/coin-gym-self-improve.sh`](../tests/gadugi/coin-gym-self-improve.sh) |

The harness is **offline by default**: Phase 4 grades against a mock oracle so
the whole pipeline runs with no VM, Docker, or network. The Phase-5
self-improvement loop (`coin-gym improve --holdout fresh`) runs a full cycle
(failure-analyst → overfitting-reviewer gate → apply → verify on held-out fresh
→ keep/rollback) offline.

## Scope boundary

This goal covers the **LOCAL offline harness and the self-improvement loop**.
**Live VM grading** (the real `coin evaluate` / `coin verify` on a provisioned
Docker host) is **Phase 3**, externally gated on issue #2823, and is
**out of scope** for this goal's done-gate.

## Measurable done-criteria

The goal is DONE when every criterion below passes. Each is asserted by the
built-in `coin-gym verify` acceptance self-check
([`run_acceptance_checks`](../src/coin_gym/mod.rs)) and re-asserted end-to-end by
the two gadugi scenarios.

| ID | Criterion | Checked by |
|----|-----------|-----------|
| CG-1 | **target-loader** — pinned + held-out-fresh target slices load; both `Frontier` and `NonTrivialReachable` families present | `coin-gym verify` |
| CG-2 | **baseline-runner** — single-model baseline produces exactly one graded outcome per pinned target | `coin-gym verify` |
| CG-3 | **team-runner** — multi-agent team produces one graded outcome per pinned target | `coin-gym verify` |
| CG-4 | **scorer** — reach/precision, family split, and outcome histogram computed over all outcomes | `coin-gym verify` |
| CG-5 | **leaderboard-comparator** — a run is scored against a published leaderboard entry with a reach delta | `coin-gym verify` |
| CG-6 | **self-improvement-loop** — held-out reach lifts via kept tactics; overfitting-only tactics are rolled back; kept tactics bank to durable memory | `coin-gym verify`, `tests/gadugi/coin-gym-self-improve.sh` |
| CG-7 | **contract-wiring** — the executor can build non-empty `coin evaluate` / `coin verify` argv under the LOCAL-ONLY guardrail | `coin-gym verify`, `tests/gadugi/coin-gym-harness.sh` |

## Definition of "done" (the done-gate)

The harness-and-self-improvement-loop goal is **done** when this single command
exits `0`:

```
scripts/check-coin-gym-done-gate.sh
```

It builds the `coin-gym` binary and runs `coin-gym verify`, which executes all
seven CG-* criteria against the built-in sample snapshot (offline mock oracle)
and exits non-zero if any criterion fails. This is the concrete artifact the
goal's done-criteria points at — the done-gate can run it every cycle and
certify the goal the moment it passes.

Optionally, `scripts/check-coin-gym-done-gate.sh --full` additionally runs the
two outside-in gadugi scenarios for a full end-to-end confirmation.

## Progress log

- **2026-07-18** — Bound the goal's finish condition to the machine-checkable
  `coin-gym verify` self-check via `scripts/check-coin-gym-done-gate.sh`. Local
  run: **7/7 criteria PASS, exit 0** (target-loader, baseline-runner,
  team-runner, scorer, leaderboard-comparator, self-improvement-loop,
  contract-wiring). The harness and self-improvement loop are delivered; the
  done-gate can now observe and certify the goal instead of re-stalling on
  unmeasurable criteria.
