---
title: "COIN benchmark & skwaq gym — study and a LOCAL \"COIN Gym\" design sketch"
description: >
  Phase 1 (LEARN COIN) + Phase 2 (STUDY skwaq) research artifact for the
  build-a-local-coin-benchmark-harness goal. Documents what COIN measures, how
  to install/run/score it locally (verified against the now-published COIN repo
  and HuggingFace dataset), its dataset location and leaderboard structure, and
  how skwaq's self-improvement loop (failure-analyst + overfitting-reviewer)
  maps onto a future LOCAL "COIN Gym". Research + design only — no VM, no
  external submission.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: explanation
status: draft
---

# COIN benchmark & skwaq gym — study and a LOCAL "COIN Gym" design sketch

**Status:** research spike (Phase 1 LEARN COIN + Phase 2 STUDY skwaq).
**Scope:** local study only. This document does **not** provision any Azure VM
and does **not** post COIN results externally. It is the foundation for the
harness build tracked in phases 3–5 (see the linked tracking issues at the end).

> **Companion:** for a focused Phase-1 primer that answers just the four core
> COIN questions (what it measures, how to install/run, how it scores, its
> leaderboard), see [COIN benchmark — Phase 1 primer](coin-benchmark.md). This
> document is the deeper study: it adds the target-construction signal algebra,
> the skwaq gym internals, and a LOCAL "COIN Gym" design sketch.

> **2026-07-07 refresh.** COIN's GitHub repo and HuggingFace dataset — labelled
> "to be published" when this study was first drafted — are now **live**. The
> COIN sections below were re-verified against the published repository README
> and dataset card, and the install/run commands, dataset location, splits, and
> schema now reflect those authoritative sources (see the *Sources* table). The
> skwaq sections carry `path:line` citations against a read-only study clone at
> `~/src/skwaq`.

This document answers three questions:

1. **COIN** — what it measures, how targets are constructed, how the score works,
   how the leaderboard is structured, and how to install/run it locally.
2. **skwaq gym** — how skwaq's self-improvement loop works internally: the
   benchmark harness, failure-analysis, the overfitting-reviewer gate, the
   multi-agent debate, and the structured role-cards / output-schemas.
3. **A LOCAL "COIN Gym"** — a concrete design sketch for a harness that runs
   COIN locally, scores locally against the published leaderboard, and measures
   a single-model baseline vs. a multi-agent team — mirroring skwaq's
   failure-analysis + overfitting-reviewer gating.

---

## Sources

Primary, authoritative sources used for this study (verified directly, not via
summarizers — a generic web search for "COIN benchmark" returns several
unrelated projects with the same acronym, none of which are the code-reasoning
benchmark below):

| Topic | Source |
|-------|--------|
| COIN — task, construction, leaderboard, results | <https://coin-bench.github.io/> (page + `assets/app.js`) |
| COIN — code, install/run, build pipeline | <https://github.com/coin-bench/coin> (repo README) |
| COIN — dataset location, splits, schema, provenance | <https://huggingface.co/datasets/COIN-Bench/coin> (dataset card, `v2026-07`) |
| skwaq — overview, `~66%` reject-rate claim | `~/src/skwaq/README.md:9` · <https://github.com/rysweet/skwaq> |
| skwaq — gym scoring | `~/src/skwaq/crates/gym/src/scoring.rs` (`cwe_family()` L221; P/R/F1 L289–292; `CWE_REGRESSION_NOISE_MARGIN=0.02` L8) |
| skwaq — self-improvement loop | `~/src/skwaq/crates/gym/src/improve.rs` (`ImprovementKind` L63–76; budgets L40–42; `HOLDOUT_OVERFITTING_GAP_THRESHOLD=0.15` L34; durable memory L2825/L2890) |
| skwaq — agents / role-cards | `~/src/skwaq/agents/*.md` (18 role-cards; `failure-analyst.md`, `overfitting-reviewer.md`, `verdict-synthesizer.md`) |
| skwaq — debate output schemas | `~/src/skwaq/crates/core/src/agents/output_schema.rs` (`exploit-analyst-v1`, `defense-analyst-v1`); `crates/core/src/agents/pipeline.rs` (`threshold_hint`, `HIGH_CONFIDENCE_CONFIRM_THRESHOLD=140`) |

> **Verification note.** COIN's marketing site (`coin-bench.github.io`) still
> labels its GitHub/Hugging Face links "to be published", but both are now
> **live** and were fetched directly for this refresh: the repo README supplies
> the authoritative install/run/build commands, and the dataset card pins the
> `v2026-07` snapshot (repo commit `e99b764…`, 70 targets across 7 projects).
> The skwaq facts were verified against a **read-only** study clone at
> `~/src/skwaq` — no writes to that repo.

---

## Part 1 — COIN (COde → INput)

