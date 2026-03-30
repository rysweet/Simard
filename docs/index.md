---
title: Simard documentation
description: Start here for the current executable surfaces, the planned unified `simard` CLI, runtime contracts, and benchmark flow.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
---

# Simard documentation

Simard is in a transition state.

Today:

- `simard` is a thin bootstrap entrypoint configured through environment variables
- `simard_operator_probe` exposes the current engineer, meeting, goal-curation, improvement-curation, and review commands
- `simard-gym` exposes the current benchmark CLI

The product architecture targets a unified `simard` CLI with namespaces for `engineer`, `meeting`, `goal-curation`, `improvement-curation`, `gym`, `review`, and `bootstrap`. The docs below call out clearly whether something is current or planned so they do not overstate what ships today.

## Start here

- [Tutorial: Run your first local session](./tutorials/run-your-first-local-session.md) - Exercise the current local-session flows through today's binaries and see how they map to the planned unified CLI.
- [Tutorial: Run your first benchmark gym suite](./tutorials/run-your-first-benchmark-gym.md) - Run the shipped starter benchmark suite through `simard-gym` today and see the planned `simard gym` mapping.
- [How to configure bootstrap and inspect reflection](./howto/configure-bootstrap-and-inspect-reflection.md) - Verify the current bootstrap entrypoint, inspect the truthful runtime snapshot, and see the planned bootstrap subcommand.
- [How to carry meeting decisions into engineer sessions](./howto/carry-meeting-decisions-into-engineer-sessions.md) - Persist meeting records under a shared state root and confirm later engineer runs carry them forward.
- [Simard CLI reference](./reference/simard-cli.md) - Look up the planned unified command tree together with the current runnable command mappings.
- [Runtime contracts reference](./reference/runtime-contracts.md) - Look up the current executable contracts, the in-process runtime contract, and the planned unified CLI surface.
- [Concept: truthful runtime metadata](./concepts/truthful-runtime-metadata.md) - Read the design rationale behind the stricter runtime contract.

## Current executable surfaces

Simard currently guarantees these operator-visible entrypoints:

- `simard` boots a local session from environment variables and prints the reflected startup summary
- `simard_operator_probe` runs the current multi-mode compatibility commands:
  - `bootstrap-run`
  - `engineer-loop-run`
  - `terminal-run`
  - `meeting-run`
  - `goal-curation-run`
  - `improvement-curation-run`
  - `review-run`
  - `review-read`
- `simard-gym` runs the shipped benchmark commands:
  - `list`
  - `run <scenario-id>`
  - `run-suite <suite-id>`

These binaries already exercise real runtime behavior. They are not placeholders.

## Planned operator surface

The feature Simard is being built toward is a unified CLI shaped like this:

- `simard engineer ...`
- `simard meeting ...`
- `simard goal-curation ...`
- `simard improvement-curation ...`
- `simard gym ...`
- `simard review ...`
- `simard bootstrap ...`

Until `src/main.rs` dispatches that tree directly, the reference and tutorial docs keep both surfaces visible.

## Running from source

The examples in this docs set use the installed binary names when they refer to current executables:

- `simard`
- `simard_operator_probe`
- `simard-gym`

From the repository root, the corresponding Cargo commands are:

- `cargo run --quiet -- ...` for `simard`
- `cargo run --quiet --bin simard_operator_probe -- ...` for `simard_operator_probe`
- `cargo run --quiet --bin simard-gym -- ...` for `simard-gym`

## Contributor verification

Repository changes are expected to pass the same checks locally and in CI:

- `python3 -m pre_commit install --hook-type pre-commit --hook-type pre-push`
- `python3 -m pre_commit run --all-files --hook-stage pre-commit`
- `python3 -m pre_commit run --all-files --hook-stage pre-push`

Those hooks enforce `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, and `cargo test --all-features --locked`.

## Reading paths

If you are new to Simard, start with the [local session tutorial](./tutorials/run-your-first-local-session.md).

If you need exact current-to-planned command mappings, use the [Simard CLI reference](./reference/simard-cli.md).

If you need exact field names or lifecycle errors, use the [runtime contracts reference](./reference/runtime-contracts.md).

If you are changing architecture, read the [truthful runtime metadata concept guide](./concepts/truthful-runtime-metadata.md) first.
