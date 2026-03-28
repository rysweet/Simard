---
title: Simard documentation
description: Start here for the current Simard runtime contracts, bootstrap flow, and reflection metadata.
last_updated: 2026-03-28
review_schedule: as-needed
owner: simard
---

# Simard documentation

Simard is a DI-first runtime shell for agent execution, reflection, memory, and evidence capture.

This docs set describes the runtime behavior that exists in this repository today.

## Start here

- [Tutorial: Run your first local session](./tutorials/run-your-first-local-session.md) - Walk the local runtime flow end to end.
- [How to configure bootstrap and inspect reflection](./howto/configure-bootstrap-and-inspect-reflection.md) - Verify bootstrap inputs and inspect the truthful reflection surface.
- [Runtime contracts reference](./reference/runtime-contracts.md) - Look up the current public API and error contracts.
- [Concept: truthful runtime metadata](./concepts/truthful-runtime-metadata.md) - Read the design rationale behind the stricter contract.

## Current guarantees

Today Simard provides:

- explicit bootstrap configuration, with `builtin-defaults` available only through opt-in startup mode
- explicit base-type and topology selection at bootstrap, with opt-in defaults only in `builtin-defaults`
- builtin manifest-advertised base types selectable at startup today: `local-harness`, `rusty-clawd`, and `copilot-sdk`
- `single-process` as the only supported v1 topology for the builtin base types, with unsupported pairs failing explicitly
- `ManifestContract { entrypoint, composition, precedence, provenance, freshness }`
- `ReflectionSnapshot { manifest_contract, adapter_backend, memory_backend, evidence_backend }`
- truthful memory and evidence backend descriptors
- truthful adapter backend metadata from the runtime-selected base type
- canonical session IDs shaped as `session-<uuid-v7>`, with validation at parsing boundaries
- a real stopped runtime state whose snapshot remains inspectable after shutdown
- explicit `RuntimeStopped`, `InvalidSessionId`, and `InvalidManifestContract` errors
- a local CLI/runtime contract, not an HTTP API or database-backed service

## Contributor verification

Repository changes are expected to pass the same checks locally and in CI:

- `python3 -m pre_commit install --hook-type pre-commit --hook-type pre-push`
- `python3 -m pre_commit run --all-files --hook-stage pre-commit`
- `python3 -m pre_commit run --all-files --hook-stage pre-push`

Those hooks enforce `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, and `cargo test --all-features --locked`.

## Key runtime facts

- `src/main.rs` is the thin CLI wrapper; `bootstrap::run_local_session` owns the run loop and `simard::bootstrap::assemble_local_runtime` remains the reflected assembly boundary
- defaults are startup choices, never silent runtime recovery
- reflection metadata is derived from the active runtime wiring, not placeholder labels
- post-stop `start()`, `run()`, and repeated `stop()` surface `SimardError::RuntimeStopped`

## Reading paths

If you are new to Simard, start with the [local session tutorial](./tutorials/run-your-first-local-session.md).

If you need to wire configuration or debug reflection output, jump to the [bootstrap and reflection how-to](./howto/configure-bootstrap-and-inspect-reflection.md).

If you need exact field names or error contracts, use the [runtime contracts reference](./reference/runtime-contracts.md).

If you are changing architecture, read the [truthful runtime metadata concept guide](./concepts/truthful-runtime-metadata.md) first.
