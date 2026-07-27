---
title: Agentic-recipes-first reasoning principle
description: >
  Simard's REASONING prompt assets carry one canonical, byte-identical
  design principle — when a problem requires intelligence or judgment, solve it
  by composing, reusing, or inventing deterministic recipes of agentic steps run
  via the recipe runner, never by writing brittle imperative code or one-off
  heuristics. Imperative code is confined to the thin deterministic rails
  (dispatch, I/O, storage, scheduling ticks); the reasoning itself lives in
  agentic recipe steps. The block is embedded in all nine OODA / Overseer /
  planning reasoners, references (does not duplicate) engineer_system.md's G3,
  and is pinned in place by a drift-guard test.
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./unified-recipe-brain.md
  - ./prompt-driven-ooda-brain.md
  - ./agentic-merge-queue-reasoning.md
  - ./overseer-root-cause-why.md
  - ./no-progress-root-cause-resolution.md
  - ./durable-documentation-policy.md
---

# Agentic-recipes-first reasoning principle

> **Status: implemented.** This page is the source-of-truth description of the
> principle. It defines, in one place, the canonical block that is embedded in
> the reasoning assets, the nine assets that carry it, and the drift-guard test
> that pins the copies byte-identical. The block is present in all nine assets
> and `tests/prompt_agentic_recipes_principle.rs` enforces it — that test fails
> closed if any copy is missing, altered, or mis-ordered. Sections below describe
> the enforced target state.

## The principle

A single canonical block must be embedded, byte-identical, into every prompt
asset where Simard decides **how** to solve a problem:

> **Agentic-recipes-first (extends engineer `G3`).** When a problem requires
> intelligence or judgment, solve it by composing, reusing, or inventing
> deterministic recipes of agentic steps run via the recipe runner — never by
> writing brittle imperative code or one-off heuristics. Reuse existing
> recipes/sub-recipes first; invent a new agentic recipe when none fits.
> Imperative code is only for the thin deterministic rails (dispatch, I/O,
> storage, scheduling ticks) — the reasoning itself lives in agentic recipe
> steps. This is the reasoning-time application of engineer `G3`
> (`engineer_system.md`, "Engineering Guidelines"); it does not change your
> output contract below.

The meaning is fixed. Wording is tuned only to fit each host prompt's voice; the
pinned canonical sentence and its keyword invariants (`recipe runner`, thin
deterministic rail) must be identical everywhere and are enforced by test.

## The problem this fixes

Simard's Overseer failed to self-heal a bug that crash-looped **seven** standing
goals **286+ times**. The reflexive fix first proposed was imperative Rust
plumbing:

- wiring `record_step_failure` into every failure-origin call site,
- consecutive-failure counters,
- an N-identical-failure threshold heuristic.

That is exactly the antipattern to eliminate. The operator, by contrast,
diagnosed the entire crash-loop **agentically** in a handful of journal reads:

```
journalctl --user -u simard-ooda
simard status
simard goal list
```

The lesson generalizes: the problems Simard faces that *require intelligence* —
health assessment, root-cause analysis, remediation decisions, scheduling,
verification, admission, cleanup — must be solved as **agentic recipes on thin
deterministic rails**, not as imperative code or hand-tuned heuristics. Every
counter and threshold added imperatively is a place where judgment ossifies into
a brittle constant that cannot adapt to a situation its author did not foresee.

## Where the block lives (nine reasoning assets)

The block must be embedded advisorily — after each prompt's ROLE / guideline
preamble and **before** any `OPTIONS` / `DECISION` / output-contract section, so
it frames *how to reason* without touching *what to emit*.

| Reasoning stage | Asset |
| --- | --- |
| OODA — Observe/brain | `prompt_assets/simard/ooda_brain.md` |
| OODA — Orient | `prompt_assets/simard/ooda_orient.md` |
| OODA — Decide | `prompt_assets/simard/ooda_decide.md` |
| Overseer — Observe | `prompt_assets/simard/overseer/observe.md` |
| Overseer — Escalation triage | `prompt_assets/simard/overseer/escalation_triage.md` |
| Overseer — Deploy gate | `prompt_assets/simard/overseer/deploy_gate.md` |
| Planning — Goal decomposition | `prompt_assets/simard/goal_decomposition.md` |
| Planning — Improvement curator | `prompt_assets/simard/improvement_curator_system.md` |
| Planning — Engineer planning | `prompt_assets/simard/engineer_planning.md` |

