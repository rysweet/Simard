---
title: "COIN Gym: single-model baseline vs. multi-agent team measurement"
description: >
  The reproducible reference measurement for the LOCAL COIN Gym harness: how the
  single-model baseline arm compares to the multi-agent team arm on the bundled
  sample target set, why the team's abstention gate lifts precision, and the
  exact commands to reproduce it. A durable reference of a unit-tested harness
  property — LOCAL-ONLY, offline-scaffold; a real leaderboard grade is Phase 3.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: reference
status: reference
related:
  - ../howto/run-the-coin-gym-harness.md
  - ./coin-benchmark-and-skwaq-study.md
  - ./coin-benchmark.md
---

# COIN Gym: single-model baseline vs. multi-agent team measurement

The LOCAL [COIN Gym](../howto/run-the-coin-gym-harness.md) harness exists to
answer one question empirically: **does a multi-agent team beat a single-model
baseline on the COIN target-reachability task?** This reference documents the
answer the harness produces on its bundled sample target set, *why* it produces
it, and exactly how to reproduce it. It is the durable companion to the
[operator how-to](../howto/run-the-coin-gym-harness.md) and the
[COIN benchmark & skwaq gym study](./coin-benchmark-and-skwaq-study.md).

> **Scope and honesty.** This is a **durable reference** of a reproducible,
> unit-tested harness property — not a one-off finding. The numbers below are
> deterministic (fixed fixture + mock oracle), pinned by the unit test
> `execute_run_baseline_vs_team_shows_precision_tradeoff`
> (`src/coin_gym/tests_cli.rs`), and are expected to be **updated by any future
> PR that changes the strategies or the sample fixture**. The measurement runs
> **offline** against a mock oracle (Phase 4) — it exercises the harness's
> control flow and the precision/abstention design, **not** a live model on a
> real COIN snapshot. A real grade delegates to `coin evaluate` / `coin verify`
> on a provisioned Docker host and is **Phase 3** (issue #2823). **LOCAL-ONLY:**
> nothing here is ever submitted externally or entered on any leaderboard.

## The two arms

Both arms run over the **same** pinned targets in
`src/coin_gym/fixtures/sample_snapshot.json` (5 targets: 2 `frontier`,
3 `non-trivial-reachable`). They differ only in *how* a candidate reaching-input
is decided before it is graded:

- **`baseline`** — a single model submits its candidate input directly. It never
  declines: every target yields a submission.
- **`team`** — a *reacher* proposes an input, a *skeptic* challenges the
  over-claim, and a *synthesizer* **submits or abstains** through a
  `threshold_hint` gate. Because COIN **precision** = reached / *submitted*
  punishes over-claiming, the team abstains on low-confidence inputs rather than
  submit wrong ones.

## Reference result (bundled sample target set)

| Arm | Reach | Precision | Outcome histogram |
|-----|-------|-----------|-------------------|
| `baseline` (single model) | 60.0% (3/5) | 60.0% (3/5)  | `R:3/W:2/A:0/T:0/N:0/E:0` |
| `team` (multi-agent)      | 60.0% (3/5) | 100.0% (3/3) | `R:3/W:0/A:2/T:0/N:0/E:0` |

**Both arms reach the same 3 of 5 targets. The team's abstention gate lifts
precision from 60% to 100% (+40 percentage points) with no loss of reach** — the
central trade-off the harness is built to make observable. Reach is unchanged
because abstaining never *reaches* a target the model would otherwise have
reached; it only removes wrong submissions from the precision denominator.

- **reach rate** = reached / total targets.
- **precision** = reached / *submitted* inputs (abstain and no-submission are
  excluded from the denominator), which is what exposes over-claiming.

### Why the arms diverge (per-target)

The divergence is isolated to the two targets whose scripted candidate carries
low confidence. On those, the baseline submits a *wrong* input (`W`) while the
team *abstains* (`A`); on the three high-confidence targets both arms submit and
reach (`R`):

| Target | Family | Script confidence | `baseline` | `team` |
|--------|--------|------------------:|:----------:|:------:|
| `libraw-fuji-480`    | frontier              | 0.82 | `R` | `R` |
| `libraw-crx-221`     | non-trivial-reachable | 0.71 | `R` | `R` |
| `harfbuzz-shape-540` | non-trivial-reachable | 0.66 | `R` | `R` |
| `liboqs-kem-88`      | non-trivial-reachable | 0.44 | `W` | `A` |
| `zstd-huf-1207`      | frontier              | 0.35 | `W` | `A` |

The per-family split shows the same effect within each family: the team holds
precision at 100% for both `frontier` (n=2) and `non-trivial-reachable` (n=3)
while the baseline drops to 50%/66.7% respectively.

## Reproduce it

```bash
cargo build --bin coin-gym

# Isolate state so the run is clean and repeatable.
export COIN_GYM_HOME="$(pwd)/target/coin-gym-reference"
rm -rf "$COIN_GYM_HOME"

# Both arms over the identical bundled sample target set.
coin-gym run "Claude Opus 4.6" --strategy baseline --profile ref-baseline
coin-gym run "Claude Opus 4.6" --strategy team     --profile ref-team
```

Each `run` prints the arm's score directly; `coin-gym score <run-id>` re-prints
it for a saved run and `coin-gym compare <run-id>` diffs it against the published
leaderboard (see below). `coin-gym leaderboard` ranks the two saved arms against
**each other** — the LOCAL standings — and prints the best-of-arm verdict
(`multi-agent team CLIMBS ABOVE the single-model baseline …`), the direct
operator view of the +40-point precision gain documented here. The exact
reference numbers above are also asserted by
`src/coin_gym/tests_cli.rs::execute_run_baseline_vs_team_shows_precision_tradeoff`,
so `cargo test -p simard coin_gym` fails if the harness ever stops reproducing
them.

> **Provenance.** These reference figures were last reproduced end-to-end by
> building and running the `coin-gym` CLI at commit
> `ad4c8032ff84fc947af7b3123ec5a052275744e6` (the tip of `main` on 2026-07-08,
> after Phases 1/2/4/5 merged: PRs #3038 and #3075, issue #3001 closed). Because
> the run is deterministic, any commit at which the strategies behave as designed
> reproduces the same table.

## Leaderboard comparison is illustrative only

`coin-gym compare` diffs the local run against COIN's published targeted-track
numbers for the same model (Claude Opus 4.6 published at reach 30.0%, precision
52.5%). On this **offline scaffold** run the tool prints the deltas but labels
them *illustrative only*:

```text
reach:     local 60.0%  vs published 30.0%  (Δ +30.0 pts)
precision: local 60.0%  vs published 52.5%  (Δ  +7.5 pts)   # baseline arm
material-deviation: YES
note: offline scaffold run (mock oracle) — deltas are illustrative only; real
      comparison requires a `coin evaluate` grade on a pinned snapshot (Phase 3)
```

A mock oracle cannot reproduce leaderboard numbers, so the `material-deviation`
flag here is expected and carries **no capability meaning**. The flag only
becomes a real harness/config signal once the run is graded live by
`coin evaluate` on a pinned snapshot (Phase 3). Do **not** read the offline
deltas as "the Gym outperforms the leaderboard".

## What this does and does not establish

- **Does establish (offline, control-flow level):** the multi-agent team's
  abstention gate is wired end-to-end and, on the bundled fixture, converts the
  baseline's two over-claims into abstentions — a measurable +40-point precision
  gain at equal reach. The effect is deterministic and unit-tested.
- **Does not establish (needs Phase 3):** that a *live* model in a team scaffold
  beats a *live* single-model baseline on a *real* COIN snapshot. That requires
  `coin evaluate` / `coin verify` grading on a provisioned Docker host
  (issue #2823) and a live-model candidate source, not the mock oracle.

## See also

- [Run the LOCAL COIN Gym harness](../howto/run-the-coin-gym-harness.md) — the
  operator how-to for every `coin-gym` command.
- [COIN benchmark & skwaq gym study](./coin-benchmark-and-skwaq-study.md) — the
  design behind the baseline-vs-team split and the skwaq-style gating.
- [COIN benchmark — Phase 1 primer](./coin-benchmark.md) — what COIN measures and
  how reach/precision are defined.
