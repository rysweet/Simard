---
title: Simard documentation
description: Start here for Simard runtime bootstrap, base-type selection, topology-neutral runtime composition, handoff, and reflection contracts.
last_updated: 2026-03-29
review_schedule: as-needed
owner: simard
---

# Simard documentation

Simard is a DI-first runtime shell for local CLI execution and in-process Rust runtime composition.

This docs set describes the runtime/base-type feature as a local contract in this repository. It covers the CLI bootstrap path, the topology-neutral runtime kernel, the base-type/session interfaces, truthful reflection, and runtime handoff export and restore. It does not define an HTTP API, a remote service contract, or a database schema.

## Start here

- [Tutorial: Run your first local session](./tutorials/run-your-first-local-session.md) - Walk the default CLI runtime flow end to end.
- [Tutorial: Run RustyClawd on the loopback mesh](./tutorials/run-rusty-clawd-on-loopback-mesh.md) - Learn the second topology path and the real `rusty-clawd` backend.
- [How to configure bootstrap and inspect reflection](./howto/configure-bootstrap-and-inspect-reflection.md) - Verify startup inputs and inspect truthful runtime metadata.
- [How to export and restore runtime handoff](./howto/export-and-restore-runtime-handoff.md) - Move the latest session boundary into fresh runtime ports while preserving carried-over memory and evidence records.
- [Runtime contracts reference](./reference/runtime-contracts.md) - Look up the public CLI and in-process Rust contracts.
- [Concept: topology-neutral runtime kernel](./concepts/topology-neutral-runtime-kernel.md) - Understand why runtime topology, transport, handoff, and supervision are injected instead of hardcoded.
- [Concept: truthful runtime metadata](./concepts/truthful-runtime-metadata.md) - Read the design rationale behind the reflection contract.

## Current guarantees

Today Simard provides:

- explicit bootstrap configuration, with `builtin-defaults` available only through opt-in startup mode
- explicit base-type and topology selection at bootstrap, with no silent fallback after startup
- builtin base-type registrations for `local-harness`, `rusty-clawd`, and `copilot-sdk`, with `rusty-clawd` reflected as a distinct backend and `copilot-sdk` reflected honestly as the current `local-harness` alias
- a topology-neutral runtime kernel built around `RuntimePorts`, `RuntimeRequest`, and `RuntimeKernel`
- the default CLI path through `cargo run --quiet`, which assembles the in-process runtime services and runs a single-process session locally
- a second injected topology path through `LoopbackMeshTopologyDriver` and `LoopbackMailboxTransport` for `multi-process` execution in the in-process Rust API
- a `distributed` topology value in the bootstrap/runtime contract, with explicit failure unless callers inject a compatible topology/base-type combination
- truthful reflection through `ReflectionSnapshot`, including manifest, runtime node, mailbox address, base-type backend, topology backend, transport backend, supervisor backend, memory backend, evidence backend, and handoff backend
- runtime handoff export/import through `RuntimeHandoffSnapshot` and `RuntimeKernel::compose_from_handoff(...)`, with restore currently validating identity and selected base type before rehydrating memory and evidence
- persisted scratch and summary memory plus live reflection summaries that record objective metadata instead of raw objective text
- [PLANNED] exported handoff session text should follow the same objective-metadata rule; current `export_handoff()` clones the latest session record, so snapshots remain fully sensitive runtime artifacts
- canonical session IDs shaped as `session-<uuid-v7>`, with validation at parsing boundaries
- explicit lifecycle errors for stopped runtimes, failed runtimes, invalid handoff snapshots, unsupported topologies, and invalid session IDs
- explicit rejection of `MemoryPolicy.allow_project_writes=true` in v1
- a local CLI/runtime contract, not a networked API surface

## Reading paths

If you are new to Simard, start with the [local session tutorial](./tutorials/run-your-first-local-session.md).

If you need the non-default runtime path, continue with [Tutorial: Run RustyClawd on the loopback mesh](./tutorials/run-rusty-clawd-on-loopback-mesh.md).

If you need a task-focused guide for moving work between runtime instances, use [How to export and restore runtime handoff](./howto/export-and-restore-runtime-handoff.md).

If you need exact field names, constructors, or error meanings, use the [runtime contracts reference](./reference/runtime-contracts.md).

If you are changing architecture or reviewing the topology/base-type split, read [Concept: topology-neutral runtime kernel](./concepts/topology-neutral-runtime-kernel.md) and [Concept: truthful runtime metadata](./concepts/truthful-runtime-metadata.md).
