# COIN benchmark — Phase 1 completion record ("LEARN COIN")

**Status:** Phase 1 (LEARN COIN) of the *build-a-local-coin-benchmark-harness*
goal — **complete**.
**Scope:** local study only. This record does **not** provision any VM and does
**not** post COIN results anywhere externally.

This is the Phase-1 **completion record and verdict** for the LOCAL COIN Gym
umbrella goal. It is deliberately short: the substantive research already landed
on `main` and this note ties the goal's done-criteria to concrete, verifiable
evidence and states an explicit verdict the progress gate can parse.

The four required questions (what COIN measures, how to install/run it locally,
how it scores, how its leaderboard is structured) are answered in full — with
sandbox-verified commands, the outcome-code table, aggregate metrics, and the
published leaderboard snapshot — in the canonical Phase-1 primer:

- **Canonical primer (single source of truth):**
  [COIN benchmark — Phase 1 primer](coin-benchmark.md)
- **Companion depth (target-construction signal algebra + LOCAL COIN Gym design
  sketch):** [COIN benchmark & skwaq gym study](coin-benchmark-and-skwaq-study.md)
- **Phase 2 companion:**
  [skwaq self-improvement loop](skwaq-self-improvement-loop.md)

> **Prior-art / honesty note.** Phase 1 was shipped by
> [PR #2763](https://github.com/rysweet/Simard/pull/2763) (`Closes #2752`,
> merged), which added `docs/research/coin-benchmark.md`; the Phase-1 tracking
> issue [#2752](https://github.com/rysweet/Simard/issues/2752) is **closed**.
> This record does **not** re-derive that research — it consolidates the
> completion state, restates the local-run recipe in one place, and supplies the
> explicit verdict line that the progress gate needs.

---

## The four questions — one-line answers

| # | Question | Answer | Depth |
|---|----------|--------|-------|
| 1 | **What** it measures | Code-reasoning by **reachability**: emit a concrete INPUT that drives execution to a chosen line `ℓ` at a pinned commit `c`; graded by **running the code** on a coverage-instrumented build — binary, objective, contamination-free. **The input is the proof.** | [§1](coin-benchmark.md#1-what-coin-measures) |
| 2 | **How** to obtain/install/run locally | `git clone https://github.com/COIN-Bench/coin` → `uv sync --extra llm` → `coin evaluate --dataset <hf-repo> --revision <tag>` → `coin show`. Needs **Python ≥ 3.12** + a **Docker** daemon. MIT-licensed, built on OSS-Fuzz. | [§2](coin-benchmark.md#2-how-to-obtain-install-and-run-it-locally) |
| 3 | **How** it scores | Per-target **binary** reach (outcome codes `R/W/A/T/N/E`), aggregated as **reach rate** (fraction provably reached — primary) and **precision** (fraction of *submitted* inputs that reached — exposes over-claiming), over **70 targets** (**35** frontier), **2 h** budget/target. | [§3](coin-benchmark.md#3-how-it-scores) |
| 4 | **Leaderboard** structure | Ranks **agents = model × scaffold** on `Reach % · Precision % · Reached (n/70) · Frontier (n/35) · $/reached · Total $`. "Submission" = a reproducible **published dataset** snapshot (`coin publish`), not an opaque score upload. Our use is **LOCAL-ONLY — never posted externally.** | [§4](coin-benchmark.md#4-leaderboard-structure-and-our-local-only-constraint) |

The published headline: the **frontier is a wall** — aggregate reach on the 35
frontier targets is ~**0.7 %** vs. ~**38 %** on non-trivial-reachable targets.

---

## Local run recipe (exact commands)

Verified against upstream on **2026-07-07** (`README.md` on
`COIN-Bench/coin@main`; site + repo + HF dataset all returned HTTP 200; the repo
was cloned at commit `83261b0` and its CLI exercised in the sandbox during the
Phase-1 primer work). LOCAL-ONLY: only `coin evaluate` / `coin show` are used;
`coin publish` and any `--hf-repo` / `--registry` push are **out of scope**.

```bash
# 1. Obtain
git clone https://github.com/COIN-Bench/coin.git
cd coin

# 2. Install (recommended: uv resolves the bundled corpusdb sub-package)
uv sync --extra llm --extra dev        # core + LLM SDKs + test deps
#   plain-pip alternative:
#   pip install -e ./corpusdb          # bundled local path dep, install FIRST
#   pip install -e ".[llm]"            # add ".[dev]" for tests

# 3. Configure
cp config.example.yaml config.yaml
#   then set, in a .env (or exported):
#     LITELLM_URL, LITELLM_MASTER_KEY   # LiteLLM proxy the eval agents call

# 4. Run against a PUBLISHED snapshot (no pipeline run needed)
coin evaluate --dataset <hf-repo> --revision <tag>   # e.g. --dataset you/coin@v1
coin show                                            # local dashboard at http://localhost:8765
```

**Requirements:** Python ≥ 3.12 and a working Docker daemon (build/eval stages
shell out to Docker; eval agents run in privileged Docker-in-Docker). The CLI is
`coin` (or `python -m coin`); `coin <command> --help` lists flags. Building a
dataset from scratch is an advanced multi-stage pipeline and is **not** required
to run the benchmark against a published snapshot.

---

## Phase-1 done-criteria → evidence

| Done-criterion (from goal + issue #2752) | Evidence | Status |
|------------------------------------------|----------|--------|
| Answer WHAT COIN measures (reachability, run-graded) | primer [§1](coin-benchmark.md#1-what-coin-measures) | ✅ |
| Answer HOW to obtain/install/run locally | primer [§2](coin-benchmark.md#2-how-to-obtain-install-and-run-it-locally) + Local run recipe above (upstream-verified 2026-07-07) | ✅ |
| Answer HOW it scores | primer [§3](coin-benchmark.md#3-how-it-scores) | ✅ |
| Answer LEADERBOARD structure | primer [§4](coin-benchmark.md#4-leaderboard-structure-and-our-local-only-constraint) | ✅ |
| Committed doc lands on `main` | `docs/research/coin-benchmark.md` (PR #2763, merged) | ✅ |
| Phase-1 tracking issue closed | [#2752](https://github.com/rysweet/Simard/issues/2752) — CLOSED | ✅ |
| LOCAL-ONLY constraint recorded (never post external results) | primer §4 + this record | ✅ |
| Explicit local-run recipe in one place | this record | ✅ |
| Explicit verdict line for the progress gate | below | ✅ |

---

## Verdict

**VERDICT: PASS — Phase 1 (LEARN COIN) is complete.** All four required
questions are answered with verified detail on `main`
(`docs/research/coin-benchmark.md`), the Phase-1 tracking issue #2752 is closed,
and the local-run recipe is verified against upstream (2026-07-07). No new
research was required this cycle; this record consolidates the completion state
and supplies the explicit verdict.

Next bounded slice: **Phase 2 — study skwaq's self-improvement gym**
(see [skwaq self-improvement loop](skwaq-self-improvement-loop.md)). Phase 2 is
**out of scope for this cycle.**

<!-- progress-gate: PROGRESS: honest-partial (administrative Phase-1 ratification; substantive Phase-1 already merged in PR #2763). Do NOT record a PROGRESS bump on the umbrella for this consolidation. -->
