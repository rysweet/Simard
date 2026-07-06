# COIN benchmark & skwaq gym — study and a LOCAL "COIN Gym" design sketch

**Status:** research spike (Phase 1 LEARN COIN + Phase 2 STUDY skwaq).
**Scope:** local study only. This document does **not** provision any Azure VM
and does **not** post COIN results externally. It is the foundation for the
harness build tracked in phases 3–5 (see the linked tracking issue at the end).

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
| COIN — task, construction, leaderboard, run commands | <https://coin-bench.github.io/> (page + `assets/app.js`) |
| COIN — code | <https://github.com/COIN-Bench/coin> |
| COIN — dataset | <https://huggingface.co/datasets/COIN-Bench/coin> |
| skwaq — overview | <https://github.com/rysweet/skwaq> · <https://rysweet.github.io/skwaq/> |
| skwaq — gym design | `Specifications/skwaq-gym-design.md` (in the skwaq repo) |
| skwaq — gym loop | `docs/gym-self-improvement.md` (in the skwaq repo) |
| skwaq — agents | `agents/failure-analyst.md`, `agents/overfitting-reviewer.md`, debate agents |
| skwaq — scoring/loop code | `crates/gym/src/scoring.rs`, `crates/gym/src/improve.rs` |

> COIN's own site labels its GitHub and Hugging Face links "to be published";
> the URLs above are the values embedded in the site's `assets/app.js`
> (`LINKS.code` / `LINKS.data`). Treat them as the canonical entry points but
> re-verify availability before relying on them in the harness build.

---

## Part 1 — COIN (COde → INput)

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

Per the site's "Run it yourself" section, the whole benchmark is one command
against a **published snapshot**. Each project is rebuilt from its pinned
commits and gated by a functional precheck, so you need only the dataset and a
Docker host — no OSS-Fuzz tree, no re-mining.

**Requirements:** Python **≥ 3.12** and **Docker**.

```bash
# install (uv resolves the bundled corpusdb)
uv sync --extra llm

# evaluate an agent against a published COIN snapshot
coin evaluate --dataset you/coin@v1

# view the dashboard
coin show
```

Publishing a snapshot (turns a finished experiment into a citable, immutable
snapshot — not needed to *run* the benchmark):

```bash
docker login ghcr.io
coin publish -e <experiment_id> --version v1 \
    --hf-repo you/coin --registry ghcr.io/you
```

**Environment:** set `LITELLM_URL` / `LITELLM_MASTER_KEY` for the agents;
`HF_TOKEN` to publish. Agents are driven through LiteLLM, so the harness is
model-agnostic. Released under **MIT**; built on **OSS-Fuzz**.

**Local-vs-leaderboard scoring.** Because grading is execution-based and
reproducible from a pinned snapshot, a local `coin evaluate` run of a given
model *should* reproduce that model's leaderboard reach/precision within
variance. This is exactly the property the LOCAL COIN Gym exploits to score
locally against the public numbers without submitting anything externally.

---

## Part 2 — skwaq's self-improvement loop

skwaq is a self-improving, multi-agent **vulnerability analyzer** (18 agents on
the RustyClawd framework, a LadybugDB code-property graph). It is not a
reachability tool, but its **self-improvement machinery** is the pattern this
spike wants to copy. Three Rust crates:

- **skwaq-core** — parsing, graph DB, analysis engine, 18 agent definitions,
  LLM client, durable agent memory.
- **skwaq-gym** — benchmark harness, industry adapters, the self-improvement
  loop with **failure-analyst** and **overfitting-reviewer** agents.
- **skwaq** (CLI) — `clap`-based, 20+ commands including the `gym` subcommands.

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
**precision = TP/(TP+FP)**, **recall = TP/(TP+FN)**, **F1 = 2PR/(P+R)**, plus:

- **Per-CWE family** scoring — `cwe_family()` collapses specific CWEs to their
  parent family (e.g. CWE-121 → CWE-119 memory-safety) so scoring is not brittle
  to exact-ID mismatches.
