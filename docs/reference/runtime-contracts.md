---
title: Runtime contracts reference
description: Reference for the shipped Simard executable surfaces, compatibility binaries, and the in-process runtime contracts that back them.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../index.md
  - ./simard-cli.md
  - ../howto/configure-bootstrap-and-inspect-reflection.md
  - ../tutorials/run-your-first-local-session.md
---

# Runtime contracts reference

This document covers:

- the canonical `simard` CLI surface
- the compatibility binaries that still expose a few legacy entrypoints
- the in-process Rust runtime and bootstrap types in `src/bootstrap.rs`, `src/runtime.rs`, and related modules

Simard does **not** expose:

- an HTTP API
- a network service contract
- a database schema contract

## Executable surfaces

| Runtime behavior | Canonical surface | Compatibility surface |
| --- | --- | --- |
| explicit bootstrap | `simard bootstrap run ...` | `simard_operator_probe bootstrap-run ...` |
| bounded engineer loop | `simard engineer run ...` | `simard_operator_probe engineer-loop-run ...` |
| terminal-backed engineer substrate | `simard engineer terminal ...` | `simard_operator_probe terminal-run ...` |
| meeting mode | `simard meeting run ...` | `simard_operator_probe meeting-run ...` |
| goal-curation mode | `simard goal-curation run ...` | `simard_operator_probe goal-curation-run ...` |
| improvement-curation mode | `simard improvement-curation run ...` | `simard_operator_probe improvement-curation-run ...` |
| review artifact persistence and readback | `simard review ...` | `simard_operator_probe review-run ...` and `review-read ...` |
| benchmark scenarios and suites | `simard gym ...` | `simard-gym ...` |

## Canonical CLI surface

The shipped operator-facing command tree is:

- `simard engineer run <topology> <workspace-root> <objective> [state-root]`
- `simard engineer terminal <topology> <objective> [state-root]`
- `simard meeting run <base-type> <topology> <structured-objective> [state-root]`
- `simard goal-curation run <base-type> <topology> <structured-objective> [state-root]`
- `simard improvement-curation run <base-type> <topology> <structured-objective> [state-root]`
- `simard gym list`
- `simard gym run <scenario-id>`
- `simard gym compare <scenario-id>`
- `simard gym run-suite <suite-id>`
- `simard review run <base-type> <topology> <objective> [state-root]`
- `simard review read <base-type> <topology> [state-root]`
- `simard bootstrap run <identity> <base-type> <topology> <objective> [state-root]`

Bare `simard` prints help for that tree instead of attempting a hidden bootstrap fallback.

## Shared state-root contract

Whenever a command accepts `[state-root]`, Simard validates it before any persistence work tied to durable operator state.

Rejected inputs include:

- any path containing `..`
- an existing path that is not a directory
- a symlink root

Safe roots are canonicalized once and then reused throughout the command.

## Mode contracts

### Engineer mode

Canonical entrypoint: `simard engineer run <topology> <workspace-root> <objective> [state-root]`

Compatibility surface: `simard_operator_probe engineer-loop-run <topology> <workspace-root> <objective> [state-root]`

The bounded engineer loop is intentionally narrow:

- it inspects the selected repo before acting
- it prints a short action plan and explicit verification steps
- it chooses one bounded local action
- it verifies the result explicitly
- it persists concise evidence and memory under the selected state root
- it surfaces active goals and up to the three most recent carried meeting records from the same state root

The bounded engineer loop supports two honest action shapes:

- a read-only repo-native scan such as `cargo-metadata-scan` or `git-tracked-file-scan`
- one explicit structured text replacement on a clean repo when the objective includes all of:
  - `edit-file: <repo-relative path>`
  - `replace: <existing text>`
  - `with: <replacement text>`
  - `verify-contains: <required post-edit text>`

That structured edit path is intentionally narrow:

- the target path must stay inside the selected repo
- the repo must start clean so Simard does not overwrite unrelated user changes
- only one expected changed file is allowed
- verification must confirm both file content and git-visible change state

### Terminal-backed engineer substrate

Canonical entrypoint: `simard engineer terminal <topology> <objective> [state-root]`

Compatibility surface: `simard_operator_probe terminal-run <topology> <objective> [state-root]`

This substrate exposes the real `terminal-shell` base type on the primary CLI:

