# skwaq self-improvement loop — Phase 2 study ("port the gym to COIN")

**Status:** research spike — Phase 2 (STUDY skwaq) of the LOCAL COIN Gym goal.
**Scope:** local study only. This note does **not** provision any VM and does
**not** post any results externally.

This is the focused Phase-2 companion to the
[COIN benchmark primer](coin-benchmark.md). It answers one question with
verified, actionable detail:

> **Which mechanisms of skwaq's self-improvement gym do we port into a LOCAL
> COIN Gym, and how do they change when the oracle is _reachability by
> execution_ instead of _vulnerability detection_?**

Every mechanism below was read directly from a fresh clone of the skwaq
source (not a summarizer). The longer end-to-end LOCAL COIN Gym design sketch
lives in the [combined study](coin-benchmark-and-skwaq-study.md), Part 3; this
document is the skwaq half made self-contained and the **port map** explicit.

## Sources (verified directly)

`github.com/rysweet/skwaq` cloned to a scratch dir (`~/.simard/research/skwaq`,
**not** under a guardrail-protected path) at commit `9a6b7d8` on 2026-07-07.
Facts below cite the exact file that establishes them.

| Mechanism | Source of truth (in skwaq) | Verified |
|-----------|----------------------------|----------|
| Gym harness, suites, modes | `crates/gym/src/lib.rs` | read |
| Scoring (TP/FP/FN, per-family, negative calibration) | `crates/gym/src/scoring.rs` | read |
| Self-improvement loop (5 phases, budgets, rollback) | `crates/gym/src/improve.rs` · `docs/gym-self-improvement.md` | read |
| Overfitting-reviewer gate (3 questions) | `agents/overfitting-reviewer.md` | read |
| Failure-analyst (structured proposals) | `agents/failure-analyst.md` | read |
| Multi-agent debate + output schemas | `agents/exploit-analyst.md` · `agents/defense-analyst.md` · `agents/verdict-synthesizer.md` | read |
| Multi-model profiles | `crates/gym/src/profiles.rs` · `docs/gym-profiles.md` | read |

> The `~/.simard/research/skwaq` clone is a throwaway study checkout. It is
> **not** vendored into Simard and nothing in this repo depends on it.

---

## 1. What skwaq is (one paragraph)

skwaq is a self-improving, multi-agent **vulnerability analyzer** built on a
code-property graph. It ships three Rust crates — **`core`** (parsing, graph
DB, the LLM client, durable agent memory, and the **18** agent role-cards under
`agents/`), **`gym`** (the benchmark harness + the self-improvement loop), and
**`cli`** (the `skwaq` command). It is **not** a reachability tool, but its
self-improvement machinery — analyze your own failures → propose fixes → gate
against overfitting → verify or roll back — is the exact pattern the LOCAL COIN
Gym copies.

---

## 2. The Gym harness

`skwaq gym run <suite>` runs a benchmark suite through a per-suite adapter and
scores each case. Adapters registered in `crates/gym/src/lib.rs`: **fixtures,
realworld, juliet, cyberseceval, owasp, binpool, cybergym**. Three execution
modes make baseline-vs-team measurement first-class:

| Mode | Flag | What it measures |
|------|------|------------------|
| Pattern-only | `--quick` | fast regex/pattern baseline |
| Full | (default) | patterns + LLM agents + synthesis (best accuracy) |
| LLM-only | `--llm-only` | agent understanding **without** pattern help |

(The mode is carried on the gym config as `quick_mode` / `llm_only` in
`lib.rs`.)

**Scoring** (`crates/gym/src/scoring.rs`) is standard information-retrieval —
`precision = TP/(TP+FP)`, `recall = TP/(TP+FN)`, `F1 = 2PR/(P+R)` — with three
robustness features that matter for a self-improving loop:

- **Per-CWE-family scoring.** `cwe_family()` collapses a specific CWE to its
  parent family (e.g. CWE-121 → CWE-119 memory-safety) so scoring is not
  brittle to exact-ID mismatch.