- **Benchmark vs. adjudicated precision** — a finding on an unlabeled case is a
  *disagreement pending adjudication*, not automatically a false positive. The
  benchmark answer key is not treated as a complete precision oracle.
- **Negative-case calibration** — patched/safe cases track the false-positive
  rate separately; only `critical`-severity findings matching the original CWE
  count as FPs, so pattern noise doesn't inflate FP counts.

### 2.2 The self-improvement loop (five phases)

`skwaq gym improve <suite>` runs an automated cycle. The essential idea: agents
analyze **their own failures**, propose targeted fixes, a reviewer **gates
against overfitting**, accepted patches are applied, and the benchmark re-runs —
keeping a change **only if it improves without regressing**.

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
**overfitting-reviewer** agent, which rejects **~66%** of proposals. It asks
three questions:

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

**Phase 4 — Patch application.** Accepted proposals are applied by type, each
with a narrow, validated strategy (path-canonicalized, exact find/replace or
schema-validated, never partial):

| Kind | Target | Strategy |
|------|--------|----------|
| `NewPattern` | `patterns_source.rs` | append/replace typed `SourcePattern` (regex size-limited) |
| `AgentPrompt` | `agents/*.md` | append after last `##` / exact replace |
| `CweMapping` | `scoring.rs` | find/replace on mapping fns |
| `TaintRule` | CPG SQLite `data_sources`/`data_sinks` | parameterized `INSERT OR IGNORE` |
| `RecipeChange` | `recipes/analysis/*.yaml` | insert stage before `debate:`; YAML re-validated |
| `GroundTruthFix` | `ground_truth/*.toml` | find/replace (extra scrutiny) |

**Phase 5 — Verification.** Re-run the benchmark; **accept only if**: F1 does
not decrease, precision drops ≤ **2%**, and **no per-CWE detection rate
regresses beyond the 2% noise margin** (`CWE_REGRESSION_NOISE_MARGIN`).
Otherwise **roll back all patches**. A training/holdout F1 gap beyond **0.15**
raises an overfitting warning and flags the cycle. Token budgets are bounded
(≈50k tokens/case target, 100k max, ≤20 cases/cycle, 3M-token cycle cap).

**Durable memory.** Each cycle appends to `fn-insights.md` (per-case failure
analysis) and `learned-patterns.md` (patterns discovered, with regex + target
CWE + source case). Future analysts read these to avoid re-proposing rejected
ideas — the loop *remembers*.

### 2.3 Multi-agent debate + structured role-cards

Beyond the gym, skwaq's analysis pipeline uses a **debate** stage. Specialist
agents argue exploitability with **structured output schemas**
(`exploit-analyst-v1`, `defense-analyst-v1`), and a **verdict-synthesizer**
combines them. Key mechanics worth copying:

- **Schema-backed contracts.** `skwaq agents list` shows each agent's role
  title and declared output schema; when structured exploit/defense outputs
  parse, the debate emits **confidence-threshold hints** (`threshold_hint`) that
  act as an auto-confirm/auto-reject gate. If parsing fails, it falls back to
  direct code review rather than trusting free text.
- **Exploitability-led promotion.** `HIGH_CONFIDENCE_CONFIRM` requires a strong
  exploit-side signal *plus* supporting defense agreement — a net-positive score
  alone never auto-promotes. Ambiguous findings are biased toward rejection
  unless direct code evidence is strong.

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
coin-gym profiles                                    # list per-model isolated state
```

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
| 3 | Provision compute (`azlin` VM) + pull COIN snapshot | ⏭ tracked (issue) |
| 4 | Build the LOCAL COIN Gym harness (Part 3) | ⏭ tracked (issue) |
| 5 | Iterative self-improve (baseline vs team; failure-analysis + overfit gate) | ⏭ tracked (issue) |

Phases 3–5 are captured in the tracking issue **"Build LOCAL COIN Gym harness —
phases 3-5"** in `rysweet/Simard`, which carries the harness design above plus
the remaining work (VM provision, harness build, iterative self-improve).