- the selected base type remains `terminal-shell`
- reflection still reports `terminal-shell::local-pty` as the adapter implementation
- terminal evidence lines remain operator-visible
- unsupported topology and invalid state-root choices still fail explicitly

### Meeting mode

Canonical entrypoint: `simard meeting run <base-type> <topology> <structured-objective> [state-root]`

Compatibility surface: `simard_operator_probe meeting-run <base-type> <topology> <structured-objective> [state-root]`

Meeting mode persists concise durable planning data without touching repository contents. Structured lines may include:

- `agenda: ...`
- `update: ...`
- `decision: ...`
- `risk: ...`
- `next-step: ...`
- `open-question: ...`
- `goal: title | priority=1 | status=active | rationale=...`

### Goal-curation mode

Canonical entrypoint: `simard goal-curation run <base-type> <topology> <structured-objective> [state-root]`

Compatibility surface: `simard_operator_probe goal-curation-run <base-type> <topology> <structured-objective> [state-root]`

Goal-curation mode maintains durable backlog records and the active top five goals.

### Improvement-curation mode

Canonical entrypoint: `simard improvement-curation run <base-type> <topology> <structured-objective> [state-root]`

Compatibility surface: `simard_operator_probe improvement-curation-run <base-type> <topology> <structured-objective> [state-root]`

Improvement-curation mode promotes approved review proposals into durable priorities.

### Review mode

Canonical entrypoints:

- `simard review run <base-type> <topology> <objective> [state-root]`
- `simard review read <base-type> <topology> [state-root]`

Compatibility surface: `simard_operator_probe review-run ...` and `simard_operator_probe review-read ...`

Review mode persists a structured review artifact and makes the latest artifact readable from the same durable state root.

### Benchmark gym

Canonical entrypoints:

- `simard gym list`
- `simard gym run <scenario-id>`
- `simard gym run-suite <suite-id>`

Compatibility surface: `simard-gym ...`

The gym currently benchmarks scenarios built around:

- repo exploration and truthful local inspection
- docs refresh flow
- safe code change flow through the `rusty-clawd` identity
- composite session review

Artifacts are written under `target/simard-gym/` as JSON and text reports plus a `review.json` artifact for each scenario run.

The gym also supports persisted run-to-run comparison for a single scenario:

- `simard gym compare <scenario-id>` compares the latest two completed runs
- comparison results are classified as `improved`, `unchanged`, or `regressed`
- comparison artifacts are written under `target/simard-gym/comparisons/<scenario-id>/`

### Bootstrap contract

Canonical entrypoint: `simard bootstrap run <identity> <base-type> <topology> <objective> [state-root]`

Compatibility surface: `simard_operator_probe bootstrap-run <identity> <base-type> <topology> <objective> [state-root]`

The operator-facing bootstrap contract is now explicit:

- required values are passed positionally
- identity, base type, and topology mismatches fail explicitly
- there is no public zero-argument fallback path
- state-root validation runs before durable artifacts are read or written

## Durable carryover contract

Meeting, engineer, goal-curation, review, and improvement-curation commands can all share one explicit state root.

That shared state root is what allows:

- carried meeting decisions to appear in later engineer runs
- durable goals to stay visible across operator modes
- review artifacts to feed improvement-curation

The contract depends on passing the same validated state root across commands, not on hidden global state.

## Identity and backend contract

The builtin identities currently advertised by the loader are `simard-engineer`, `simard-meeting`, `simard-goal-curator`, `simard-improvement-curator`, `simard-gym`, and the composite `simard-composite-engineer`. All of them accept `local-harness`, `rusty-clawd`, and `copilot-sdk`; `simard-engineer` additionally accepts `terminal-shell` for the local terminal-backed path.

Reflection reports both the selected base type and the honest backend identity. For example:

- `copilot-sdk` currently resolves to the `local-harness` adapter implementation
- `terminal-shell` reports `terminal-shell::local-pty`
- `rusty-clawd` reports `rusty-clawd::session-backend`

## See also

- [Simard CLI reference](./simard-cli.md)
- [How to configure bootstrap and inspect reflection](../howto/configure-bootstrap-and-inspect-reflection.md)
- [Tutorial: Run your first local session](../tutorials/run-your-first-local-session.md)