> **Name.** The marketing site expands COIN as **"COde → INput"**; the published
> repository and dataset expand it as **"COde-understanding through INput-space
> mapping"**. Same benchmark — both expansions describe the semantic→input
> mapping task below.

### 1.1 What COIN measures

Most code benchmarks measure **comprehension**: read a function, explain it,
answer questions — graded against a reference answer or an LLM judge. That is
**subjective** (a judge can be persuaded) and **leakable** (answers can end up
in training data).

COIN measures the mapping that turns comprehension into action: from a
program's **semantic space** (what the code means) to its **input space** (an
input that reaches a chosen location). This is called **reachability**:

> Produce a concrete input that drives execution to a chosen line. Graded by
> **running the code** — reached or not: objective, and impossible to leak.

The **input is the proof**. Because grading is done by execution, COIN is a
**verifiable, objective, contamination-resistant** measure of code reasoning.

### 1.2 The task

A COIN task is a single **target line `ℓ`** in a real project at a pinned
**commit `c`**. The agent:

1. picks one of the project's **maintainer-written harnesses**, and
2. produces a **concrete input** (bytes) that, when fed to that harness,
   **executes `ℓ`**.

Grading re-runs the harness on a **coverage-instrumented build** and checks
whether `ℓ` was reached. The outcome is **binary and objective** — no labels,
no multiple choice, no LLM judge. The ground truth is simply "an input that
actually reaches the line."

Three properties fall out of this design:

- **Contamination-free.** "What input reaches line `ℓ` in commit `c`?" is not
  text that exists on the web. Frontier targets have *never* been hit, so no
  solution can leak into training data.
- **Tool-permissive.** Because there is no answer to memorize, agents may use
  anything a real engineer uses — web search, static analyzers, the project
  source itself — without leaking ground truth.
- **Automatically verified.** Outcomes come from executing the project's own
  harness on an instrumented build.

*Example target (frontier):* project `libraw` (a C++ raw-image decoder),
harness `libraw_raf_fuzzer`, target `src/metadata/fuji.cpp:480`. The agent must
emit bytes that decode as a Fuji RAF file and drive control flow into that line.

### 1.3 How targets are constructed

COIN is built from **OSS-Fuzz** long-running coverage data plus
maintainer-written harnesses, spanning **1000+ real-world projects in nine
languages**. Crucially, it filters **against** cheap coverage rather than simply
sampling covered lines.

Reachability is established two ways, and cheap "target-blind" baselines are
then **subtracted**. Each candidate line carries five signals:

| Signal | Group | Meaning |
|--------|-------|---------|
| **C** | reachability | **Static reach** — CodeQL call-graph evidence a harness→target path exists (no dynamic run needed). |
| **B** | reachability | **Untaken branch** — first executable line of an untaken branch/loop body *adjacent* to already-covered code. |
| **G** | reachability | **Long fuzzing** — line covered by OSS-Fuzz's continuous, industrial-scale fuzzing. |
| **F** | cheap baseline | **Short fuzzing** — a fresh, fixed-budget libFuzzer run in-experiment. |
| **L** | cheap baseline | **LLM seed-gen** — an LLM given the harness but *not* the target line, generating seeds (goal-blind). |

Two target families are sampled from the signal algebra:

- **Frontier targets:** `(B ∪ C) \ (G ∪ F ∪ L)` — provably reachable by static
  analysis, but **never** reached by any fuzzer or baseline. Hardest and most
  contamination-resistant class.
- **Non-trivial reachable:** `G \ (F ∪ L)` — reached by long-running fuzzing,
  but missed by fresh fuzzers and goal-blind LLM seeds. The agent must
  *rediscover* a witnessed path that cheap baselines can't.

The pipeline sampled **639,410** candidate uncovered lines (≈31,259 non-trivial
reachable + ≈7,048 branch-frontier candidates) down to **70 evaluable
targets** — five per family across 7 projects. Because it is regenerated from
OSS-Fuzz and upstream commits, **new code opens new frontiers**: the benchmark
is **self-refreshing** and "cannot saturate," and yesterday's solved targets
are replaced.

**How the published snapshot concretizes this.** The dataset card assigns every
candidate a **6-bit signal cube** `(G, S, N, F, L, C)` — GCS-covered,
seed-reachable, fuzzer-reachable (libFuzzer, no seed), baseline-reachable
(libFuzzer), LLM-baseline-reachable, and CodeQL-statically-reachable — and then
partitions the released targets into **two first-match-wins splits**:

| Split (dataset) | Signal cell | Rows | Maps to the family above |
|-----------------|-------------|-----:|--------------------------|
| `gcs_reachable` | `G and not (F or L)` | 35 | **non-trivial reachable** |
| `codeql_only`   | `C and not (G or F or L)` | 35 | **frontier** (statically reachable, never covered) |

So the `v2026-07` release is exactly **70 targets = 35 + 35**. The `codeql_only`
split is the contamination-resistant "frontier" class (a static path is proven
to exist, yet no fuzzer or baseline has ever reached the line).

