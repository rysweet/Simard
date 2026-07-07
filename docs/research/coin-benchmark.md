# COIN benchmark — Phase 1 primer ("learn the benchmark")

**Status:** research spike — Phase 1 (LEARN COIN) of the LOCAL COIN Gym goal.
**Scope:** local study only. This note does **not** provision any VM and does
**not** post COIN results anywhere externally.

This is the focused Phase-1 primer. It answers **exactly four questions** with
concrete, verified detail:

1. **What** COIN measures.
2. **How** to obtain, install, and run it locally.
3. **How** it scores.
4. **How** its leaderboard is structured — and why our use of it is
   **LOCAL-ONLY**.

For the deeper study (target-construction signal algebra, the skwaq gym, and a
LOCAL "COIN Gym" design sketch) see the companion document,
[COIN benchmark & skwaq gym study](coin-benchmark-and-skwaq-study.md). This
primer is deliberately self-contained on the four questions above.

> **Naming caution.** A generic web search for "COIN benchmark" returns several
> unrelated projects that share the acronym. The code-reasoning benchmark
> studied here is the one at <https://coin-bench.github.io/>, code at
> <https://github.com/COIN-Bench/coin>. Use those canonical entry points.

## Sources (verified directly)

| Topic | Source | Verified |
|-------|--------|----------|
| Task, scoring, leaderboard, run commands | <https://coin-bench.github.io/> + `assets/app.js` | fetched |
| Code, install/run README, CLI | <https://github.com/COIN-Bench/coin> (commit `83261b0`) | cloned + run in sandbox |
| Published dataset (snapshots) | <https://huggingface.co/datasets/COIN-Bench/coin> | HTTP 200 |
| License | repo `LICENSE` | MIT |

---

## 1. What COIN measures

Most code benchmarks measure **comprehension**: read a function, explain it,
answer questions — graded against a reference answer or an LLM judge. That is
**subjective** (a judge can be persuaded) and **leakable** (answers can end up
in training data).

COIN ("COde-understanding through INput-space mapping") measures the mapping
that turns comprehension into action: from a program's **semantic space** (what
the code means) to its **input space** (an input that reaches a chosen
location). This is **reachability**:

> Produce a concrete input that drives execution to a chosen line. Graded by
> **running the code** — reached or not: objective, and impossible to leak.

A COIN task is a single **target line `ℓ`** in a real project at a pinned
**commit `c`**. The agent (1) picks one of the project's **maintainer-written
harnesses** and (2) produces a **concrete input** (bytes) that, fed to that
harness, **executes `ℓ`**. Grading re-runs the harness on a
**coverage-instrumented build** and checks whether `ℓ` was reached. The outcome
is **binary and objective** — no labels, no multiple choice, no LLM judge. The
ground truth is simply "an input that actually reaches the line." **The input
is the proof.**

Three properties fall out of this design:

- **Contamination-free.** "What input reaches line `ℓ` in commit `c`?" is not
  text that exists on the web, so no solution can leak into training data.
- **Tool-permissive.** Because there is no answer to memorize, agents may use
  anything a real engineer uses — web search, static analyzers, the project
  source — without leaking ground truth.
- **Automatically verified.** Outcomes come from executing the project's own
  harness on an instrumented build.

The corpus is built from **OSS-Fuzz** projects and their saturated corpora,
which supply hard-to-reach targets that state-of-the-art fuzzers miss. (The
full signal algebra used to *construct* frontier vs. non-trivial-reachable
targets is covered in the [companion study](coin-benchmark-and-skwaq-study.md);
it is not needed to *run* the benchmark.)

---

## 2. How to obtain, install, and run it locally

**Requirements:** Python **≥ 3.12** and a working **Docker daemon** (build and
eval stages shell out to Docker; the eval agents run in privileged
Docker-in-Docker). Released under **MIT**, built on **OSS-Fuzz**.

### Clone

```bash
git clone https://github.com/COIN-Bench/coin.git
cd coin
```

### Install

Recommended, with [uv](https://docs.astral.sh/uv/) (resolves the bundled
`corpusdb` sub-package automatically):

```bash
uv sync --extra llm --extra dev   # core + LLM SDKs + test deps
```

With plain pip, install the bundled `corpusdb` local path dependency first:

```bash
pip install -e ./corpusdb
pip install -e ".[llm]"           # add ".[dev]" for tests
```

The CLI is `coin` (or `python -m coin`). Run `coin --help` for the grouped
command list.

### Configure

```bash
cp config.example.yaml config.yaml
```

Then set credentials in a `.env` (or export them):

| Task | Required environment |
|------|----------------------|
| Evaluate agents | `LITELLM_URL`, `LITELLM_MASTER_KEY` — the LiteLLM proxy the agents call |
| Publish a snapshot | `HF_TOKEN` (write) **and** `docker login ghcr.io` for the image push |

Agents are driven through a LiteLLM proxy, so the harness is **model-agnostic**.

### Run (most users: against a published snapshot)

Each project is rebuilt from the dataset's pinned commits and gated by a
functional precheck, so you only need the dataset and a Docker host — no
OSS-Fuzz tree, no re-mining:

```bash
coin evaluate --dataset <hf-repo> --revision <tag>   # e.g. --dataset you/coin@v1
coin show                                            # dashboard at http://localhost:8765
```

Key `coin evaluate` flags (verified via `--help`): `--dataset` +
`--revision` (snapshot mode; `--revision` is required with `--dataset`),
`-e/--experiment` (local-pipeline mode; mutually exclusive with `--dataset`),
`-p/--project` (filter), `--split`, and `--source [rebuild|image]` (`rebuild`
is the default; `image` forces the digest-pinned prebuilt image).

