---
title: "Tutorial: Run your first benchmark gym suite"
description: Exercise the shipped Simard benchmark scenarios through the canonical `simard gym` surface and understand where the legacy `simard-gym` binary still fits.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: tutorial
related:
  - ../index.md
  - ../reference/simard-cli.md
  - ../reference/runtime-contracts.md
  - ./run-your-first-local-session.md
---

# Tutorial: Run your first benchmark gym suite

This tutorial exercises the benchmark surface through the canonical `simard gym` namespace.

## What you'll learn

- how to list the shipped benchmark scenarios
- how to run the starter suite across the current builtin base-type selections
- where Simard writes benchmark artifacts
- when you might still reach for the compatibility `simard-gym` binary

## Prerequisites

- Rust and Cargo installed
- a shell in the repository root

All runnable examples below use the canonical benchmark surface.

## Step 1: List the shipped benchmark scenarios

The starter suite is intentionally small and curated.

```bash
cargo run --quiet -- gym list
```

You should see four scenarios:

- `repo-exploration-local`
- `docs-refresh-copilot`
- `safe-code-change-rusty-clawd`
- `composite-session-review`

Together they cover:

- the dedicated `simard-gym` identity
- the composite `simard-composite-engineer` identity
- `local-harness`
- `copilot-sdk`
- `rusty-clawd`
- both `single-process` and loopback `multi-process`

If you need exact legacy output for an older script, `cargo run --quiet --bin simard-gym -- list` still works as a compatibility surface.

## Step 2: Run the starter benchmark suite

Run the full shipped suite like an operator would:

```bash
cargo run --quiet -- gym run-suite starter
```

You should see output shaped like:

```text
Suite: starter
Suite passed: true
- repo-exploration-local: passed (target/simard-gym/...)
- docs-refresh-copilot: passed (target/simard-gym/...)
- safe-code-change-rusty-clawd: passed (target/simard-gym/...)
- composite-session-review: passed (target/simard-gym/...)
Suite artifact report: target/simard-gym/suites/starter.json
```

## Step 3: Inspect the generated artifacts

The gym writes artifacts under `target/simard-gym/`.

Per scenario, Simard currently emits:

- `report.json`
- `report.txt`
- `review.json`

The suite run also writes:

- `target/simard-gym/suites/starter.json`

Those artifacts record:

- scenario metadata
- the selected identity, base type, and topology
- runtime and handoff summaries
- review proposals linked to persisted evidence
- correctness checks and whether they passed
- measurement notes that explain what the current v1 gym does not yet infer automatically

## Step 4: Inspect one scenario report directly

You can also run a single scenario:

```bash
cargo run --quiet -- gym run safe-code-change-rusty-clawd
```

That command prints the scenario result plus the exact artifact paths for the scenario run.

Open the JSON artifact and look for:

- `passed`
- `checks`
- `runtime`
- `handoff`
- `artifacts.review_json`
- `scorecard.human_review_notes`
- `scorecard.measurement_notes`

## Step 5: Compare the latest two runs for one scenario

Once a scenario has been executed at least twice, you can ask Simard to compare the latest two completed runs directly:

```bash
cargo run --quiet -- gym compare safe-code-change-rusty-clawd
```

You should see output shaped like:

```text
Scenario: safe-code-change-rusty-clawd
Comparison status: unchanged
Current report: target/simard-gym/safe-code-change-rusty-clawd/...
Previous report: target/simard-gym/safe-code-change-rusty-clawd/...
Comparison artifact report: target/simard-gym/comparisons/safe-code-change-rusty-clawd/...
```

That surface gives operators a lightweight regression check without manually diffing JSON.

## Step 6: Understand the current measurement boundary

The current benchmark foundation is real, but intentionally modest.

Today it verifies:

- bounded session completion
- runtime reflection and stopped-state behavior
- benchmark-scoped memory and evidence capture
- handoff export and restore continuity
- coverage across all shipped base-type selections and the composite identity

Today it does **not** yet infer:

- a task-specific semantic judge for code correctness
- automatic unnecessary action counting
- autonomous retry-and-replan loops inside the gym runner

Those gaps are recorded in the emitted `measurement_notes` instead of being hidden.

## Summary

You now know how to:

- list the shipped benchmark scenarios through `simard gym`
- run the starter benchmark suite
- inspect the emitted benchmark artifacts
- compare the latest two runs for a shipped scenario
- use the compatibility `simard-gym` binary only when an older script still depends on it

## Next steps

- Use the [local session tutorial](./run-your-first-local-session.md) to compare ordinary runtime execution with benchmark execution.
- Use the [Simard CLI reference](../reference/simard-cli.md) when you need the exact command tree and compatibility mapping.
- Use the [runtime contracts reference](../reference/runtime-contracts.md) when you need the exact public contract details.