### 1.4 Scoring

Two metrics drive the headline leaderboard (targeted-reachability track):

- **Reach rate** = fraction of targets **provably reached**, verified by harness
  replay on the instrumented build.
- **Precision** = fraction of an agent's **submitted** inputs that **actually
  reached**. This exposes over-claiming: agents routinely submit inputs that do
  *not* reach the line. Even the best model's submissions are right only ~half
  the time (52.5%); the weakest lands 12.8%.

Per-target outcomes (used in the results matrix) are one of:

| Code | Outcome |
|------|---------|
| `R` | Reached ✓ |
| `W` | Submitted — wrong input (did not reach) |
| `A` | Abstained |
| `T` | Timed out |
| `N` | No submission |
| `E` | Error |

### 1.5 Leaderboard structure

The leaderboard evaluates **8 state-of-the-art agents × 70 targets**, with a
**2-hour budget per target**. Columns: rank, model, scaffold, **Reach %**,
**Precision %**, Reached (n/70), Frontier (n/35), `$/reached`, Total `$`.

Published targeted-track results (source: the site's `assets/app.js`,
`export/T2_main_targeted.csv`):

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

- **The frontier is a wall.** Across all 8 agents and 35 frontier targets, the
  aggregate reach rate is **0.7%**, versus **38.2%** on non-trivial reachable
  targets. Reasoning to a *never-before-seen* input is categorically harder
  than rediscovering one long-running fuzzing already found.
- **Union is small.** **47/70** targets were solved by **no** model; only **1**
  by all eight; **23/70** solved by *any* model (Opus alone accounts for 21).
- **Cost ≠ capability.** Spend-per-reached-target does not track skill:
  GPT-5.4-mini is cheapest per success ($1.29) while Sonnet costs $15.51 —
  more than Opus's $9.99 despite reaching fewer targets. Backward reasoning is
  the bottleneck, not budget.
- **Capability gap.** Within each vendor, flagships beat lightweight siblings
  (**25.7%** vs **19.1%** reach). Commercial models average **22.4%** vs
  **10.7%** for open-weights.

COIN defines **three tracks from a single target set**:

1. **Targeted reachability** (headline) — reach one specific line; reach rate +
   precision.
2. **Seed-tool ablation** — same targeted task, but mount the saturated seed
   corpus + a `corpusdb` query tool. Aggregate reach barely moves
   (19.5% → 19.1%); it just reshuffles *which* lines get solved.
3. **Coverage maximization** — no target line; maximize whole-project coverage.
   Coverage is **not** a target proxy: on liboqs it touches 47.7% of lines yet
   hits **0** of 10 targets.

### 1.6 Install & run locally

