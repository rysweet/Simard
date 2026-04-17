# Workflows

Simard's top-level workflows — the major orchestration loops that do work.

## OODA daemon

The autonomous background loop. Observes issues, gym scores, handoff files, memory; orients by ranking priorities; decides on actions; acts by launching sessions; reviews and curates the result. Run with `simard ooda run`.

Replaces the role amplihack plays through its **smart-orchestrator** recipe plus the recipe runner, though the DSL is different: Simard encodes the loop in Rust rather than YAML.

## Engineer loop

One-shot or multi-turn work on a concrete objective. `simard engineer run <topology> <workspace-root> <objective>`. Inspect → select → execute → verify. Persists a session record to disk.

Closest amplihack analog: a recipe run whose top-level step is the default development workflow. Engineer loop does not parse YAML recipes; it is a direct Rust implementation of the loop.

## Meeting REPL

Interactive meeting facilitator. `simard meeting repl <topic>`. Produces decisions, action items, and handoff files.

No direct amplihack analog — this is a new capability.

## Goal curation

Creates, prioritizes, and closes goals backed by the durable goal register. `simard goal-curation run ...`. See [howto/inspect-durable-goal-register.md](howto/inspect-durable-goal-register.md).

## Improvement curation

Proposes, reviews, and sequences improvements to Simard or the repos it stewards. `simard improvement-curation run ...`. See [howto/inspect-improvement-curation-state.md](howto/inspect-improvement-curation-state.md).

## Self-improve cycle

Evaluate against gym → analyze weaknesses → propose improvements → re-evaluate. Executed inside the OODA daemon or via `simard improvement-curation`.

## Gym evaluation

`simard gym list`, `simard gym run <scenario>`, `simard gym compare ...`. Evaluates Simard's behavior against bounded benchmark scenarios.

**Runtime dependency:** today `python/simard_gym_bridge.py` imports `amplihack.eval.progressive_test_suite` and `amplihack.eval.long_horizon_memory`. Native Rust gym eval is a tracked parity issue. See [amplihack-comparison.md](amplihack-comparison.md#evaluation).

## Review pipeline

`simard review run ...` — multi-perspective review of a candidate change against Simard's quality standards.

## Bootstrap

`simard bootstrap run <identity> <base-type> <topology> <objective>` — warms a session with the right identity, base type, and topology and hands off control.

## Comparison with amplihack recipe-runner

| Capability | amplihack | Simard |
|---|---|---|
| Multi-step workflow DSL | YAML recipes under `amplifier-bundle/recipes/` | Rust orchestration code (no YAML) |
| Nested sessions | `run_recipe_by_name` spawns Claude Code / Copilot subprocesses | Engineer loop spawns base-type adapters |
| Step-level guard rails | Recipe-runner enforces | Engineer loop + OODA daemon enforce |
| Progress streaming | `progress=True` on recipe | Session stdout + dashboard |
| Resumption after crash | Recipe-runner persists per-step state | Session record replay |

See [recipes.md](recipes.md) for what Simard currently does that overlaps with amplihack recipes.