Building a dataset from scratch (advanced) is an eight-stage pipeline
(`gcs-sync → select → prepare-build → build → baseline → agent-baseline →
codeql → extract-targets → evaluate → show`); not required to run the benchmark.

### Sandbox verification (what was actually run here)

Verified in this sandbox (Python 3.13, uv 0.11.7, Docker 29.1.3):

| Step | Command | Result |
|------|---------|--------|
| Clone | `git clone https://github.com/COIN-Bench/coin.git` | ✓ HEAD `83261b0` |
| Install | `uv sync` | ✓ resolved + installed core deps |
| CLI | `uv run coin --help` | ✓ prints `publish` / `evaluate` / `show` + pipeline stages |
| CLI | `uv run coin evaluate --help` | ✓ prints dataset/experiment modes + flags |

Not run here (heavy, and outside a research/docs spike): a full
`coin evaluate` against a published snapshot, which needs a running Docker
daemon with privileged Docker-in-Docker and downloads per-project runtime
images. The install + CLI surface is confirmed working; the end-to-end
evaluation is deferred to the Gym-build phase.

---

## 3. How it scores

Grading is **per-target and binary**: the submitted input is replayed through
the chosen harness on a coverage-instrumented build, and the target line either
is or is not reached. Each target's outcome is one of six codes (from the
site's results matrix):

| Code | Outcome |
|------|---------|
| `R` | Reached ✓ |
| `W` | Submitted — wrong input (did not reach) |
| `A` | Abstained |
| `T` | Timed out |
| `N` | No submission |
| `E` | Error |

Two **aggregate metrics** drive the headline (targeted-reachability) track:

- **Reach rate** — fraction of targets **provably reached** (verified by harness
  replay). This is the primary metric.
- **Precision** — fraction of an agent's **submitted** inputs that **actually
  reached**. This exposes over-claiming: an agent can submit many inputs that do
  not reach the line, so a high reach rate with low precision means noisy
  guessing.

The published set is **70 targets** (of which **35** are the hardest
"frontier" class), with a **2-hour budget per target**. COIN defines three
tracks over the *same* target set: **targeted reachability** (headline),
**seed-tool ablation** (same task with a seed corpus + `corpusdb` query tool),
and **coverage maximization** (no target line; maximize whole-project coverage —
explicitly *not* a proxy for reaching specific targets).

Because grading is execution-based and reproducible from a pinned snapshot, a
local `coin evaluate` run of a given model should reproduce that model's
published reach/precision within variance — the exact property the LOCAL COIN
Gym exploits to score against public numbers **without submitting anything
externally**.

---

## 4. Leaderboard structure — and our LOCAL-ONLY constraint

The public leaderboard (targeted track) ranks **agents = model × scaffold**
over the 70 targets. Its columns (source: `export/T2_main_targeted.csv`,
rendered by `assets/app.js`) are:

`rank · model (family) · scaffold · Reach % · Precision % · Reached (n/70) ·
Frontier (n/35) · $/reached · Total $`

Published entries at time of writing (verified from `assets/app.js`;
`REACHED_TOTAL = 70`, `FRONTIER_TOTAL = 35`):

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

The headline signal: the **frontier is a wall** — across all 8 agents × 35
frontier targets the aggregate reach rate is **0.7%**, versus **38.2%** on
non-trivial-reachable targets. Reasoning to a never-before-seen input is
categorically harder than rediscovering one long-running fuzzing already found.

### Submission / snapshot format

COIN's leaderboard is not a "POST your JSON" endpoint. Results are produced by
running the harness and captured as an **immutable, citable snapshot**:

```bash
docker login ghcr.io                        # once, for the image push
coin publish -e <experiment_id> --version v1 \
    --hf-repo you/coin --registry ghcr.io/you
```

`coin publish` pushes runtime images to a container registry (ghcr.io) and
dataset rows to a HuggingFace dataset repo. Anyone can then reproduce the
numbers locally with `coin evaluate --dataset you/coin --revision v1`. The
"submission format" is therefore a **reproducible published dataset**, not an
opaque score upload.

### LOCAL-ONLY constraint (mandatory for this project)

> We run COIN **locally** to measure our own agents and to score against the
> **public** numbers. We **never** publish, submit, or post our COIN results to
> any external leaderboard, HuggingFace repo, or registry. `coin publish` and
> any `--hf-repo` / `--registry` push are **out of scope** for this project.

Concretely, for the LOCAL COIN Gym we will only ever use `coin evaluate`
(reading a published snapshot) and `coin show` (a local dashboard on
`localhost`). The `publish` path exists in the tool but is deliberately unused.

---

## Summary

| Question | Answer (one line) |
|----------|-------------------|
| **What** | Code-reasoning by *reachability*: emit an input that drives execution to a chosen line, graded by running the code. |
| **Install/run** | `git clone` → `uv sync --extra llm` → `coin evaluate --dataset <repo> --revision <tag>` → `coin show`; needs Python ≥3.12 + Docker. |
| **Scoring** | Binary per-target reach (R/W/A/T/N/E), aggregated as **reach rate** + **precision** over 70 targets (35 frontier), 2-hour budget/target. |
| **Leaderboard** | Ranks model × scaffold on reach%/precision%/reached/frontier/cost; snapshots are published datasets — but our use is **LOCAL-ONLY, never posted externally**. |