`engineer_system.md` is intentionally **not** in this list. It already carries
recipe-runner enforcement and guideline **G3** ("prefer agentic steps over
brittle parsing; prefer recipes/prompts over code"). The canonical block
**references and extends** G3 rather than restating it — there is one source of
truth for the engineering guideline, and the nine reasoners point at it.

## Why per-file embedding, not a shared-injection seam

Simard's prompts load one file at a time (`include_str!` fallbacks plus explicit
path references from recipes). There is no concatenation seam that would let a
single principle fragment be injected into every reasoner at load time, and
**adding one would be heavy imperative wiring** — precisely the kind of
plumbing this principle discourages, and forbidden by the "thin rail only"
constraint for this change.

So the block must be embedded statically and identically in all nine assets. To
keep nine copies from drifting apart, the byte-consistency guarantee is moved
onto a thin deterministic rail: a test (see below). This is itself an
application of the principle — the *judgment* (what the block says) lives in the
prompt; only the *mechanical invariant* (all copies match, in the right place)
is enforced by code.

## Drift-guard test

`tests/prompt_agentic_recipes_principle.rs` is a thin verification rail. It
asserts that:

1. the pinned canonical sentence — "When a problem requires intelligence or
   judgment, solve it by composing, reusing, or inventing deterministic recipes
   of agentic steps run via the recipe runner" — appears in **all nine** target
   assets;
2. the keyword invariants `recipe runner` and the thin-deterministic-rail phrase
   appear alongside it in each file;
3. `engineer_system.md` is referenced by the block (the G3 source of truth is
   not duplicated);
4. in each asset the canonical sentence appears **before** the first output
   anchor (`OPTIONS`, `DECISION`, or the "Return ONLY" JSON contract line),
   so a later edit cannot silently move the advisory block below the output
   contract without failing the test — a positional invariant, not just a
   presence check.

The test fails **closed**: if any copy is missing, edited, partially
updated, mis-ordered, deleted, or tampered with, the assertion breaks. It checks
keyword invariants and ordering, not full snapshots, so additive prose in any
host prompt stays safe.

Run it with:

```bash
cargo test --test prompt_agentic_recipes_principle
```

The pre-existing `tests/prompt_assets.rs` and
`tests/prompt_autonomy_contract.rs` continue to pass unchanged — the added block
is advisory framing placed away from output-contract sections, and those suites
assert keyword invariants rather than prompt snapshots.

## What this does and does not change

- **Does not change any output contract.** The block is advisory framing. Each
  reasoner still emits exactly the `DECISION` / `OPTIONS` / JSON payload it did
  before; the closing "does not change your output contract" line makes that
  explicit at every site.
- **Narrows, not widens, imperative authority.** It confines imperative code to
  the thin deterministic rails (dispatch, I/O, storage, scheduling ticks) and
  routes all judgment through agentic recipes.
- **No Rust reasoning code, no new runtime surface.** Assets remain static
  `include_str!` content with no dynamic interpolation or user input — no
  injection vector is introduced.
- **Reuse-first.** The block directs Simard to reuse existing recipes and
  sub-recipes before inventing a new one, and to invent a new agentic recipe
  only when none fits.

## How to apply it (for engineer sessions)

When a task asks Simard to add "detection", "a threshold", "a counter", "a
retry policy", "a health check", or any other decision that depends on
context-sensitive judgment, the principle says:

1. **Look for an existing recipe or sub-recipe** that already performs the
   reasoning (e.g. the Overseer root-cause `why` recipe, the merge-queue
   reasoning pass). Reuse it.
2. **If none fits, invent a new agentic recipe** — a structured brief plus agent
   reasoning that emits a typed decision — and run it via the recipe runner.
3. **Write imperative code only for the rail** that dispatches the recipe, moves
   its I/O, persists its result, or schedules its tick. Never encode the
   judgment as a constant, a hand-tuned heuristic, or a brittle parser.

If you find yourself hard-coding "after N identical failures, do X", stop: that
threshold is a judgment, and judgment belongs in an agentic recipe step.

## Related

- [Unified recipe brain](./unified-recipe-brain.md)
- [Prompt-driven OODA brain](./prompt-driven-ooda-brain.md)
- [Agentic observe/orient merge-queue reasoning](./agentic-merge-queue-reasoning.md)
- [Overseer root-cause "why"](./overseer-root-cause-why.md)
- [No-progress root-cause resolution](./no-progress-root-cause-resolution.md)
- [Overseer Signal operator-liaison](../reference/overseer-signal-liaison.md) — an agentic-first application: operator-message interpretation lives in the `operator-liaison` recipe; Rust is a thin receive/authorize/dedup/dispatch rail.
- [Overseer autonomous PR rework loop](../reference/overseer-rework-loop.md) — an agentic-first application: the fixable-vs-escalate judgment lives in the merge-judge prompt; Rust is a thin cap/dedup/dispatch rail.
- [Durable documentation policy](./durable-documentation-policy.md)
- Engineering Guidelines G1–G4 — `CONTRIBUTING.md`, and `engineer_system.md`
  "Engineering Guidelines" section (canonical G3).
