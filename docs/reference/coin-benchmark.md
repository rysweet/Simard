---
title: COIN benchmark reference
description: >-
  Canonical reference for the COIN (COde -> INput) code-reasoning benchmark: how
  to obtain, install, and run it locally; the exact agent submission contract and
  execution-graded scoring/verification pipeline; the dataset schema, splits, and
  digest-pinned runtime images; the full `coin` CLI surface; and a snapshot of the
  published leaderboard. This is the design-input contract the LOCAL COIN Gym
  harness (`src/coin_gym/`) is validated against.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../research/coin-benchmark.md
  - ../research/coin-benchmark-phase1.md
  - ../research/skwaq-self-improvement-loop.md
  - ../research/coin-benchmark-and-skwaq-study.md
  - ../../src/coin_gym/mod.rs
  - ../../src/coin_gym/executor.rs
  - ../../src/coin_gym/target_loader.rs
  - ../../src/coin_gym/scorer.rs
  - ../../src/coin_gym/leaderboard.rs
---

# COIN benchmark reference

> **Scope.** This is the canonical, maintained **reference** for the external
> COIN benchmark — the stable "look it up" contract that the LOCAL COIN Gym
> harness ([`src/coin_gym/`](https://github.com/rysweet/Simard/tree/main/src/coin_gym))
> is built and validated against. For narrative background and the Gym design
> sketch see the research spikes:
> [COIN Phase-1 primer](../research/coin-benchmark.md),
> [Phase-1 completion record](../research/coin-benchmark-phase1.md),
> [skwaq self-improvement loop](../research/skwaq-self-improvement-loop.md), and
> the combined [COIN & skwaq study](../research/coin-benchmark-and-skwaq-study.md).
> Unlike those time-stamped spikes, this page adds the **code-verified**
> submission and verification contracts extracted directly from the COIN source.

> **LOCAL-ONLY.** Simard uses COIN as a **local** measurement substrate. We never
> submit results to the public leaderboard and never provision external
> infrastructure to do so. The published numbers in
> [Leaderboard snapshot](#leaderboard-snapshot) are used only as a **local
> comparison baseline** by the Gym's leaderboard comparator.

## Contents

- [What COIN measures](#what-coin-measures)
- [Where to obtain it (repo, dataset, license)](#where-to-obtain-it)
- [Install](#install)
- [Run one task locally (recipe)](#run-one-task-locally)
- [Scoring mechanics (execution-graded)](#scoring-mechanics)
  - [The task](#the-task)
  - [The agent submission contract](#the-agent-submission-contract)
  - [The verification pipeline](#the-verification-pipeline)
  - [Metrics and per-target outcomes](#metrics-and-per-target-outcomes)
- [Dataset schema, splits, and runtime images](#dataset-schema)
- [Target construction (signal cube and families)](#target-construction)
- [`coin` CLI surface](#coin-cli-surface)
- [Leaderboard snapshot](#leaderboard-snapshot)
- [What this contract means for the LOCAL COIN Gym](#implications-for-the-coin-gym)
- [Sources](#sources)

## What COIN measures

**COIN (COde -> INput)** measures whether an LLM agent can map a program's
*semantic space* to its *input space*: given a **target line** in a real project
at a pinned commit, the agent must produce a concrete **input** that, when fed to
one of the project's fuzzing harnesses, drives execution **to that line**.

The defining property is that **execution decides**. There is no reference
answer, no multiple choice, and no LLM judge — a submission is graded by
**running the code** on a coverage-instrumented build and checking whether the
target line was covered. This makes the score:

- **Objective** — a binary reached / not-reached per target.
- **Contamination-resistant** — "what input reaches line `ℓ` at commit `c`?" is
  not text on the web; frontier targets have *never* been reached, so no solution
  can leak into training data.
- **Live / self-refreshing** — targets are regenerated from OSS-Fuzz coverage and
  upstream commits, so the benchmark cannot saturate.

Targets are drawn from **1000+ OSS-Fuzz projects across nine languages**. The
published snapshot pins **70 targets** from **7 projects**.

## Where to obtain it

| Artifact | Location |
|----------|----------|
| Landing page / leaderboard | <https://coin-bench.github.io/> |
| Source code (CLI + pipeline) | <https://github.com/COIN-Bench/coin> |
| Dataset (snapshots) | <https://huggingface.co/datasets/COIN-Bench/coin> |
| License | **MIT** (`Copyright (c) 2026 The COIN Authors`) |
| Built on | [OSS-Fuzz](https://google.github.io/oss-fuzz/) |

> The site's GitHub / Hugging Face buttons are populated at runtime from
> `assets/app.js` (`LINKS.code` / `LINKS.data`) and were, at capture time,
> internally labelled "to be published." The URLs above are those embedded
> values and were **verified reachable** (the repo exists, MIT-licensed; the
> dataset page renders the schema below). Re-verify availability before pinning
> a specific snapshot in the harness build.

The current published snapshot is **`COIN-Bench/coin` revision `v2026-07`**
(70 verified targets, 7 projects).

## Install

**Requirements:** Python **>= 3.12** and a working **Docker** daemon. Pipeline
stages 4-7 shell out to Docker; the eval agents run in privileged
Docker-in-Docker.

Recommended, with [uv](https://docs.astral.sh/uv/) (resolves the bundled
`corpusdb` sub-package automatically):

```bash
git clone https://github.com/COIN-Bench/coin && cd coin
uv sync --extra llm --extra dev   # core + LLM SDKs + test deps
```

With plain `pip`, install the bundled `corpusdb` local path dependency first
(pip cannot resolve it from PyPI):

```bash
pip install -e ./corpusdb
pip install -e .            # add ".[llm]" and/or ".[dev]" as needed
```

The CLI entry point is `coin` (equivalently `python -m coin`). Use
`coin --help` for the grouped command list and `coin <command> --help` for
flags.

**Configuration.** Copy the example config and provide credentials via a `.env`
or exported environment variables:

```bash
cp config.example.yaml config.yaml
```

| Task | Required environment |
|------|----------------------|
| Evaluate agents (stage 7) | `LITELLM_URL`, `LITELLM_MASTER_KEY` — the LiteLLM proxy the agents call |
| Publish a snapshot | `HF_TOKEN` (write) **and** `docker login ghcr.io` for the image push |

Because agents are driven through **LiteLLM**, the harness is **model-agnostic**:
swapping the agent under test is a config change, not a code change.

## Run one task locally

The intended "most users" path evaluates an agent against a **published
snapshot** — no pipeline run, no OSS-Fuzz tree. Each project is rebuilt from the
dataset row's pinned commits and gated by a functional precheck, falling back to
the digest-pinned runtime image only if a rebuild fails.

```bash
# 0) Prereqs: Python >=3.12, Docker running, `uv sync --extra llm` done,
#    LITELLM_URL / LITELLM_MASTER_KEY exported, config.yaml in place.

# 1) Evaluate an agent against ONE project from the published snapshot.
#    --split restricts the target set; --project restricts to a single project
#    so a first run touches one small target group instead of all 70.
coin evaluate \
  --dataset COIN-Bench/coin \
  --revision v2026-07 \
  --split codeql_only \
  --project cups \
  --source rebuild            # default; use --source image to force the prebuilt image

# 2) Verify submissions by replaying them against the harness (writes
#    `reached` back into each result.json). `evaluate` mints an experiment id;
#    pass it here (see the run's output / output/experiments/<id>/).
coin verify --experiment <experiment_id>

# 3) Inspect results in the dashboard (defaults to http://localhost:8765).
coin show
```

Notes captured from the CLI:

- `coin evaluate` accepts either `--dataset <hf-repo> --revision <tag>` (snapshot
  mode, mutually exclusive with `--experiment`) or `--experiment <id>` (local
  pipeline output). `--dataset you/coin@v1` is shorthand for
  `--dataset you/coin --revision v1`.
- `--split` is repeatable; omit it to run all splits.
- `--source rebuild` (default) rebuilds from the row's pins and gates on the
  functional precheck; `--source image` always pulls the digest-pinned image.
- `--retry-item <item_id>` re-runs a single named item (force-deletes its prior
  dir) and leaves all other items untouched.

Running the full pipeline from scratch (mining OSS-Fuzz coverage to build a new
dataset) is an advanced path — see [`coin` CLI surface](#coin-cli-surface).

## Scoring mechanics

This is the heart of the reference: **exactly** how a task is posed, how an agent
submits, and how the submission is graded by running the code.

### The task

A COIN task is a single **target line range** `ℓ` in a real project at commit
`c`. The agent is given (via a generated `TASK.md`): the project, the enclosing
function, the file and line range, a source snippet, git-clone instructions for
the pinned source, the OSS-Fuzz commit, and the list of available **harness
binaries** (mounted read-only at `/workspace/harnesses/`). The agent runs as
**root** in a Docker container with full internet and Docker access.

For **branch-frontier** targets the prompt additionally names the controlling
conditional and states which arm a baseline already took, framing the task as
"flip this branch so the never-executed arm runs." The submission and
verification contract is unchanged: *make the target line execute*.

### The agent submission contract

> Source: `coin/stages/stage7/evaluate/prompt.py` (the generated `TASK.md`).

When ready, the agent writes its **final answer** to the bind-mounted
`/answer/` directory:

1. **`/answer/blob.bin`** — the raw input **bytes** to feed the harness.
2. **`/answer/blob.harness`** — plain text, **one line**: the harness binary name
   (from the provided list) the blob should be fed to.

Critical properties of the contract:

- **No live reachability feedback.** There is no verification endpoint to call
  during the run. Whatever is in `/answer/` when the agent exits is final; it is
  replayed later. **Only the last write of each file counts.**
- **Abstention.** To abstain, write **only** `/answer/UNREACHABLE.md` with
  concrete evidence the line is unreachable by any input through any listed
  harness, and **do not** write `blob.bin`. If `blob.bin` exists it is treated as
  a normal attempt and `UNREACHABLE.md` is ignored. A wrong abstention (the line
  was in fact reachable) counts against the agent.
- **Tool-permissive.** Because there is no answer to memorize, the agent may use
  anything — web search, static analyzers, cloning the project source — without
  leaking ground truth. In the seed-tool track a `corpusdb` CLI and a
  pre-replayed per-harness seed corpus are additionally mounted at
  `/workspace/corpusdb/` as an optional hint; the submission contract is
  unchanged.

### The verification pipeline

> Source: `coin/stages/stage7/verify/runner.py` and
> `coin/stages/stage7/answer/verify_server.py`.

Evaluation is **decoupled** from verification. Agents produce `/answer/`
artefacts and exit; the reach check runs afterward as a separate batch step
(`coin verify`). Per project, the verifier:

1. Groups the project's submitted work items with their selected-target metadata
   (coverage keys come from stage 6's `targets_selected.jsonl`).
2. Starts **one `verify_server` container** per project with that project's
   **coverage-instrumented build** mounted, and registers all of the project's
   targets in one shot.
3. For each submitted work item, POSTs the blob to the server's **`/verify`**
   endpoint and asks whether the **named harness** reached the target line. The
   server runs the harness on the coverage build and parses the coverage output
   (llvm-cov HTML for C/C++/Rust, JaCoCo for JVM) to decide reached / not.
4. Writes **`reached`** (bool) back into the work item's `result.json`.
5. Tears the server down and moves to the next project.

The dashboard (`coin show`) reads `reached` from each `result.json` and rolls up
the verified hit rate — no further plumbing. The reach outcome is therefore a
**binary, objective** result of executing the project's own maintainer-written
harness on an instrumented binary.

### Metrics and per-target outcomes

Two metrics drive the headline (targeted-reachability) leaderboard:

- **Reach rate** — fraction of targets **provably reached**, verified by harness
  replay on the instrumented build.
- **Precision** — fraction of an agent's **submitted** inputs that **actually
  reached**. This exposes over-claiming: agents routinely submit inputs that do
  not reach the line. Even the best model's submissions are right only ~half the
  time (52.5%); the weakest lands 12.8%.

Each target's outcome (used in the per-target results matrix) is one of:

| Code | Outcome |
|------|---------|
| `R` | Reached (verified) |
| `W` | Submitted — wrong input (did not reach) |
| `A` | Abstained (`UNREACHABLE.md`) |
| `T` | Timed out |
| `N` | No submission |
| `E` | Error |

COIN defines **three tracks from a single target set**:

1. **Targeted reachability** (headline) — reach one specific line; reach rate +
   precision.
2. **Seed-tool ablation** — same targeted task, plus the saturated seed corpus +
   `corpusdb` query tool. Aggregate reach barely moves (19.5% -> 19.1%); it just
   reshuffles *which* lines get solved.
3. **Coverage maximization** — no target line; maximize whole-project coverage.
   Coverage is **not** a target proxy: on liboqs it touches 47.7% of lines yet
   hits **0** of 10 targets.

## Dataset schema

> Source: <https://huggingface.co/datasets/COIN-Bench/coin> (revision
> `v2026-07`).

Load a split with the `datasets` library:

```python
from datasets import load_dataset

ds = load_dataset("COIN-Bench/coin", revision="v2026-07", split="codeql_only")
print(ds[0]["target_id"], ds[0]["runtime_image"])
```

**Splits** are first-match-wins over a 6-bit signal cube (see
[Target construction](#target-construction)):

| Split | Cell | Rows |
|-------|------|-----:|
| `gcs_reachable` | `G and not (F or L)` | 35 |
| `codeql_only` | `C and not (G or F or L)` | 35 |

**Selected schema columns** (each row is one target):

| Column | Type | Meaning |
|--------|------|---------|
| `target_id` | str | `<project>:<harness>:<file>:<line_start>[-<line_end>]` |
| `coin_version` | str | snapshot tag (e.g. `v2026-07`) |
| `coin_commit` / `oss_fuzz_commit` | str | COIN + OSS-Fuzz commits pinned for the snapshot |
| `project` / `harness` | str | OSS-Fuzz project + primary reaching harness binary |
| `harness_binaries` | str (JSON list) | harness binary basenames in the project |
| `language` | str | `c` / `c++` / `rust` / `python` / `jvm` |
| `file`, `line_start`, `line_end`, `line_count` | str/int | canonical `/src/<project>/<rel>` path + target range |
| `function` | str | enclosing function (empty if not extracted) |
| `source_snippet` | str | +/- 100 lines around the target |
| `src_commits` | str (JSON list) | `[{path, rev, url}]` for git-cloning the source |
| `gcs_covered` | bool | `G` signal — line in the GCS coverage report |
| `baseline_reachable` / `agent_baseline_reachable` / `codeql_reachable` | str (JSON) | `F` / `L` / `C` signal evidence |
| `runtime_image` / `runtime_digest` | str | digest-pinned runtime image for the project |
| `target_text` / `prompt_preview` | str | rendered target description / prompt preview |

**Runtime images** are one per project, digest-pinned for reproducibility — the
dataset rows reference these digests directly, so reruns months later hit the
same binaries. The `v2026-07` snapshot pins images for: `cups`, `cyclonedds`,
`hdf5`, `karchive`, `liboqs`, `libraw`, `rdkit` (sizes ~2.1 GB to ~14.8 GB).

## Target construction

> COIN does not take targets *from* fuzzing coverage — it filters *against* it.

Reachability is established two ways, then **cheap, target-blind baselines are
subtracted** so that only genuinely hard lines remain. The signal letters:

| Group | Signal | Meaning |
|-------|:------:|---------|
| Reachability | `C` | **Static reach** — CodeQL call-graph proof a harness-to-target path exists |
| Reachability | `B` | **Untaken branch** — first line of an untaken branch/loop body adjacent to covered code |
| Reachability | `G` | **Long fuzzing** — line covered by OSS-Fuzz's continuous industrial-scale fuzzing |
| Baseline (subtracted) | `F` | **Short fuzzing** — fresh fixed-budget libFuzzer run in-experiment |
| Baseline (subtracted) | `L` | **LLM seed-gen** — LLM given the harness but *not* the target line |

Target **families** (what the two published splits sample from):

- **Frontier** — `(B ∪ C) \ (G ∪ F ∪ L)`: statically reachable but **never**
  covered by any fuzzer or baseline. The hardest, most contamination-resistant
  class (`codeql_only` split).
- **Non-trivial reachable** — `G \ (F ∪ L)`: reached by long-running fuzzing but
  missed by fresh fuzzers and goal-blind LLM seeds (`gcs_reachable` split).

The pipeline funnels **639,410** candidate uncovered lines (31,259 non-trivial
reachable + 7,048 branch-frontier candidates) down to **70** evaluable targets
(five per family across 7 projects). Because it is fully automated, the benchmark
refreshes itself as OSS-Fuzz and upstream code evolve — new commits open new
frontiers, so COIN cannot saturate.

## `coin` CLI surface

> Source: `coin/cli.py`. Commands are grouped; `coin <command> --help` for flags.

**Consume a published snapshot (most users):**

| Command | Purpose |
|---------|---------|
| `coin evaluate --dataset <repo> --revision <tag> [--split ...] [--project ...] [--source rebuild\|image]` | Run agent evaluation against a snapshot |
| `coin eval-tool` | Seed-tool ablation track (corpus + `corpusdb` mounted) |
| `coin eval-coverage` | Coverage-maximization track |
| `coin verify --experiment <id> [--max-concurrent N]` | Replay submissions; write `reached` to result.json |
| `coin show` | Dashboard over the results |

**Build a dataset from scratch (advanced, 8-stage pipeline):**

| Stage | Command | Purpose |
|:-----:|---------|---------|
| 1 | `coin gcs-sync --project <p>` | Sync OSS-Fuzz coverage |
| 2 | `coin select` | Pick projects -> mints a `build_id` |
| 3 | `coin prepare-build -b <build_id>` | Patch Dockerfiles |
| 4 | `coin build -b <build_id>` | Normal + coverage builds |
| 5 | `coin baseline -b <build_id>` / `coin agent-baseline -b <build_id>` / `coin codeql -b <build_id>` | Fuzzer, LLM seed-gen, and CodeQL reachability signals |
| 6 | `coin extract-targets -b <build_id>` | Select targets -> mints an `experiment_id` |
| 7 | `coin evaluate -e <experiment_id>` then `coin verify -e <experiment_id>` | Evaluate agents, then verify |
| 8 | `coin show` | Dashboard |

**Publish** (turn a finished experiment into a citable, immutable snapshot):

```bash
docker login ghcr.io
coin publish -e <experiment_id> --version v1 \
    --hf-repo you/coin --registry ghcr.io/you
```

Additional helpers include `coin reextract`, `coin cov-reconstruct`,
`coin cov-series-csv`, `coin cov-covered-set`, and `coin index-seeds`.

## Leaderboard snapshot

> Source: `assets/app.js` (`export/T2_main_targeted.csv`). Targeted-reachability
> track: **8 agents x 70 targets, 2-hour budget per target.** Columns: rank,
> model, scaffold, Reach %, Precision %, Reached (n/70), Frontier (n/35),
> `$/reached`, Total `$`. **Used LOCALLY only, as a comparison baseline.**

| # | Model | Scaffold | Reach % | Precision % | Reached | Frontier | $/reached | Total $ |
|---|-------|----------|--------:|------------:|--------:|---------:|----------:|--------:|
| 1 | Claude Opus 4.6 | Claude Code | 30.0 | 52.5 | 21/70 | 1/35 | 9.99 | 210 |
| 2 | Claude Sonnet 4.6 | Claude Code | 25.7 | 45.0 | 18/70 | 0/35 | 15.51 | 279 |
| 3 | Gemini 3.1 Pro | Gemini CLI | 24.3 | 51.5 | 17/70 | 1/35 | 4.12 | 70 |
| 4 | GPT-5.4 | Codex | 22.9 | 41.0 | 16/70 | 0/35 | 3.45 | 55 |
| 5 | GPT-5.4-mini | Codex | 18.6 | 31.0 | 13/70 | 0/35 | 1.29 | 17 |
| 6 | GLM-5 (open-weights) | Claude Code | 14.3 | 27.0 | 10/70 | 0/35 | 7.29 | 73 |
| 7 | Gemini 3 Flash | Gemini CLI | 12.9 | 15.3 | 9/70 | 0/35 | 5.53 | 50 |
| 8 | DeepSeek-V3.2 (open-weights) | Claude Code | 7.1 | 12.8 | 5/70 | 0/35 | 6.30 | 32 |

Headline findings:

- **The frontier is a wall.** Aggregate reach across all 8 agents on 35 frontier
  targets is **0.7%**, versus **38.2%** on non-trivial reachable targets.
- **The solved union is small.** **47/70** targets were solved by **no** model;
  only **1** by all eight; **23/70** solved by *any* model (Opus alone: 21).
- **Cost != capability.** GPT-5.4-mini is cheapest per success ($1.29) while
  Sonnet ($15.51) costs more than Opus ($9.99) despite reaching fewer targets.
- **Capability gap.** Flagships beat lightweight siblings (**25.7%** vs
  **19.1%**); commercial models average **22.4%** vs **10.7%** for open-weights.

## Implications for the COIN Gym

This contract is the design input for the LOCAL COIN Gym harness
([`src/coin_gym/`](https://github.com/rysweet/Simard/tree/main/src/coin_gym)).
The concrete obligations it places on the harness:

- **Executor** ([`executor.rs`](https://github.com/rysweet/Simard/blob/main/src/coin_gym/executor.rs))
  delegates grading to COIN's own oracle — it must drive `coin evaluate` (or the
  agent container directly) so that the agent-under-test writes exactly
  `/answer/blob.bin` + `/answer/blob.harness` (or `/answer/UNREACHABLE.md`), then
  runs `coin verify` and reads `reached` from each `result.json`. The Gym must
  **never** re-implement reach-checking; execution on the instrumented build is
  the only source of truth.
- **Target loader** ([`target_loader.rs`](https://github.com/rysweet/Simard/blob/main/src/coin_gym/target_loader.rs))
  parses the [dataset schema](#dataset-schema): `target_id`, `project`,
  `harness`, `file`/`line_start`/`line_end`, and `family` (frontier vs.
  non-trivial reachable), pinned by `revision`. A **held-out fresh** slice from a
  newer snapshot is the natural anti-overfit oracle.
- **Scorer** ([`scorer.rs`](https://github.com/rysweet/Simard/blob/main/src/coin_gym/scorer.rs))
  computes **reach rate** and **precision** overall and per family, plus the
  `R/W/A/T/N/E` histogram — precision matters because it punishes over-claiming,
  which is exactly what a low-confidence single-model baseline does.
- **Leaderboard comparator** ([`leaderboard.rs`](https://github.com/rysweet/Simard/blob/main/src/coin_gym/leaderboard.rs))
  diffs local reach/precision against the [snapshot](#leaderboard-snapshot) for
  the same model; a large deviation (e.g. local Opus 4.6 far from 30.0%) signals
  a harness/config bug, not a capability result.
- **Baseline vs. multi-agent team.** Because the contract is model-agnostic
  (LiteLLM) and precision penalizes over-claiming, the natural A/B is a single
  model vs. a debate team whose synthesizer abstains (`UNREACHABLE.md`) rather
  than submit a low-confidence blob.

## Sources

Verified directly (a generic web search for "COIN benchmark" returns several
unrelated projects sharing the acronym — none is this code-reasoning benchmark):

| Topic | Source |
|-------|--------|
| Task, construction, leaderboard, run commands | <https://coin-bench.github.io/> (page + `assets/app.js`, `assets/matrix.js`) |
| CLI, install, submission + verification contract | <https://github.com/COIN-Bench/coin> — `README.md`, `coin/cli.py`, `coin/stages/stage7/evaluate/prompt.py`, `coin/stages/stage7/verify/runner.py` |
| Dataset schema, splits, runtime images | <https://huggingface.co/datasets/COIN-Bench/coin> (revision `v2026-07`) |
| Simard research background + Gym design | [primer](../research/coin-benchmark.md) · [completion record](../research/coin-benchmark-phase1.md) · [skwaq loop](../research/skwaq-self-improvement-loop.md) · [study](../research/coin-benchmark-and-skwaq-study.md) |