- **Benchmark vs. adjudicated precision.** A finding on an *unlabeled* case is
  a *disagreement pending adjudication*, not automatically a false positive
  (`adjudicated_precision` stays `None` until an adjudication pass runs). The
  answer key is not treated as a complete precision oracle.
- **Negative-case calibration.** On known-safe/patched cases, only
  `critical`-severity findings whose CWE matches the original family count as
  false positives, so pattern noise does not inflate the FP rate
  (`NegativeCaseCalibration` in `scoring.rs`).

---

## 3. The self-improvement loop (five phases)

`skwaq gym improve <suite>` runs one automated cycle
(`crates/gym/src/improve.rs`, mirrored in `docs/gym-self-improvement.md`):

```
Benchmark → Failure Analysis → Proposal Generation → Overfitting Review → Patch Application
    ↑                                                                           |
    └───────────────────── Re-benchmark & Verify ───────────────────────────────┘
```

**Phase 1 — Benchmark & collect.** Run the suite, score every case, collect
false negatives with their source. The case set is split into **training** and
**holdout** partitions (`holdout_fraction` in `improve.rs`); only training
cases feed the analyst, and the holdout validates that gains **generalize**.

**Phase 2 — Failure analysis.** The **failure-analyst** agent
(`agents/failure-analyst.md`, `claude-opus-4.6`, `max_turns: 25`) examines each
false negative with enriched graph context (imports, data sources, cross-file
call graph, string refs), diagnoses *why* detection failed, and emits
**structured proposals** — one of `AGENT_PROMPT`, `TAINT_RULE`, `CWE_MAPPING`,
`NEW_PATTERN`, `DEEPER_ANALYSIS`, `NEW_AGENT_CAPABILITY`, `GROUND_TRUTH_ERROR`.
It is explicitly told to **prefer graph-aware fixes** (`AGENT_PROMPT`,
`TAINT_RULE`) over brittle regex (`NEW_PATTERN`), and **every proposal must cite
`Evidence:`** (a knowledge-base hit or a durable-memory recall) or the cycle
fails.

**Phase 3 — Overfitting review (the gate).** See §4.

**Phase 4 — Patch application.** Accepted proposals are applied by type, each
with a narrow, validated strategy — exact find/replace or schema-validated,
**never a partial apply** (`docs/gym-self-improvement.md`):

| Kind | Target | Strategy |
|------|--------|----------|
| `NewPattern` | `crates/core/src/analysis/patterns_source.rs` | append/replace typed pattern (size-limited) |
| `AgentPrompt` | `agents/*.md` | append after last `##` / exact replace |
| `CweMapping` | `crates/gym/src/scoring.rs` | find/replace on mapping fns |
| `TaintRule` | CPG SQLite `data_sources`/`data_sinks` | parameterized `INSERT OR IGNORE` |
| `RecipeChange` | `recipes/analysis/*.yaml` | insert before `debate:`; YAML re-validated |
| `GroundTruthFix` | `data/gym/ground_truth/*.toml` | find/replace (extra scrutiny) |

Duplicate proposals are dropped: a patch whose replacement text has
**Jaccard token-overlap ≥ 0.6** with an already-accepted patch for the same
file is skipped (`improve.rs`).

**Phase 5 — Verification.** Re-run and **accept only if there is no
regression**. `has_any_regression()` is `has_cwe_regression() ||
has_precision_regression()`; a per-family detection drop counts only if it
exceeds the noise margin `CWE_REGRESSION_NOISE_MARGIN = 0.02` (`scoring.rs`),
and negative-case FP rate must not rise (`precision_regression()`). Otherwise
**roll back**. A training/holdout F1 gap beyond
`HOLDOUT_OVERFITTING_GAP_THRESHOLD = 0.15` raises an overfitting warning.
Failure-analyst token budgets are bounded per case
(`FAILURE_ANALYST_TARGET_BUDGET_PER_CASE = 50_000`, `…MAX… = 100_000`).

---

## 4. The overfitting-reviewer gate

