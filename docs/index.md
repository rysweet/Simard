---
title: Simard documentation
description: Start here for the shipped `simard` operator CLI, the shared-state-root bridge from bounded terminal sessions into the repo-grounded engineer loop, the `engineer read` audit companion, compatibility binaries, runtime contracts, and benchmark flow.
last_updated: 2026-04-03
review_schedule: as-needed
owner: simard
---

# Simard documentation

`simard` is the canonical operator-facing CLI.

The shipped command tree covers `engineer`, `meeting`, `goal-curation`, `improvement-curation`, `gym`, `review`, and `bootstrap` from one binary, including the read-only `engineer read` audit companion and the bounded `engineer terminal*` session surfaces. The legacy `simard_operator_probe` and `simard-gym` binaries remain available as compatibility surfaces while operators migrate, but the primary product surface is now `simard ...`.

Terminal sessions and repo-grounded engineer runs now bridge through one explicit local `state-root`. That bridge is file-backed and operator-visible. It does not imply hidden resume logic, external orchestration, or automatic continuation.

## Start here

- [Tutorial: Run your first local session](./tutorials/run-your-first-local-session.md) - Exercise the local runtime through the primary CLI.
- [How to move from terminal recipes into engineer runs](./howto/move-from-terminal-recipes-into-engineer-runs.md) - Start with a discoverable terminal recipe, then continue into the repo-grounded engineer loop through the same explicit state root.
- [Tutorial: Run your first benchmark gym suite](./tutorials/run-your-first-benchmark-gym.md) - Run the shipped starter benchmark suite.
- [How to configure bootstrap and inspect reflection](./howto/configure-bootstrap-and-inspect-reflection.md) - Bootstrap an explicit runtime selection and inspect the truthful runtime snapshot.
- [How to reclaim disk space and run low-space Rust builds](./howto/reclaim-disk-space-and-run-low-space-rust-builds.md) - Reclaim stale build artifacts and run Cargo through one shared low-space target dir across worktrees.
- [How to carry meeting decisions into engineer sessions](./howto/carry-meeting-decisions-into-engineer-sessions.md) - Persist meeting records under a shared state root and confirm later engineer runs carry them forward.
- [How to inspect meeting records](./howto/inspect-meeting-records.md) - Read back the latest durable meeting record without mutating stored state.
- [How to inspect improvement-curation state](./howto/inspect-improvement-curation-state.md) - Read back the latest approved, deferred, and promoted improvement state without mutation.
- [How to inspect the durable goal register](./howto/inspect-durable-goal-register.md) - Read back the active top-5 goals and backlog without mutation.
- [How to run the OODA daemon](./howto/run-ooda-daemon.md) - Start the continuous OODA loop for autonomous goal-driven operation and act on meeting decisions.
- [Simard CLI reference](./reference/simard-cli.md) - Look up the shipped command tree, `engineer read` audit surface, and compatibility mappings.
- [Runtime contracts reference](./reference/runtime-contracts.md) - Look up executable contracts, state-root guarantees, and the shipped engineer audit readback semantics.
- [Base type adapters reference](./reference/base-type-adapters.md) - Look up the pluggable agent execution substrates, their capabilities, and topology support.
- [Bridge wire protocol reference](./reference/bridge-wire-protocol.md) - Look up the JSON-line protocol for Rust-Python bridge communication.
- [Concept: truthful runtime metadata](./concepts/truthful-runtime-metadata.md) - Read the design rationale behind the stricter runtime contract.
- [Concept: same-path copy guard](./concepts/same-path-copy-guard.md) - Read the design rationale for the canonicalize()-based guard that prevents self-copy crashes.

## Canonical executable surface

Simard guarantees these operator-visible namespaces on the primary binary:

- `simard engineer ...`
- `simard meeting ...`
- `simard goal-curation ...`
- `simard improvement-curation ...`
- `simard gym ...`
- `simard review ...`
- `simard bootstrap ...`
- `simard ooda ...`
- `simard act-on-decisions`
- `simard spawn ...`
- `simard handover ...`

Bare `simard` prints the unified help text instead of attempting a hidden environment-only bootstrap fallback.

## Compatibility binaries

The compatibility binaries remain shipped, but they are no longer the canonical entrypoint:

- `simard_operator_probe` preserves the legacy multi-mode probe commands
- `simard-gym` preserves the legacy benchmark binary

Use them only when you need compatibility with older scripts or exact legacy output.

## Running from source

From the repository root, the corresponding Cargo commands are:

- `cargo run --quiet -- ...` for `simard`
- `cargo run --quiet --bin simard_operator_probe -- ...` for `simard_operator_probe`
- `cargo run --quiet --bin simard-gym -- ...` for `simard-gym`

If you are tight on disk or working across many Simard worktrees, prefer `scripts/cargo-low-space ...` for local builds and use `scripts/reclaim-build-space` to preview or delete stale build artifact directories.

## Contributor verification

Repository changes are expected to pass the same checks locally and in CI:

- `python3 -m pre_commit install --hook-type pre-commit --hook-type pre-push`
- `python3 -m pre_commit run --all-files --hook-stage pre-commit`
- `python3 -m pre_commit run --all-files --hook-stage pre-push`

Those hooks enforce `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, and `cargo test --all-features --locked`.

## Reading paths

If you are new to Simard, start with the [local session tutorial](./tutorials/run-your-first-local-session.md).

If you need exact commands, use the [Simard CLI reference](./reference/simard-cli.md).

If you need exact field names or lifecycle errors, use the [runtime contracts reference](./reference/runtime-contracts.md).

If you are changing architecture, start with the [architecture overview](./architecture/overview.md), then read the [truthful runtime metadata concept guide](./concepts/truthful-runtime-metadata.md).

## Architecture

- [Architecture overview](./architecture/overview.md) - System diagram, core principles, component descriptions, and module map.
- [Agent composition](./architecture/agent-composition.md) - How Simard composes subordinate agents with goal assignment, supervision, and crash recovery.
- [Bridge pattern](./architecture/bridge-pattern.md) - Rust-Python subprocess bridges with circuit breaker fault tolerance.
- [Cognitive memory](./architecture/cognitive-memory.md) - Six-type memory model, session lifecycle mapping, and hive mind integration.
- [Implementation plan](./architecture/implementation-plan.md) - Phased roadmap with current status and quality gates.
- [OODA meeting handoff integration](./architecture/ooda-meeting-handoff-integration.md) - Wire meeting handoffs into the OODA daemon and seed default goals (Issues #157, #158).
