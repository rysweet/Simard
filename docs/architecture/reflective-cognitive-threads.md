---
title: Reflective cognitive threads act via tools (no JSON envelope)
description: >
  How the reflective cognitive threads (reflection, metacognition, salience, narrative,
  consolidation, prospection, values-deliberation, analogy, operator-model) perform their
  side effects by CALLING existing `simard` tools from inside their recipes — exactly like
  the distiller (issue #2679) — instead of printing a JSON envelope for Rust to scrape and
  re-act on. Documents the single-layer contract: a thread checks its gate, triggers its
  recipe, and records ran/health from the recipe's EXIT STATUS alone. Supersedes the
  emit→parse→act layer introduced by PR #3142.
last_updated: 2026-07-26
owner: simard
doc_type: concept
related:
  - ./distillation-semantic-handoff.md
  - ../reference/simard-memory-remember-cli.md
  - ../reference/simard-cognition-salience-signal-cli.md
  - ../reference/cognitive-thread-scheduling.md
  - ../howto/configure-reflective-cognitive-threads.md
  - ../howto/add-a-new-cognitive-thread.md
  - ../concepts/agentic-recipes-first-principle.md
  - ../memory.md
---

# Reflective cognitive threads act via tools (no JSON envelope)

!!! note "Status — implemented; superseding PR for #3142; default-ON (opt-out) since #4845"
    The nine reflective cognitive threads are wired into the `Mind` scheduler and
    run on their configured cadence. Every thread is **ENABLED by default (opt-out)**
    behind a **default-ON double gate** (master + per-thread; a thread runs unless
    a gate is set to an explicit falsy token). Each thread's
    side effects are performed **inside its recipe** by calling existing `simard`
    command-line tools — there is **no JSON envelope**, no output file, and **no
    Rust-side parse** of recipe stdout. A thread's only job is: *check gate →
    trigger recipe → record `ran`/`health` from the recipe's exit status.*

## Problem statement

Simard runs a "mind of many processes": the authoritative OODA loop plus a set
of background [cognitive threads](../reference/cognitive-thread-scheduling.md),
each on its own cadence. Nine of those are **reflective** threads — they stand
back from the current work and write durable knowledge (facts, procedures, a
narrative, a values appraisal, a salience ranking) back into Simard.

The first cut of these threads (PR #3142) reinvented a pattern the operator has
**forbidden** and that the distiller already removed in
[#2679](https://github.com/rysweet/Simard/issues/2679): each recipe **printed a
JSON envelope**, Rust **scraped and deserialized** that stdout, and each thread
then re-performed the side effect (`store_fact` / `store_procedure` /
`propose_goal` / `write_signal`) from the parsed fields — often with
`.unwrap_or("")` silent defaults. That is the single failure surface the
distillation handoff was built to eliminate: one trailing comma, one stray log
line, or one launcher banner makes the strict parse fail and discards the entire
result **silently**.

This document describes the **finished, reworked** design: reflective threads
follow the [distillation semantic handoff](./distillation-semantic-handoff.md)
exemplar. The recipe **is** the reasoning surface **and** the acting surface;
its tool calls are its only output.

## The exemplar we copied

`prompt_assets/simard/recipes/distill-episodes.yaml` is the template. The
distiller agent calls `simard memory remember` once per fact; the recipe states
plainly *"There is no output file and no JSON to print."* Effects land through
the daemon's authoritative write-boundary gate (secret-scrub, concept-key
validation, grounding, dedup, quarantine) — all enforced **inside the tool**,
regardless of caller. Simard interprets the recipe **by its exit status alone.**

See [Agentic recipes, first principle](../concepts/agentic-recipes-first-principle.md).

## The single-layer contract

Before (PR #3142) — three layers, one fragile parse:

```
recipe  ── prints strict-JSON envelope ──▶ recipe_rail (classify stdout as JSON)
                                               │
                                               ▼
              thread.tick: envelope.get("field").unwrap_or("")
                           → ctx.memory.store_* / propose_goal / write_signal
```

After (reworked) — one layer, the tool call IS the effect:

```
thread.tick: gate → assemble read-only context file(s) → trigger recipe
                                               │  (child process; SIMARD_MEMORY_SOCKET exported)
                                               ▼
   recipe: simard memory remember …   | simard memory remember-procedure …
         | simard goal add …          | simard cognition salience-signal …
           (NO JSON, NO output file)
                                               │
                                               ▼
   thread.tick records: ran = (exit == 0); health from exit status + stderr tail
```

**Rust never parses recipe stdout for semantic content.** The invoker
(`src/cognitive_threads/recipe_rail.rs`) is a **pure runner**: it spawns the
recipe with the security fence and context, exports `SIMARD_MEMORY_SOCKET` into
the child env (mirroring the distiller), and returns **only** a success/failure
verdict plus a bounded stderr tail for health logging. The classifier
(`classify_recipe_stdout`), the step-output parser (`parse_step_output`), the
`extract_json_payload` call-path, and every JSON-carrying `InvokeResult` variant
are **gone**.

## Which tool each effect uses

| Effect the recipe wants | Tool it calls | Notes |
| --- | --- | --- |
| Durable semantic **fact** | [`simard memory remember`](../reference/simard-memory-remember-cli.md) | One process = one fact; scalar flags only. |
| Durable **procedure** | [`simard memory remember-procedure`](../reference/simard-memory-remember-cli.md) | One process = one procedure. |
| Propose a **goal** | `simard goal add <priority> "<desc>"` | Durable via the goal-board store; capacity-capped and **loud** at `MAX_ACTIVE_GOALS`. |
| Advisory **salience ranking** | [`simard cognition salience-signal`](../reference/simard-cognition-salience-signal-cli.md) | Clamps/validates every numeric field and atomically writes `state/salience_signal.json`. |
| Durable salience **rationale** | [`simard memory remember`](../reference/simard-memory-remember-cli.md) | The free-text `reason` per goal lands as a durable `salience:<goal_id>` fact — kept **out** of the numeric signal file. |

The `salience` thread is the one thread whose recipe makes **two** kinds of tool
call: `simard memory remember` for the free-text `salience:<goal_id>` rationale
facts **and** `simard cognition salience-signal` for the numeric-only ranking.
Both are direct tool calls; neither is a JSON envelope. Dropping either one
silently loses part of the appraisal, so the reworked `salience-appraise` recipe
must retain both.

Recipes carry large context (episode batches, prior narrative) via **files or
stdin**, never argv/env — argv has an `E2BIG` ceiling. Each recipe's prompt ends
with an explicit reminder: *"There is no JSON to print; your tool calls ARE the
effect. Simard reads this recipe by its exit status."*

## The nine reflective threads

Each thread maps to one recipe and one per-thread gate. All are additive and
**ENABLED by default (opt-out)** since #4845.

| Thread (`threads/*.rs`) | Recipe (`prompt_assets/simard/recipes/`) | Per-thread gate | Primary effect |
| --- | --- | --- | --- |
| `reflection` | `reflect-postmortem.yaml` | `SIMARD_THREAD_REFLECTION_ENABLED` | Post-mortem facts + optional goal |
| `metacognition` | `metacognition-appraise.yaml` | `SIMARD_THREAD_METACOGNITION_ENABLED` | Appraisal facts about Simard's own process |
| `salience` | `salience-appraise.yaml` | `SIMARD_THREAD_SALIENCE_ENABLED` | Numeric salience ranking (`cognition salience-signal`) **and** durable `salience:<goal_id>` rationale facts (`memory remember`) |
| `narrative` | `narrative-identity.yaml` | `SIMARD_THREAD_NARRATIVE_ENABLED` | Narrative-identity facts |
| `consolidation` | `consolidate-sleep.yaml` | `SIMARD_THREAD_CONSOLIDATION_ENABLED` | Sleep/dream consolidation facts |
| `prospection` | `prospect-foresight.yaml` | `SIMARD_THREAD_PROSPECTION_ENABLED` | Foresight facts + optional goal |
| `values_deliberation` | `values-deliberate.yaml` | `SIMARD_THREAD_VALUES_ENABLED` | Values-deliberation facts |
| `analogy` | `analogy-map.yaml` | `SIMARD_THREAD_ANALOGY_ENABLED` | Analogy-mapping facts |
| `operator_model` | `operator-model.yaml` | `SIMARD_THREAD_OPERATOR_MODEL_ENABLED` | Operator-model facts |

The `interoception` thread is **deterministic** — it does pure Rust sensing on a
cadence, has **no recipe** and **no parse path**, and still reports the same
`ran`/`health`/`consecutive_errors` outcome shape. It is included in the
"no `.unwrap_or` silent default" rule but excluded from "trigger a recipe."

Each `Thread::tick` reduces to:

```text
if !master_gate_open() || !thread_enabled() { return ThreadOutcome::skipped(); }
let ctx_files = assemble_read_only_context(&ctx);   // no writes here
match invoke_recipe(RECIPE_NAME, ctx_files) {
    Ran         => ThreadOutcome { ran: true,  health: Healthy },
    Failed(err) => ThreadOutcome { ran: false, health: Error(err) },  // LOUD, logged
}
```

No thread calls `ctx.memory.store_*`, `propose_goal`, or `write_signal`. No
thread reads `envelope.get(...)`. A recipe failure is recorded **loudly** (health
error + `consecutive_errors` incremented + stderr tail logged), never as "ran,
wrote nothing."

## Health and failure semantics — no silent defaults

- **Success** = the recipe subprocess exited `0`. `ran = true`, health advances
  toward `Healthy`, `consecutive_errors` resets to `0`.
- **Failure** = non-zero exit, spawn failure, or fence rejection. `ran = false`,
  health becomes `Error` with the captured stderr tail, `consecutive_errors`
  increments. This is surfaced in thread telemetry.
- There are **no `.unwrap_or(...)` fallbacks** anywhere on the effect path. A
  missing/failed effect is an error, not an empty write.

## What is preserved unchanged (zero-behavior-change when opted out)

The rework is behavior-preserving. These invariants are **unchanged**:

- **Default-ON opt-out gates (#4845).** The master gate
  `SIMARD_COGNITIVE_THREADS_ENABLED` and the per-thread
  `SIMARD_THREAD_<NAME>_ENABLED` are default-ON: a reflective thread runs unless
  a gate is set to an explicit falsy token (`0`/`false`/`no`/`off`).
- **Security fence.** `secret_scrub`, `sanitize_value`, `is_fenced_payload`,
  `build_context_args`, and `resolve_recipe_path` (SR-4) still guard every
  recipe invocation and every context value.
- **Background scheduler wiring.** The `Mind` still runs **after** the daemon's
  authoritative inline OODA cycle each iteration
  (`src/operator_commands_ooda/daemon/mod.rs`), behind the `AtomicBool` overlap
  guard with `ClearOnDrop` panic-safety and spawn-failure handling. It can never
  delay or starve OODA.
- **Fail-closed salience read.** The deterministic OODA "Decide" reorder in
  `src/ooda_loop/cycle.rs` still reads `state/salience_signal.json` via
  `salience_signal::advisory_priority_order` (which wraps `read_valid_signal`)
  **fail-closed**: an absent / oversized / malformed / stale signal yields an
  **empty ordering** — no reorder, OODA keeps its own goal ordering — while every
  surviving field is re-clamped and every off-board id dropped on the way in.
  (`read_valid_signal` itself returns `None` on those same conditions; the
  ordering wrapper turns that into an empty `Vec`.) This reader is **not touched**
  by the rework — see the
  [salience-signal CLI reference](../reference/simard-cognition-salience-signal-cli.md).

Opting the gates out (setting them to a falsy token) is a **zero-behavior
change**; a regression test asserts no reflective recipe fires when the gates are
off. Note this is now the *opt-out* path, not the default — a stock daemon runs
the roster (issue #4845).

## Verification (definition of done)

- `grep -rn 'extract_json_payload\|parse_step_output\|classify_recipe_stdout' src/cognitive_threads/`
  returns nothing.
- No `threads/*.rs` calls `ctx.memory.store_*` / `propose_goal` / `write_signal`
  from parsed recipe output, and none reads `envelope.get(...)`.
- Every reworked recipe calls `simard memory remember` /
  `remember-procedure` / `goal add` / `cognition salience-signal` and prints no
  JSON envelope / writes no output file.
- The double gate, the security fence, the scheduler wiring, and the fail-closed
  salience reader are unchanged; an opted-out deployment is zero-behavior-change.

## See also

- [Distillation semantic handoff](./distillation-semantic-handoff.md) — the
  exemplar this design copies.
- [`simard memory remember` CLI](../reference/simard-memory-remember-cli.md) —
  the fact/procedure write tools the recipes call.
- [`simard cognition salience-signal` CLI](../reference/simard-cognition-salience-signal-cli.md) —
  the clamping tool that writes the advisory salience signal.
- [Cognitive-thread scheduling](../reference/cognitive-thread-scheduling.md) —
  the `Mind` scheduler that hosts these threads.
- [Configure the reflective cognitive threads](../howto/configure-reflective-cognitive-threads.md) —
  how to enable them.