Every proposal passes through the **overfitting-reviewer**
(`agents/overfitting-reviewer.md`, `claude-opus-4.6`, `max_turns: 15`) — the
mechanism that keeps the loop from **building to the benchmark**. skwaq's own
framing is that this gate rejects a large share of proposals (about
**two-thirds** in practice); its role-card asks three questions:

1. **Real-world generality** — would this detect vulns in *real* production
   code, or only match benchmark-specific naming
   (`CWE121_Stack_Based_Buffer_Overflow__`, `cgc_allocate`)? Reject the latter.
2. **Pattern specificity** — reject wildcard patterns (`\w+_read`,
   `\w+_receive`) that fire on safe production APIs; accept specific,
   known-dangerous functions (`\brecv\s*\(`).
3. **CWE-mapping accuracy** — reject mappings that inflate scores by mapping to
   a broader family (e.g. `format_string → CWE-119`).

It emits a structured verdict — `ACCEPT | REJECT | MODIFY` plus
`Overfitting risk` and `Real-world applicability` (LOW/MEDIUM/HIGH). Its rule
of thumb: **when in doubt, favor precision over recall.** Rejected proposals
and their reasons are logged to `data/knowledge/fn-insights.md` so later cycles
do not re-propose them.

---

## 5. Multi-agent debate + structured role-cards

Beyond the gym, skwaq's analysis pipeline runs a **debate** stage with
**schema-backed** role-cards. `agents/exploit-analyst.md` and
`agents/defense-analyst.md` declare `output_schema: exploit-analyst-v1` /
`defense-analyst-v1`; `agents/verdict-synthesizer.md` combines them. When
structured outputs parse, the debate emits a per-finding **`threshold_hint`**
that acts as an automation gate (`agents/verdict-synthesizer.md`):

- `HIGH_CONFIDENCE_CONFIRM` — strong exploit signal **plus** defense agreement;
  may auto-confirm if code evidence stays coherent.
- `HIGH_CONFIDENCE_REJECT` — signals strongly favor rejection.
- `REVIEW_REQUIRED` — **must not** auto-confirm; read the code, require precise
  evidence, default toward rejection if support is weak.

If structured parsing fails, the gym falls back to direct code review rather
than trusting free text (schema validation in `crates/gym/src/agentic.rs`).
Two lessons transfer: **(a)** structured, schema-validated agent I/O with a
graceful fallback, and **(b)** a confidence gate that biases toward
**abstention** on weak evidence.

**Durable memory.** Each cycle appends to `fn-insights.md` (per-case failure
analysis) and `learned-patterns.md`; future analysts read these to avoid
re-proposing rejected ideas — the loop *remembers*.

**Profiles.** `skwaq gym profile create <name> --backend <b> --model <m>` gives
each model an isolated results DB, memory graph, and telemetry
(`crates/gym/src/profiles.rs`), so baseline-vs-team and model-vs-model
comparisons never cross-contaminate.

---

## 6. Port map: skwaq → COIN Gym

This is the deliverable — the explicit mapping of **which skwaq mechanisms we
port** and how each one changes when the oracle becomes *reachability by
execution* (see the [COIN primer §3](coin-benchmark.md) for the scoring model).

