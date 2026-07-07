---
title: Run the LOCAL COIN Gym harness
description: Operator guide for the coin-gym CLI — a local harness that runs the COIN benchmark shape, scores vs. the published leaderboard, and A/Bs a single-model baseline against a multi-agent team.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../research/coin-benchmark-and-skwaq-study.md
---

# Run the LOCAL COIN Gym harness

The **COIN Gym** is a local harness for the [COIN](https://coin-bench.github.io/)
benchmark (COde → INput): it drives an agent to produce an input that reaches a
target line, grades the result, scores reach/precision against the published
leaderboard, and compares a **single-model baseline** against a **multi-agent
team** — mirroring skwaq's failure-analysis + overfitting-reviewer gating.

This guide covers **Phase 4** (the local scaffold). The full design and the
phase plan live in
[COIN benchmark & skwaq gym study](../research/coin-benchmark-and-skwaq-study.md)
and are tracked in issue #2713.

> **Offline by default.** In Phase 4 the harness grades against a **mock oracle**
> so the whole pipeline runs without a VM. Runs are clearly labelled
> `OFFLINE SCAFFOLD`. Real grading delegates to `coin evaluate` (Docker +
> instrumented replay) and needs a provisioned host — that is **Phase 3**. The
> live self-improvement loop (apply → verify on held-out fresh targets →
> keep-or-roll-back) is **Phase 5**.

## Build the CLI

```bash
cargo build --bin coin-gym
```

The binary reads/writes state under a home directory: `target/coin-gym` by
default, overridable with the `COIN_GYM_HOME` environment variable.

## Commands

```text
coin-gym run <model> [--strategy baseline|team] [--profile <name>] [--targets <path>]
coin-gym score   <run-id> [--profile <name>]
coin-gym compare <run-id> [--profile <name>]
coin-gym improve <run-id> [--profile <name>]
coin-gym profiles
```

### `run` — evaluate a model on the target set

```bash
coin-gym run claude-opus-4.6 --strategy baseline --profile opus
coin-gym run claude-opus-4.6 --strategy team     --profile opus-team
```

`run` loads the bundled sample snapshot (or a JSON snapshot passed with
`--targets`), drives the chosen strategy over the pinned targets, grades each
submission, saves a run under the profile, and prints the score. The two
strategies make the design's central trade-off explicit:

- **`baseline`** — a single model submits its candidate input directly.
- **`team`** — a *reacher* proposes an input, a *skeptic* challenges the
  over-claim, and a *synthesizer* **submits or abstains** via a `threshold_hint`
  gate. Because COIN **precision** punishes over-claiming, the team abstains on
  low-confidence inputs instead of submitting wrong ones.

On the bundled sample the two strategies reach the same number of targets, but
the team's abstention gate lifts precision from 60% to 100%:

```text
baseline  reach 60.0% (3/5)   precision 60.0% (3/5)   R:3/W:2/A:0/T:0/N:0/E:0
team      reach 60.0% (3/5)   precision 100.0% (3/3)  R:3/W:0/A:2/T:0/N:0/E:0
```

### `score` — reach / precision + family split

```bash
coin-gym score <run-id>
```

Prints overall **reach rate** and **precision**, the split by family (frontier
vs. non-trivial reachable), and the `R/W/A/T/N/E` outcome histogram.

- **reach rate** = reached / total targets.
- **precision** = reached / *submitted* inputs (abstain and no-submission are
  excluded from the denominator).

### `compare` — local vs. published leaderboard

```bash
coin-gym compare <run-id>
```

Diffs the run's reach/precision against COIN's published targeted-track numbers
for the same model. A gap beyond 10 percentage points is flagged as a **material
deviation** — a signal of a harness/config bug rather than a capability result.
Offline scaffold runs are labelled *illustrative only* (a mock oracle cannot
reproduce leaderboard numbers).

### `improve` — offline failure analysis + overfitting gate

```bash
coin-gym improve <run-id>
```

Runs the **Phase-4 slice** of the self-improvement loop over a saved run:

1. **Failure-analyst** turns each unreached target (`W`/`T`/`N`) into a
   **general** reachability tactic (e.g. "for format-gated decoders, satisfy the
   magic-byte/header validator before targeting deep lines").
2. **Overfitting-reviewer gate** rejects any tactic that memorises a specific
   input or keys off a specific target id / project / locator, accepting only
   tactics that plausibly generalise.

Applying an accepted tactic, re-running on held-out **fresh** targets, and
keeping it only if reach improves without a precision regression (else rolling
back) requires live grading and is **Phase 5**.

### `profiles` — list isolated per-model run state

```bash
coin-gym profiles
```

Each profile is an isolated directory (`<home>/profiles/<name>/`) holding its own
metadata and saved runs, so baseline-vs-team and model-vs-model comparisons never
cross-contaminate.

## Use a custom snapshot

`--targets <path>` points `run` at a JSON snapshot manifest with this shape (see
`src/coin_gym/fixtures/sample_snapshot.json` for a complete example):

```json
{
  "snapshot": "you/coin@v1",
  "targets": {
    "pinned":         [ { "id": "…", "project": "…", "commit": "…", "harness": "…", "file": "…", "line": 1, "family": "frontier" } ],
    "held_out_fresh": [ { "id": "…", "project": "…", "commit": "…", "harness": "…", "file": "…", "line": 2, "family": "non-trivial-reachable" } ]
  },
  "oracle": { "<target-id>": "<reaching input>" },
  "script": { "<target-id>": { "input": "<candidate>", "confidence": 0.8, "rationale": "…" } }
}
```

The `oracle` and `script` sections drive the **offline** demo run only. A real
run gets its oracle from `coin evaluate` and its candidates from a live model.

## What is deferred

| Phase | Work | Status |
|-------|------|--------|
| 3 | Provision an `azlin` VM + Docker host, pull a COIN snapshot, wire the real `coin evaluate` executor | follow-up (HIGH-RISK, operator-gated) |
| 5 | Live self-improvement loop: apply tactic → verify on held-out fresh targets → keep-or-roll-back; durable tactic memory | follow-up |

Both remain unchecked on issue #2713.
