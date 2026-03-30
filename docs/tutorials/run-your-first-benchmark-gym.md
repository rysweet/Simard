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

You should see five scenarios:

- `repo-exploration-local`
- `docs-refresh-copilot`
- `safe-code-change-rusty-clawd`
- `composite-session-review`
- `interactive-terminal-driving`

Together they cover:

- the dedicated `simard-gym` identity
- the composite `simard-composite-engineer` identity
- the primary `simard-engineer` identity on the terminal-backed substrate
- `local-harness`
- `copilot-sdk`
- `rusty-clawd`
- `terminal-shell`
- both `single-process` and loopback `multi-process`

`interactive-terminal-driving` is intentionally a generic terminal-shell benchmark. It validates PTY-driven prompt/input sequencing without pretending to be a real `amplihack copilot` session.

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
- interactive-terminal-driving: passed (target/simard-gym/...)
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
- scorecard metrics including `correctness_checks_passed`, `correctness_checks_total`, `unnecessary_action_count`, and `retry_count`
- review proposals linked to persisted evidence
- correctness checks and whether they passed
- `measurement_notes` that describe the current metric scope without pretending fresh runs are unmeasured

Fresh runs now populate `scorecard.unnecessary_action_count` and `scorecard.retry_count` from benchmark-controlled attempt and action facts and stop emitting fresh review proposals, `scorecard.human_review_notes`, or `scorecard.measurement_notes` entries that say those metrics are "not measured". Older or incomplete artifacts should show `unmeasured` instead of fabricated zeroes.

## Step 4: Inspect one scenario report directly

You can also run a single scenario:

```bash
cargo run --quiet -- gym run interactive-terminal-driving
```

That command prints the scenario result and the persisted artifact paths directly on the operator-facing CLI.

You should see output shaped like:

```text
Scenario: interactive-terminal-driving
Suite: starter
Session: session-...
Passed: true
Checks passed: 8/8
Unnecessary actions: 0
Retry count: 0
Artifact report: target/simard-gym/interactive-terminal-driving/.../report.json
Artifact summary: target/simard-gym/interactive-terminal-driving/.../report.txt
Review artifact: target/simard-gym/interactive-terminal-driving/.../review.json
```

The detailed text artifact at `Artifact summary:` contains the full benchmark report, including identity, base type, topology, plan, execution summary, reflection summary, and the same metric values.

Open the JSON artifact and look for:

- `passed`
- `checks`
- `runtime`
- `handoff`
- `artifacts.review_json`
- `scorecard.unnecessary_action_count`
- `scorecard.retry_count`
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
Comparison summary: ...
Current session: ...
Current passed: true
Current checks passed: 8/8
Current report: target/simard-gym/safe-code-change-rusty-clawd/...
Current unnecessary actions: 0
Current retry count: 0
Previous session: ...
Previous passed: true
Previous checks passed: 8/8
Previous report: target/simard-gym/safe-code-change-rusty-clawd/...
Previous unnecessary actions: 0
Previous retry count: 0
Delta correctness checks passed: +0
Delta unnecessary actions: +0
Delta retry count: +0
Delta exported memory records: +0
Delta exported evidence records: +0
Comparison artifact report: target/simard-gym/comparisons/safe-code-change-rusty-clawd/...
Comparison artifact summary: target/simard-gym/comparisons/safe-code-change-rusty-clawd/...
```

That surface gives operators a lightweight regression check without manually diffing JSON.

If one of the two runs comes from an older `report.json` artifact that predates these fields, the metric lines stay readable and print `unmeasured` for the missing value and delta instead of inventing a zero.

## Step 6: Understand the current measurement boundary

The current benchmark foundation is real and intentionally scoped.

Today it verifies:

- bounded session completion
- runtime reflection and stopped-state behavior
- benchmark-scoped memory and evidence capture
- handoff export and restore continuity
- coverage across all shipped base-type selections and the composite identity

The current metric boundary extends that foundation with:

- truthful `unnecessary_action_count` scoring from benchmark-runner-observed benchmark-controlled action boundaries that fall outside the current scenario execution path
- truthful `retry_count` scoring from benchmark-runner-observed re-attempt counts inside one scenario run

Today it does **not** replace:

- a task-specific semantic judge for code correctness
- operator review of whether a completed task was actually the right task
- explicit inspection of execution summaries, reflection summaries, and persisted evidence

Those remaining boundaries are recorded in `measurement_notes` instead of being hidden. After the metric update lands, fresh runs should keep `measurement_notes` for the remaining scope boundaries only, not for these two metric fields.

## Summary

You now know how to:

- list the shipped benchmark scenarios through `simard gym`
- run the starter benchmark suite
- inspect the emitted benchmark artifacts
- see where `unnecessary_action_count` and `retry_count` appear in the shipped CLI and artifacts
- compare the latest two runs for a shipped scenario and understand the current metric additions
- use the compatibility `simard-gym` binary only when an older script still depends on it

## Next steps

- Use the [local session tutorial](./run-your-first-local-session.md) to compare ordinary runtime execution with benchmark execution.
- Use the [Simard CLI reference](../reference/simard-cli.md) when you need the exact command tree and compatibility mapping.
- Use the [runtime contracts reference](../reference/runtime-contracts.md) when you need the exact public contract details.