The commands below are taken from the **published repository README**
(<https://github.com/coin-bench/coin>) and the dataset card, and supersede the
marketing site's abbreviated "Run it yourself" snippet.

**Requirements:** Python **≥ 3.12** and a working **Docker daemon**. Stages 4–7
shell out to Docker, and the **evaluation agents run in privileged
Docker-in-Docker** — a security-relevant constraint that Phase 3 (VM
provisioning) must account for.

**Install** — recommended path uses [`uv`](https://docs.astral.sh/uv/), which
resolves the bundled `corpusdb` sub-package automatically:

```bash
# clone + install (core + LLM SDKs + test deps)
git clone https://github.com/coin-bench/coin && cd coin
uv sync --extra llm --extra dev
```

With plain `pip`, install the bundled local-path `corpusdb` sub-package first
(pip cannot resolve it from PyPI):

```bash
pip install -e ./corpusdb
pip install -e .            # add ".[llm]" and/or ".[dev]" as needed
```

Or install the released CLI straight from PyPI:

```bash
pip install coin-bench      # provides the `coin` CLI
```

**Configure** — copy the example config, then supply credentials via `.env`
(or exported env vars):

```bash
cp config.example.yaml config.yaml
```

| Task | Required in `.env` / environment |
|------|----------------------------------|
| Evaluate agents (stage 7) | `LITELLM_URL`, `LITELLM_MASTER_KEY` — the LiteLLM proxy the agents call |
| Publish a dataset | `HF_TOKEN` (write) **and** `docker login ghcr.io` for the image push |

The CLI is `coin` (or `python -m coin`); `coin --help` lists the grouped
commands and `coin <command> --help` the flags.

**Evaluate against a published snapshot** (the common case — no pipeline run
needed; `config.yaml` supplies the eval knobs):

```bash
# evaluate an agent against a released COIN snapshot + split
coin evaluate --dataset COIN-Bench/coin --revision v2026-07 \
    --split codeql_only --output-dir results/

# dashboard at http://localhost:8765
coin show
```

By default each project is **rebuilt from the dataset's pinned commits** and
gated by a **functional precheck**, falling back to the published runtime image
only if a rebuild fails (`--source image` forces the image path). This rebuild
+ instrumented-replay is the objective grading oracle — never re-implement it.

**Publish a snapshot** (turns a finished experiment into a citable, immutable
snapshot — not needed to *run* the benchmark):

```bash
docker login ghcr.io
coin publish -e <experiment_id> --version v1 \
    --hf-repo you/coin --registry ghcr.io/you
```

**Build a dataset from scratch (advanced).** The full pipeline is 8 operator-run
stages; stages 3–6 take a `build_id` (minted by `select`), stages 7–8 an
`experiment_id` (minted by `extract-targets`):

```bash
coin -c config.yaml gcs-sync --project jq   # 1  sync OSS-Fuzz coverage
coin -c config.yaml select                  # 2  pick projects -> build_id
coin prepare-build   -b <build_id>          # 3  patch Dockerfiles
coin build           -b <build_id>          # 4  normal + coverage builds
coin baseline        -b <build_id>          # 5  fuzzer baseline
coin agent-baseline  -b <build_id>          # 5b LLM seed-gen baseline
coin codeql          -b <build_id>          # 5  CodeQL reachability
coin extract-targets -b <build_id>          # 6  select targets -> experiment_id
coin evaluate        -e <experiment_id>     # 7  evaluate agents
coin show                                   # 8  dashboard
```

Per the dataset card, stages 1–6 (selection) are **operator-driven / manual**;
**evaluating against a published snapshot is fully automated** for downstream
users. Agents are driven through **LiteLLM**, so the harness is model-agnostic.
Released under **MIT**; built on **OSS-Fuzz**.

**Local-vs-leaderboard scoring.** Because grading is execution-based and
reproducible from a pinned snapshot, a local `coin evaluate` run of a given
model *should* reproduce that model's leaderboard reach/precision within
variance. This is exactly the property the LOCAL COIN Gym exploits to score
locally against the public numbers without submitting anything externally.

### 1.7 Dataset location, splits & schema

**Location:** `COIN-Bench/coin` on the HuggingFace Hub, tag **`v2026-07`**
(<https://huggingface.co/datasets/COIN-Bench/coin>). Load a split directly:

```python
from datasets import load_dataset

ds = load_dataset("COIN-Bench/coin", revision="v2026-07", split="codeql_only")
print(ds[0]["target_id"], ds[0]["runtime_image"])
```

The `v2026-07` release pins **70 verified targets** across **7 OSS-Fuzz
projects** — `cups`, `cyclonedds`, `hdf5`, `karchive`, `liboqs`, `libraw`,
`rdkit` — in languages `c` / `c++` / `rust` / `python` / `jvm`. (COIN's
*construction* spans 1000+ projects / 9 languages; a *release* pins a curated
subset.) Each project ships a **digest-pinned runtime image** so reruns hit the
same binaries; total image footprint is **44.9 GB**. Provenance: COIN repo @
`e99b764ba8d7a546e425666851b81545bc800f63`, source experiment
`20260520T165657Z-b45032068f`, created `2026-07-04`.

Key schema columns (dataset card) — enough to drive a harness:

| Column | Meaning |
|--------|---------|
| `target_id` | `<project>:<harness>:<file>:<line_start>[-<line_end>]` |
| `project`, `harness`, `language` | project, primary reaching harness binary, language |
| `file`, `line_start`, `line_end` | canonical `/src/<project>/…` path + target line range |
| `function`, `source_snippet` | enclosing function; ±100 lines of context |
| `gcs_covered` / `baseline_reachable` / `agent_baseline_reachable` / `codeql_reachable` | the `G` / `F` / `L` / `C` signals |
| `prompt_preview` | the `TASK.md` preview stage 7 sends to the agent |
| `runtime_image` (`<image>@sha256:…`), `runtime_digest` | image to spin up + its digest |
| `runtime_normal_path` (`/out/normal`), `runtime_coverage_path` (`/out/coverage`), `runtime_source_path` (`/src`) | paths inside the image: harness binaries, coverage build, sources |
| `split` | denormalized split name (`gcs_reachable` / `codeql_only`) |

---

## Part 2 — skwaq's self-improvement loop

skwaq is a self-improving, multi-agent **vulnerability analyzer** (18 agents on
the RustyClawd framework, a LadybugDB code-property graph). It is not a
reachability tool, but its **self-improvement machinery** is the pattern this
spike wants to copy. Three Rust crates:

- **skwaq-core** — parsing, graph DB, analysis engine, **18** agent definitions
  (`~/src/skwaq/agents/*.md`), LLM client, durable agent memory.
- **skwaq-gym** — benchmark harness, industry adapters, the self-improvement
  loop with **failure-analyst** and **overfitting-reviewer** agents.
- **skwaq** (CLI) — `clap`-based, **29** top-level commands (verified in
  `crates/cli/src/commands/mod.rs`), including the `gym` subcommands.

### 2.1 The Gym harness

`skwaq gym` runs benchmark suites (`fixtures`, `juliet`, `owasp`, `cgc`,
`cyberseceval`, `realworld`, `binpool`, `cybergym`) through per-suite adapters
and scores each case. It has **three modes** that make baseline-vs-team
measurement first-class:

| Mode | Flag | What it measures |
|------|------|------------------|
| Pattern-only | `--quick` | fast regex/pattern baseline (30s/case) |
| Full | (default) | patterns + LLM agents + synthesis (best accuracy) |
| LLM-only | `--llm-only` | agent understanding **without** pattern help |

**Scoring** (`crates/gym/src/scoring.rs`) is standard IR: TP/FP/FN with
**precision = TP/(TP+FP)** (`scoring.rs:289`), **recall = TP/(TP+FN)**
(`scoring.rs:290`), **F1 = 2PR/(P+R)** (`scoring.rs:291–292`), plus:

- **Per-CWE family** scoring — `cwe_family()` (`scoring.rs:221`) collapses
  specific CWEs to their parent family (e.g. CWE-121 → CWE-119 memory-safety) so scoring is not brittle
  to exact-ID mismatches.
- **Benchmark vs. adjudicated precision** — a finding on an unlabeled case is a
  *disagreement pending adjudication*, not automatically a false positive. The
  benchmark answer key is not treated as a complete precision oracle.
- **Negative-case calibration** — patched/safe cases track the false-positive
  rate separately; only `critical`-severity findings matching the original CWE
  count as FPs, so pattern noise doesn't inflate FP counts.

### 2.2 The self-improvement loop

`skwaq gym improve <suite>` runs an automated cycle. The essential idea: agents
analyze **their own failures**, propose targeted fixes, a reviewer **gates
against overfitting**, accepted patches are applied, and the benchmark re-runs —
keeping a change **only if it improves without regressing**. The five conceptual
phases below span more than one function: `run_improvement_cycle()`
(`crates/gym/src/improve.rs`) collects failures and emits proposals (phases 1–2),
while overfitting review, patch application, and re-benchmark/verify run in the
surrounding `apply_accepted_proposals()` + validation path (phases 3–5).

```
Benchmark → Failure Analysis → Proposal Generation → Overfitting Review → Patch Application
    ↑                                                                            |
    └───────────────────────── Re-benchmark & Verify ────────────────────────────┘
```

**Phase 1 — Benchmark & collect outcomes.** Run the suite, score every case,
collect false negatives (missed vulns) with their source. The case set is split
into **training** and **holdout** partitions; only training cases feed the
analyst, and the holdout validates that improvements **generalize** (are not
overfit to specific inputs).

**Phase 2 — Failure analysis.** The **failure-analyst** agent examines each FN
using enriched graph context (imports, data sources, cross-file call graph,
string refs), queries the knowledge base, diagnoses *why* detection failed, and
emits **structured proposals**. It is explicitly told to prefer graph-aware
fixes (`AgentPrompt`, `TaintRule`) over brittle regex (`NewPattern`), and every
proposal must cite KB or durable-memory **evidence**.

**Phase 3 — Overfitting review (the gate).** Every proposal passes through the
**overfitting-reviewer** agent, which rejects **~66%** of proposals (a figure
the skwaq README records at `README.md:9`). It asks three questions:

1. **Real-world generality** — would this detect vulns in *real* production
   code, or only match benchmark-specific naming
   (`CWE121_Stack_Based_Buffer_Overflow__`, `cgc_allocate`)? Reject the latter.
2. **Pattern specificity** — reject wildcard patterns (`\w+_read`,
   `\w+_receive`) that will fire on safe production APIs; accept specific,
   known-dangerous functions (`\brecv\s*\(`).
3. **CWE-mapping accuracy** — reject mappings that inflate scores by mapping to
   a broader family (e.g. `format_string → CWE-119`).

Its rule of thumb: **when in doubt, favor precision over recall** — better to
miss a vuln than flood users with false positives. Rejected proposals are
logged (to `data/knowledge/fn-insights.md`) so future cycles don't re-propose
them.

**Phase 4 — Patch application.** Accepted proposals are applied by type
(`enum ImprovementKind`, `crates/gym/src/improve.rs:63–76`), each with a narrow,
validated strategy (path-canonicalized, exact find/replace or schema-validated,
never partial). Verified apply targets (`improve.rs` ~L2228–2263):

| Kind | Target | Strategy |
|------|--------|----------|
| `NewPattern` | `patterns_source.rs` | append/replace typed `SourcePattern` (regex size-limited) |
| `AgentPrompt` | `agents/*.md` | append after last `##` / exact replace |
| `CweMapping` | `scoring.rs` | find/replace on mapping fns |
| `TaintRule` | taint analysis (`analysis/taint.rs`) | add taint source/sink rule |
| `RecipeChange` | `recipes/analysis/standard.yaml` | insert stage before `debate:`; YAML re-validated |
| `GroundTruthFix` | `ground_truth/` | find/replace (extra scrutiny) |

**Phase 5 — Verification.** Re-run the benchmark; **accept only if**: F1 does
not decrease, precision drops ≤ **2%**, and **no per-CWE detection rate
regresses beyond the 2% noise margin** (`CWE_REGRESSION_NOISE_MARGIN = 0.02`,
`scoring.rs:8`). Otherwise **roll back all patches**. A training/holdout F1 gap
beyond **0.15** raises an overfitting warning and flags the cycle
(`HOLDOUT_OVERFITTING_GAP_THRESHOLD = 0.15`, `improve.rs:34`). Per-case token
budgets are bounded: ≈**50k** tokens/case target
(`FAILURE_ANALYST_TARGET_BUDGET_PER_CASE`, `improve.rs:41`), **100k** max
(`…MAX_BUDGET_PER_CASE`, `improve.rs:42`), **≤20 cases/cycle**
(`FAILURE_ANALYST_MAX_CASES`, `improve.rs:40`).

**Durable memory.** Each cycle appends to `fn-insights.md` (per-case failure
analysis, `improve.rs:2825`) and `learned-patterns.md` (patterns discovered,
with regex + target CWE + source case, `improve.rs:2890`). Future analysts read
these to avoid re-proposing rejected ideas — the loop *remembers*.

### 2.3 Multi-agent debate + structured role-cards

Beyond the gym, skwaq's analysis pipeline uses a **debate** stage. Specialist
agents argue exploitability with **structured output schemas** — declared in
`crates/core/src/agents/output_schema.rs` as `exploit-analyst-v1` and
`defense-analyst-v1` — and a **verdict-synthesizer** (`agents/verdict-synthesizer.md`)
combines them. Agents themselves are **markdown role-cards** (`agents/*.md`) with
YAML front-matter (`name`, `description`, `model`, `tools`, `max_turns`); only
the specialist debate/PoC agents additionally declare an `output_schema` (e.g.
`exploit-analyst`, `defense-analyst`, `vuln-hunter`, `poc-prover`) — most agents
do not. Key mechanics worth copying:

- **Schema-backed contracts.** When structured exploit/defense outputs parse,
  the debate emits **confidence-threshold hints** (`threshold_hint`, in
  `crates/core/src/agents/pipeline.rs`) that act as an auto-confirm/auto-reject
  gate. If parsing fails, it falls back to direct code review rather than
  trusting free text.
- **Exploitability-led promotion.** A high-confidence confirm requires a strong
  exploit-side signal *plus* supporting defense agreement — implemented as a raw
  **score threshold of 140** (`HIGH_CONFIDENCE_CONFIRM_THRESHOLD = 140`,
  `pipeline.rs`), so a merely net-positive score never auto-promotes. Ambiguous
  findings are biased toward rejection unless direct code evidence is strong.

### 2.4 Multi-model comparison via profiles

`skwaq gym profile create <name> --backend <b> --model <m>` gives each model an
**isolated** results DB, memory graph, and telemetry, so baseline-vs-team and
model-vs-model comparisons are reproducible (`skwaq gym run … --profile opus`,
`skwaq gym dashboard --tui --profile opus`).

### 2.5 What transfers to COIN

| skwaq mechanism | COIN Gym analog |
|-----------------|-----------------|
| Benchmark harness + adapters | wrap `coin evaluate` as the eval engine |
| Modes (pattern / full / llm-only) | **single-model baseline** vs **multi-agent team** |
| Score TP/FP/FN, per-CWE | **reach rate / precision**, per-family (frontier vs reachable) |
| Failure-analyst on FNs | analyze **unreached targets** (`W/T/N`) |
| **Overfitting-reviewer gate** | reject strategies that **memorize specific inputs** vs. improve general reachability reasoning |
| Train/holdout split + rollback | **held-out fresh targets** (COIN self-refreshes) as the anti-overfit oracle |
| Durable memory (`fn-insights.md`) | remembered reachability tactics per project/harness |
| Profiles | per-model isolated runs vs. the public leaderboard |

---

## Part 3 — Design sketch: a LOCAL "COIN Gym"

**Objective (phases 3–5):** a local harness that (a) runs the COIN benchmark,
(b) scores locally **vs. the published leaderboard**, and (c) measures a
**single-model baseline vs. a multi-agent team**, with a skwaq-style
**failure-analysis + overfitting-reviewer** self-improvement loop — all
**local/notes only**, no external submission.

### 3.1 Why this is a good fit

COIN is *already* the ideal substrate for a skwaq-style loop:

- **Objective, execution-graded** signal (reach/no-reach) — no LLM judge to
  game, so the improvement loop optimizes a real quantity.
- **Contamination-resistant & self-refreshing** — the built-in defense against
  the exact failure skwaq's overfitting-reviewer guards against. Held-out fresh
  targets are a *natural* anti-overfit oracle: you literally cannot memorize an
  input to a line that has never been reached.
- **Model-agnostic** via LiteLLM — trivial to swap the agent under test and to
  A/B a single model vs. an orchestrated team on identical targets.

### 3.2 Architecture

```
                         ┌──────────────────────────────┐
                         │  LOCAL COIN GYM (Simard crate)│
                         └──────────────────────────────┘
  target set (COIN snapshot you/coin@v1)
        │
        ▼
  ┌───────────────┐   drives   ┌──────────────────────────────┐
  │ Agent runner  │──────────▶ │ Agent-under-test              │
  │ (per target)  │            │  A) single-model baseline     │
  └──────┬────────┘            │  B) multi-agent team (debate) │
         │ submitted input     └──────────────────────────────┘
         ▼
  ┌───────────────┐   exec     ┌──────────────────────────────┐
  │ COIN harness  │──────────▶ │ Docker: instrumented replay   │
  │ (coin evaluate)│           │  reached? (R/W/A/T/N/E)       │
  └──────┬────────┘            └──────────────────────────────┘
         │ per-target outcomes
         ▼
  ┌───────────────┐   compare  ┌──────────────────────────────┐
  │ Scorer        │──────────▶ │ Leaderboard comparator        │
  │ reach/precision│           │  local vs published (Part 1.5)│
  └──────┬────────┘            └──────────────────────────────┘
         │ unreached targets (W/T/N)
         ▼
  ┌───────────────┐  propose   ┌──────────────────────────────┐
  │ Failure       │──────────▶ │ Overfitting-reviewer GATE     │
  │ analyst       │            │  reject input-memorizing      │
  └──────┬────────┘            │  strategies; accept general   │
         │ accepted strategy   └───────────┬──────────────────┘
         ▼                                 │ verify on held-out
  ┌───────────────┐                        ▼  fresh targets
  │ Apply strategy│◀────────── keep iff reach↑ & precision not↓
  └───────────────┘            else roll back
```

### 3.3 Components

1. **Target loader.** Pull a pinned COIN snapshot (`you/coin@v1`), enumerate the
   70 targets with their family (frontier vs. non-trivial reachable), project,
   harness, and target line. Keep a **held-out** slice (fresh targets from a
   newer snapshot) reserved for the verification gate.
2. **Agent-under-test runner.** Two interchangeable strategies behind one
   interface (mirrors skwaq's modes):
   - **A) single-model baseline** — one LLM (via LiteLLM) reads the harness +
     source and emits candidate input bytes.
   - **B) multi-agent team** — a skwaq-style debate: a *reacher* proposes an
     input and a rationale; a *skeptic/defense* agent challenges whether it
     truly reaches `ℓ` (predicts `W` over-claims); a *synthesizer* decides
     submit vs. abstain using a `threshold_hint`-style gate (abstain rather
     than submit a low-confidence input, since **precision** punishes
     over-claiming).
3. **COIN harness executor.** Delegates grading to `coin evaluate` (Docker +
   instrumented replay). This is the objective oracle — never re-implemented.
4. **Scorer.** Compute **reach rate** and **precision** overall and split by
   family (frontier / non-trivial reachable), plus the `R/W/A/T/N/E` outcome
   histogram.
5. **Leaderboard comparator.** Diff local reach/precision against the published
   numbers (Part 1.5) for the same model; flag material deviation (e.g. a local
   Opus 4.6 run far from 30.0% reach ⇒ harness/config bug, not a capability
   result).
6. **Self-improvement loop (skwaq-mirrored).**
   - **Failure-analyst** examines unreached targets (`W/T/N`): *why* did the
     input miss? (wrong file format, unflipped branch condition, missed
     constraint, wrong harness choice). It proposes a **general reachability
     tactic** (e.g. "for format-gated decoders, first satisfy the magic-byte /
     header validator before targeting deep lines"), citing evidence, **not** a
     hardcoded input for a specific target.
   - **Overfitting-reviewer GATE** rejects any proposal that memorizes a
     specific input or keys off a specific target id / project name — the direct
     analog of skwaq rejecting `CWE121_…`-style patterns. Accept only tactics
     that plausibly generalize across projects/harnesses.
   - **Verification** re-runs on **held-out fresh targets**; keep the tactic
     **iff** reach rate improves and precision does not drop (skwaq's
     accept-only-on-improvement-without-regression rule). Otherwise roll back.
   - **Durable memory** stores accepted tactics per project/harness family, so
     later runs start from learned reachability heuristics.
7. **Profiles.** Isolated run state per model so baseline-vs-team and
   model-vs-model comparisons are reproducible and never cross-contaminate.

### 3.4 Data model (sketch)

- `target(id, project, commit, harness, file, line, family)` — from the snapshot.
- `run(id, model, strategy, snapshot, started_at)` — one evaluation pass.
- `outcome(run_id, target_id, code {R,W,A,T,N,E}, reached: bool, submitted: bool, cost)`.
- `score(run_id, family, reach_rate, precision, n)` — aggregate.
- `proposal(id, run_id, tactic, evidence, verdict {accept,reject,modify}, review_reason)`.
- `tactic_memory(project_family, harness, tactic, accepted_at)` — durable memory.

### 3.5 CLI sketch (mirrors `skwaq gym`)

```bash
coin-gym run    <model> [--strategy baseline|team]   # evaluate on the target set
coin-gym score  <run-id>                             # reach/precision + family split
coin-gym compare <run-id>                            # local vs published leaderboard
coin-gym improve <suite> --holdout fresh             # one self-improvement cycle
coin-gym leaderboard [--profile <name>]              # LOCAL standings: does team beat baseline?
coin-gym profiles                                    # list per-model isolated state
```

> **Implemented (Phase 4).** The Phase-4 CLI landed as
> `coin-gym run <model> [--strategy baseline|team] [--profile <name>] [--targets <path>]`,
> `score|compare|improve <run-id> [--profile <name>]`, `leaderboard [--profile
> <name>]`, and `profiles`. The `improve` command runs the **offline**
> failure-analyst + overfitting-reviewer gate over a saved run; the live
> `--holdout fresh` verify/rollback cycle sketched above needs live grading and
> is Phase 5. `leaderboard` ranks the harness's own saved runs LOCALLY so the
> multi-agent team's climb over the single-model baseline is directly observable
> (LOCAL-ONLY — never posted externally). See
> [Run the LOCAL COIN Gym harness](../howto/run-the-coin-gym-harness.md). The
> reproducible baseline-vs-team result on the bundled sample target set — the
> abstention gate lifting precision from 60% to 100% at equal reach — is recorded
> in [COIN Gym — baseline vs. team measurement](./coin-gym-baseline-vs-team-measurement.md).

### 3.6 Anti-overfitting: the central design tension

skwaq's whole loop is a fight against **building to the benchmark**. COIN gives
that fight a stronger footing than skwaq's static suites:

- The verification oracle is **execution on fresh, never-solved lines**, so a
  strategy that merely memorizes past inputs earns **nothing** on held-out
  frontier targets.
- The overfitting-reviewer's job narrows to one crisp rule: **accept tactics
  that generalize the semantic→input reasoning; reject anything that encodes a
  specific answer.** This is easier to adjudicate than skwaq's precision/recall
  trade-off because reach is a hard, per-target pass/fail.
- Because COIN self-refreshes, the loop can be run continuously against new
  snapshots without the answer key ever going stale — the ideal setting for a
  long-running self-improving agent.

### 3.7 Explicitly out of scope for this spike

- No Azure/`azlin` VM provisioning (phase 3).
- No external submission of COIN results or leaderboard entry.
- No harness code — this document is the design that phases 3–5 build against.

---

## Phases & tracking

| Phase | Description | Status |
|-------|-------------|--------|
| 1 | LEARN COIN (this doc, Part 1) | ✅ done |
| 2 | STUDY skwaq loop (this doc, Part 2) | ✅ done |
| 3 | Provision compute (`azlin` VM, `DefenderATEVET17`) + pull COIN snapshot | ⏭ tracked — [#2823](https://github.com/rysweet/Simard/issues/2823) |
| 4 | Build the LOCAL COIN Gym harness (Part 3) | ✅ done ([#2824](https://github.com/rysweet/Simard/issues/2824)) — Rust `coin_gym` module + `coin-gym` CLI; see [Run the LOCAL COIN Gym harness](../howto/run-the-coin-gym-harness.md) |
| 5 | Iterative self-improve (baseline vs team; failure-analysis + overfit gate) | ⏭ tracked — [#2825](https://github.com/rysweet/Simard/issues/2825) |

> **Phase 4 note (language).** The harness landed as a Rust module
> (`src/coin_gym/`) exposing a `coin-gym` CLI, not a standalone Python package:
> the Simard repo enforces a Rust-only policy (issue #2155,
> `scripts/check-rust-only-gate.sh`) and this design already called the harness a
> "Simard crate". COIN's own `coin evaluate` tool (Python/uv/Docker) stays an
> **external** oracle the harness delegates to via a mockable executor — never
> re-implemented. See
> [Run the LOCAL COIN Gym harness](../howto/run-the-coin-gym-harness.md).

Phases 3–5 are **decomposed into three independent tracking issues** so later
cycles can fan them out to separate engineers in parallel: **#2823** (Phase 3,
VM provisioning — high-risk/gated), **#2824** (Phase 4, harness build —
delivered by this PR), and **#2825** (Phase 5, self-improve loop). Each carries
an explicit *done-when* and its own dependency ordering (5 → 4 → 3). These
supersede the former combined tracker (#2713, now closed).
