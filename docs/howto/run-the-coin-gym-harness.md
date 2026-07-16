---
title: Run the LOCAL COIN Gym harness
description: Operator guide for the coin-gym CLI — a local harness that runs the COIN benchmark shape, scores vs. the published leaderboard, and A/Bs a single-model baseline against a multi-agent team.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../research/coin-benchmark-and-skwaq-study.md
  - ../research/coin-gym-baseline-vs-team-measurement.md
---

# Run the LOCAL COIN Gym harness

The **COIN Gym** is a local harness for the [COIN](https://coin-bench.github.io/)
benchmark (COde → INput): it drives an agent to produce an input that reaches a
target line, grades the result, scores reach/precision against the published
leaderboard, and compares a **single-model baseline** against a **multi-agent
team** — mirroring skwaq's failure-analysis + overfitting-reviewer gating.

This guide covers **Phase 4** (the local scaffold) and the **Phase 5**
self-improvement loop (`improve --holdout fresh`). The full design and the phase
plan live in
[COIN benchmark & skwaq gym study](../research/coin-benchmark-and-skwaq-study.md);
Phase 5 is tracked in issue #2825.

> **Offline by default.** In Phase 4 the harness grades against a **mock oracle**
> so the whole pipeline runs without a VM. Runs are clearly labelled
> `OFFLINE SCAFFOLD`. Real grading delegates to `coin evaluate` (Docker +
> instrumented replay) and needs a provisioned host — that is **Phase 3**
> (#2823). The **Phase 5** self-improvement loop (apply → verify on held-out
> fresh targets → keep-or-roll-back, with durable tactic memory) runs the same
> way — offline against the mock oracle — behind `improve --holdout fresh`; a
> real held-out grade comes from `coin verify` on the Phase-3 VM.

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
coin-gym improve <run-id> [--profile <name>] [--holdout fresh]
coin-gym contract [--dataset <repo>] [--revision <tag>] [--split a,b] [--project x,y] [--source rebuild|image]
coin-gym verify
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

For the per-target breakdown, the reproduction commands, and the leaderboard-
comparison caveat, see
[COIN Gym — baseline vs. team measurement](../research/coin-gym-baseline-vs-team-measurement.md).

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

Runs the **offline slice** of the self-improvement loop over a saved run:

1. **Failure-analyst** turns each unreached target (`W`/`T`/`N`) into a
   **general** reachability tactic (e.g. "for format-gated decoders, satisfy the
   magic-byte/header validator before targeting deep lines").
2. **Overfitting-reviewer gate** rejects any tactic that memorises a specific
   input or keys off a specific target id / project / locator, accepting only
   tactics that plausibly generalise.

This is the analysis-only view; it does **not** apply, verify, or roll back
tactics. For that, add `--holdout fresh`.

### `improve --holdout fresh` — the Phase-5 self-improvement loop

```bash
# A run persists its offline scaffold (oracle + script), which the loop needs.
coin-gym run "Claude Opus 4.6" --targets my_snapshot.json --profile loop
coin-gym improve <run-id> --profile loop --holdout fresh
```

Runs the **live loop** (Phase 5, #2825), mirroring skwaq's
`failure-analyst → overfitting-reviewer → verify` cycle:

1. **Analyse + gate** the run's failures into general tactics (as above);
   memorising / target-specific tactics are rejected *before* verification.
2. **Apply + measure on held-out fresh targets.** Each accepted tactic is applied
   and the agent is re-run on the snapshot's **held-out fresh** slice — targets
   the tactic's motivating failure never saw. The tactic is **kept iff held-out
   reach improves and precision does not drop**; otherwise it is **rolled back**.
   (Offline, the held-out grade is synthesised from the mock oracle — see the
   note below.)
3. **Train/held-out-gap warning** (the issue's "overfitting-warning"). If a tactic
   lifts *training* reach but not *held-out* reach, the gap is flagged and the
   tactic is rolled back as **UNPROVEN**; a definitive overfit-vs-coverage verdict
   is left to the Phase-3 verifier.
4. **Durable tactic memory.** Kept tactics are persisted per **general family**
   (never per project/target — that would be overfitting) to
   `<home>/profiles/<name>/tactics.json` and **reused** on subsequent runs.

On the bundled `improve_loop_snapshot.json` fixture — pinned failures across a
decoder, a crypto state machine, and a generic guard, with a held-out slice that
covers the decoder + crypto families but **not** the generic one — one cycle
keeps the two decoder/crypto tactics, rolls back the generic one, and warns:

```text
gate:     3 accepted  0 rejected
holdout:  reach 0.0% → 100.0%   (kept 2, rolled back 1, train/held-out-gap warnings 1)
memory:   0 → 2 durable tactic(s)
  [KEEP]     dec-a (format-gated-decoder) — held-out reach 0.0% → 50.0% …
  [KEEP]     cry-a (crypto-state-machine) — held-out reach 50.0% → 100.0% …
  [ROLLBACK] gen-a (generic) — train/held-out reach GAP: lifts TRAINING reach but no held-out gain; rolled back as UNPROVEN
```

A second `improve --holdout fresh` **reuses** the banked tactics: the held-out
baseline already reaches 100%, nothing new is banked (`memory: 2 → 2`), and the
memorisation-resistant design never double-counts a family.

> **Offline scaffold — an *idealized* effect model, honestly.** The held-out
> grade here is synthesised from the **same mock oracle**: applying a tactic of
> family `F` is *assumed* to produce the oracle's reaching input for every
> in-scope target of `F` (including held-out ones). This exercises the loop's
> **control flow** — analyse → gate → apply → measure held-out → keep/rollback +
> durable memory — but it does **not** prove the tactic *text* would solve fresh
> targets. A train/held-out gap is therefore reported as **UNPROVEN** (a coverage
> gap), not a definitive overfit verdict. **Real** empirical held-out
> verification — a live model graded by `coin verify` — is **Phase 3** (#2823).
> **LOCAL-ONLY**: nothing is ever submitted externally, and the stored oracle is
> a test double, never a real verdict source.

### `contract` — show the real `coin evaluate` / `coin verify` wiring

```bash
coin-gym contract
coin-gym contract --dataset COIN-Bench/coin --revision v2026-07 \
  --split codeql_only --project cups --source image
```

Prints — **without running anything** — exactly how the harness drives COIN's
own oracle for a snapshot (issue #3001):

```text
LOCAL-ONLY: true (no external submission, no leaderboard entry, no VM provisioning)
evaluate: coin evaluate --dataset COIN-Bench/coin --revision v2026-07 --source rebuild
verify:   coin verify --experiment <experiment-id>
submission-contract:
  attempt:  /answer/blob.bin + /answer/blob.harness
  abstain:  /answer/UNREACHABLE.md  (and NO blob.bin)
verdict:  read `reached` from each result.json (never re-checked locally)
```

This is the **code-verified contract** from
[`docs/reference/coin-benchmark.md`](../reference/coin-benchmark.md): the Gym
never re-implements reach-checking — `coin verify` writes `reached` into each
`result.json` and the harness only reads it. `--split` / `--project` narrow the
target set (comma-separate to repeat), and `--source` picks rebuild-vs-image.
The executor (`src/coin_gym/executor.rs`) builds this exact argv, writes the
`/answer/` submission per the contract, and reads back `reached`; the live
Docker invocation itself is gated behind Phase 3.

### `verify` — LOCAL harness acceptance self-check (the done-gate)

```bash
coin-gym verify
```

Runs the **measurable done-criteria** for the LOCAL harness offline against the
built-in sample snapshot and prints a PASS/FAIL matrix. It exercises every
component of the design (issue #2713) and asserts a concrete postcondition for
each, then **exits non-zero if any criterion fails** — so it is a runnable,
CI-friendly done-gate rather than a subjective judgement:

```text
coin-gym verify — LOCAL harness acceptance self-check
snapshot: built-in sample (offline mock oracle)
  [PASS] target-loader            5 pinned + 2 held-out-fresh target(s); both families present
  [PASS] baseline-runner          5 outcome(s) for 5 pinned target(s)
  [PASS] team-runner              5 outcome(s) for 5 pinned target(s)
  [PASS] scorer                   reach 60.0% / precision 60.0%; 2 family split; histogram covers 5/5 outcome(s)
  [PASS] leaderboard-comparator   compared vs published 'GPT-5.4' (reach Δ +37.1 pts, material-deviation=true)
  [PASS] self-improvement-loop    held-out reach 0.0% → 100.0% (kept 2, rolled back 0); memory 0 → 2 tactic(s)
  [PASS] contract-wiring          evaluate (8 args) + verify (4 args) argv present; LOCAL-ONLY=true
result: 7/7 criteria passed
scope: LOCAL offline harness only. …
```

| Criterion | Measured postcondition |
|-----------|------------------------|
| `target-loader` | pinned **and** held-out-fresh slices non-empty; both families present |
| `baseline-runner` | exactly one graded outcome per pinned target |
| `team-runner` | exactly one graded outcome per pinned target |
| `scorer` | bounded reach/precision, per-family split, histogram covers every outcome |
| `leaderboard-comparator` | a published model diffs against its leaderboard row |
| `self-improvement-loop` | held-out reach does not regress; durable tactic memory never shrinks |
| `contract-wiring` | non-empty `coin evaluate` / `coin verify` argv (LOCAL-ONLY) |

The self-check is hermetic and deterministic: it uses a throwaway temp home for
tactic memory and never touches your real profiles. Live VM grading (Phase 3,
#2823) is **externally gated** and intentionally out of this gate's scope.

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
For `improve --holdout fresh`, include a non-empty `held_out_fresh` slice **and**
`oracle` entries covering it, so the loop has fresh lines to verify tactics
against (see `src/coin_gym/fixtures/improve_loop_snapshot.json`).

> **Real COIN dataset schema.** The compact manifest above is the offline-demo
> shape. The library also parses COIN's **published** dataset schema — rows with
> `target_id` (`<project>:<harness>:<file>:<line_start>[-<line_end>]`),
> `coin_version`, `project`, `harness`, `file`, `line_start`/`line_end`, and a
> `split` (`codeql_only` → frontier, `gcs_reachable` → non-trivial-reachable) —
> via `DatasetSource` in `src/coin_gym/target_loader.rs`, pinned by `revision`
> and reserving a held-out fresh slice as the anti-overfit oracle.

## Done-criteria for the LOCAL goal

The LOCAL COIN Gym goal
(`build-a-local-coin-benchmark-harness-and-a-self-improvement-loop`, issue #2713)
is **done** when both of the following hold — this is deliberately measurable so
an operator (or the OODA loop) can certify completion instead of stalling:

1. **`coin-gym verify` exits 0** — every LOCAL harness component passes its
   acceptance criterion (see the table above). This is the machine-checkable
   done-gate for Phases 4 and 5.
2. **Phase 3 (live VM grading) is acknowledged as externally gated.** Provisioning
   an `azlin` VM + Docker host and running `coin evaluate` / `coin verify` live is
   HIGH-RISK and operator-gated (#2823). It is **out of scope** for the LOCAL
   goal's done-gate and is tracked as a separate follow-up; the LOCAL goal does
   not block on it.

Phases 1–2 (research, PR #2712) and Phases 4–5 (this harness) are complete;
`coin-gym verify` keeps that verifiable at any time. What remains is the gated
Phase-3 follow-up below.

## What is deferred

| Phase | Work | Status |
|-------|------|--------|
| 3 | Provision an `azlin` VM + Docker host and pull a COIN snapshot, then run the **already-wired** `coin evaluate` / `coin verify` executor live (see `contract`) | follow-up (HIGH-RISK, operator-gated; #2823) |
| 5 | Live self-improvement loop: apply tactic → verify on held-out fresh targets → keep-or-roll-back; durable tactic memory | **implemented offline** (`improve --holdout fresh`, #2825) |

Phase 5 lands the loop **offline** against the mock oracle; a **real** held-out
grade (and therefore a real leaderboard delta) depends on the Phase-3 `azlin` VM
(#2823), which stays the critical path for the final done-gate. The **executor
contract** itself — the `coin evaluate` / `coin verify` argv, the
`/answer/blob.bin` + `/answer/blob.harness` (or `/answer/UNREACHABLE.md`)
submission, and reading `reached` from each `result.json` — is implemented and
unit-tested offline (issue #3001); only the live Docker invocation is gated
behind Phase 3.
