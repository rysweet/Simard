---
title: Simard documentation
description: Start here for the shipped `simard` operator CLI, compatibility binaries, runtime contracts, and benchmark flow.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
---

# Simard documentation

`simard` is the canonical operator-facing CLI.

The shipped command tree covers `engineer`, `meeting`, `goal-curation`, `improvement-curation`, `gym`, `review`, and `bootstrap` from one binary. The legacy `simard_operator_probe` and `simard-gym` binaries remain available as compatibility surfaces while operators migrate, but the primary product surface is now `simard ...`.

## Start here

- [Tutorial: Run your first local session](./tutorials/run-your-first-local-session.md) - Exercise the local runtime through the primary CLI.
- [Tutorial: Run your first benchmark gym suite](./tutorials/run-your-first-benchmark-gym.md) - Run the shipped starter benchmark suite.
- [How to configure bootstrap and inspect reflection](./howto/configure-bootstrap-and-inspect-reflection.md) - Bootstrap an explicit runtime selection and inspect the truthful runtime snapshot.
- [How to carry meeting decisions into engineer sessions](./howto/carry-meeting-decisions-into-engineer-sessions.md) - Persist meeting records under a shared state root and confirm later engineer runs carry them forward.
- [Simard CLI reference](./reference/simard-cli.md) - Look up the shipped command tree and compatibility mappings.
- [Runtime contracts reference](./reference/runtime-contracts.md) - Look up executable contracts and lifecycle guarantees.
- [Concept: truthful runtime metadata](./concepts/truthful-runtime-metadata.md) - Read the design rationale behind the stricter runtime contract.

## Canonical executable surface

Simard guarantees these operator-visible namespaces on the primary binary:

- `simard engineer ...`
- `simard meeting ...`
- `simard goal-curation ...`
- `simard improvement-curation ...`
- `simard gym ...`
- `simard review ...`
- `simard bootstrap ...`

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

If you are changing architecture, read the [truthful runtime metadata concept guide](./concepts/truthful-runtime-metadata.md) first.