| skwaq mechanism | COIN Gym analog | What changes |
|-----------------|-----------------|--------------|
| Gym harness + per-suite adapters | Wrap `coin evaluate` as the eval engine | The oracle is Docker + instrumented replay; we **never** re-implement it. |
| Modes `--quick` / full / `--llm-only` | **single-model baseline** vs **multi-agent team** | Two interchangeable "agent-under-test" strategies behind one interface. |
| Score TP/FP/FN, per-CWE-family | **reach rate / precision**, per family (frontier vs non-trivial reachable) | IR metrics → COIN's `R/W/A/T/N/E` histogram + reach/precision. |
| Failure-analyst on false negatives | Analyze **unreached targets** (`W/T/N`) | Diagnose *why the input missed* (wrong format, unflipped branch, missed constraint, wrong harness) and propose a **general reachability tactic**. |
| Structured proposal types + mandatory `Evidence:` | Structured tactic proposals with cited evidence | e.g. "for format-gated decoders, satisfy the magic-byte/header validator before targeting deep lines" — never a hardcoded input. |
| **Overfitting-reviewer gate (3 questions)** | Reject **input-memorizing / target-specific** tactics; accept only tactics that plausibly generalize | The direct analog of rejecting `CWE121_…`-style patterns: reject anything keyed to a specific target id / project name. |
| Train/holdout split + no-regression accept + rollback | **Held-out fresh COIN targets** as the anti-overfit oracle | Stronger than skwaq's static holdout: you literally *cannot* memorize an input to a line that has never been reached. Keep a tactic **iff** reach improves and precision does not drop; else roll back. |
| `threshold_hint` confirm/reject/review gate | Submit-vs-**abstain** gate | Precision punishes over-claiming, so a low-confidence input should `A` (abstain), not `W` (wrong submit). |
| Schema-backed role-cards + parse-fail fallback | Schema'd `reacher` / `skeptic` / `synthesizer` outputs | Keeps the team's I/O machine-checkable; fall back to a single-model attempt if parsing fails. |
| Durable memory (`fn-insights.md`, `learned-patterns.md`) | Remembered reachability tactics per project/harness family | Later runs start from learned heuristics instead of cold. |
| Profiles (isolated per-model state) | Per-model isolated runs vs. the public leaderboard | Reproducible baseline-vs-team and model-vs-model comparisons. |

### Why COIN is a *stronger* substrate than skwaq's suites

- **Objective, execution-graded** signal (reach / no-reach): no LLM judge to
  game, so the loop optimizes a real quantity.
- **Contamination-resistant & self-refreshing**: the built-in defense against
  the exact failure the overfitting-reviewer guards against. The gate's job
  narrows to one crisp rule — *accept tactics that generalize the
  semantic→input reasoning; reject anything that encodes a specific answer* —
  which is easier to adjudicate than skwaq's precision/recall trade-off because
  reach is a hard per-target pass/fail.
- **Model-agnostic** via LiteLLM: trivial to A/B a single model vs. an
  orchestrated team on identical targets.

### What we port first (prioritized)

1. **Agent-under-test interface** with the two modes (baseline / team) — the
   minimum needed to produce any score.
2. **Scorer + leaderboard comparator** (reach/precision by family; diff local
   vs published numbers to catch harness/config bugs).
3. **Failure-analyst on unreached targets** producing general tactics.
4. **Overfitting-reviewer gate** verified on **held-out fresh** targets, with
   keep-iff-improves-without-regression + rollback.
5. **Durable tactic memory** and **profiles** last.

### What we explicitly do **not** port

- skwaq's CWE/taint-specific proposal *types* (`TaintRule`, `CweMapping`, …) —
  COIN has no CWE taxonomy; proposals are reachability tactics, not detectors.
- Pattern/regex machinery (`patterns_source.rs`) — irrelevant to reachability.

---

## 7. Out of scope for this spike

- No Azure/VM provisioning (a later phase).
- No external submission or posting of COIN results, and no leaderboard entry.
- No harness code — this note plus the [COIN primer](coin-benchmark.md) and the
  [combined study](coin-benchmark-and-skwaq-study.md) are the design the harness
  phases (tracked in issue *"Build LOCAL COIN Gym harness — phases 3-5"*) build
  against.

## Summary

| skwaq mechanism | Ported as |
|-----------------|-----------|
| Gym harness | `coin evaluate` wrapped as the eval engine (oracle never re-implemented) |
| Baseline vs LLM-only modes | single-model baseline vs multi-agent team |
| Failure-analyst on FNs | failure-analyst on **unreached** targets → general reachability tactics |
| Overfitting-reviewer gate | reject **input-memorizing** tactics; verify on **held-out fresh** targets |
| Train/holdout + rollback | keep tactic **iff** reach↑ & precision not↓, else roll back |
| Debate `threshold_hint` | submit-vs-**abstain** gate (precision punishes over-claiming) |
| Durable memory + profiles | remembered tactics per harness; isolated per-model runs |
