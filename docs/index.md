---
title: Simard documentation
description: Start here for the current Simard runtime contracts, benchmark gym flow, bootstrap behavior, reflection metadata, and durable goal stewardship.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
---

# Simard documentation

Simard is a DI-first runtime shell for agent execution, reflection, durable goal stewardship, memory, evidence capture, and benchmark-gym validation.

This docs set describes the runtime behavior that exists in this repository today.

## Start here

- [Tutorial: Run your first local session](./tutorials/run-your-first-local-session.md) - Walk the local runtime flow end to end.
- [Tutorial: Run your first benchmark gym suite](./tutorials/run-your-first-benchmark-gym.md) - Run the shipped starter benchmark suite and inspect the emitted artifacts.
- [How to configure bootstrap and inspect reflection](./howto/configure-bootstrap-and-inspect-reflection.md) - Verify bootstrap inputs and inspect the truthful reflection surface.
- [How to carry meeting decisions into engineer sessions](./howto/carry-meeting-decisions-into-engineer-sessions.md) - Persist meeting records under a shared state root and confirm that later engineer-loop runs carry forward up to the three most recent ones.
- [Runtime contracts reference](./reference/runtime-contracts.md) - Look up the current public API and error contracts.
- [Concept: truthful runtime metadata](./concepts/truthful-runtime-metadata.md) - Read the design rationale behind the stricter contract.

## Current guarantees

Today Simard provides:

- explicit bootstrap configuration, with `builtin-defaults` available only through opt-in startup mode
- a durable local state root selected through `SIMARD_STATE_ROOT` in `explicit-config` or defaulted through `builtin-defaults`
- explicit base-type and topology selection at bootstrap, with opt-in defaults only in `builtin-defaults`
- builtin manifest-advertised base types selectable at startup today: `local-harness`, `terminal-shell`, `rusty-clawd`, and `copilot-sdk`, with `terminal-shell` wired as a real local PTY-backed shell session for `simard-engineer`, `rusty-clawd` wired as a distinct session backend, and `copilot-sdk` still aliased to the local harness implementation
- builtin identities selectable at startup today: `simard-engineer`, `simard-meeting`, `simard-goal-curator`, `simard-improvement-curator`, `simard-gym`, and the composite `simard-composite-engineer`
- a facilitator-backed `simard-meeting` identity that captures structured decisions, risks, next steps, open questions, and optional structured `goal:` updates without mutating code
- a dedicated `simard-goal-curator` identity that persists durable backlog entries and exposes the active top 5 goals through reflection
- a dedicated `simard-improvement-curator` identity that consumes persisted review artifacts, promotes explicitly approved proposals into durable active or proposed priorities, and keeps the loop operator-reviewable
- `single-process` for all builtin base types plus loopback `multi-process` execution for `rusty-clawd`, with `terminal-shell` intentionally limited to local single-process runs and unsupported pairs failing explicitly
- a local-first engineer loop probe that inspects repo state, prints a bounded action plan plus verification steps, runs either a truthful read-only repo scan or one explicit structured text edit on a clean repo, verifies the outcome, persists truthful local evidence/memory, reports the active goal set, and surfaces up to the three most recent carried meeting decisions separately without pretending remote orchestration already exists
- a starter benchmark gym suite that exercises all current builtin base-type selections plus a composite identity session through `cargo run --quiet --bin simard-gym -- run-suite starter`
- benchmark artifacts written under `target/simard-gym/`, including per-scenario JSON and text summaries, a dedicated review artifact, and a suite summary
- `ManifestContract { entrypoint, composition, precedence, provenance, freshness }`
- `ReflectionSnapshot { manifest_contract, runtime_node, mailbox_address, active_goal_count, active_goals, proposed_goal_count, proposed_goals, agent_program_backend, adapter_backend, adapter_capabilities, adapter_supported_topologies, topology_backend, transport_backend, supervisor_backend, memory_backend, evidence_backend, goal_backend }`
- truthful memory and evidence backend descriptors
- file-backed memory, evidence, and handoff stores on the bootstrap path, with persisted local state under the configured state root
- durable meeting decision records and active-goal updates persisted under the configured state root so later sessions can inspect concise planning outcomes
- a durable goal register persisted under the configured state root, with the active top 5 goals surfaced directly in runtime reflection and operator probes
- offline review artifacts and concise decision records persisted under the configured state root when the operator runs the review probe, plus a follow-on improvement-curation path that promotes approved proposals into durable priorities
- truthful runtime service metadata from the runtime-selected wiring, including the injected agent program, handoff store, the canonical backend identities behind each selected base type, and adapter capability/topology limits surfaced directly in reflection
- persisted scratch, summary, and reflection text that records objective metadata instead of raw objective text
- handoff snapshots that preserve runtime/session continuity while redacting the persisted session objective down to objective metadata
- canonical session IDs shaped as `session-<uuid-v7>`, with validation at parsing boundaries
- a real stopped runtime state whose snapshot remains inspectable after shutdown
- explicit `RuntimeStopped`, `InvalidSessionId`, and `InvalidManifestContract` errors
- explicit rejection of `MemoryPolicy.allow_project_writes=true` in v1
- a local CLI/runtime contract, not an HTTP API or database-backed service

## Contributor verification

Repository changes are expected to pass the same checks locally and in CI:

- `python3 -m pre_commit install --hook-type pre-commit --hook-type pre-push`
- `python3 -m pre_commit run --all-files --hook-stage pre-commit`
- `python3 -m pre_commit run --all-files --hook-stage pre-push`

Those hooks enforce `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, and `cargo test --all-features --locked`.

## Key runtime facts

- `src/main.rs` is the thin CLI wrapper; `bootstrap::run_local_session` owns the run loop and `simard::bootstrap::assemble_local_runtime` remains the reflected assembly boundary
- `src/bin/simard_gym.rs` is the operator-facing benchmark CLI for the starter gym suite
- `src/bin/simard_operator_probe.rs` now exposes `meeting-run`, `goal-curation-run`, `improvement-curation-run`, `terminal-run`, `engineer-loop-run`, `review-run`, and `review-read` so operators can inspect meeting capture, durable goal stewardship, review-backed improvement promotion, bounded shell execution, the local-first engineer loop, and evidence-linked review proposals through public surfaces
- `bootstrap::run_local_session` now persists durable memory/evidence records and the latest handoff snapshot under the configured state root
- defaults are startup choices, never silent runtime recovery
- reflection metadata is derived from the active runtime wiring, not placeholder labels
- post-stop `start()`, `run()`, and repeated `stop()` surface `SimardError::RuntimeStopped`

## Reading paths

If you are new to Simard, start with the [local session tutorial](./tutorials/run-your-first-local-session.md).

If you need to wire configuration or debug reflection output, jump to the [bootstrap and reflection how-to](./howto/configure-bootstrap-and-inspect-reflection.md).

If you need exact field names or error contracts, use the [runtime contracts reference](./reference/runtime-contracts.md).

If you are changing architecture, read the [truthful runtime metadata concept guide](./concepts/truthful-runtime-metadata.md) first.

- runtime handoff export/import through `RuntimeHandoffSnapshot` and `RuntimeKernel::compose_from_handoff(...)`, with restore currently validating identity and selected base type before rehydrating memory/evidence and preserving the redacted session boundary
- operator-level runtime validation through `cargo run --quiet --bin simard_operator_probe -- ...`
