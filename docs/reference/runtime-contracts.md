---
title: Runtime contracts reference
description: Reference for Simard bootstrap configuration, runtime/base-type APIs, topology support, handoff restore, and reflection/error contracts.
last_updated: 2026-03-29
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../index.md
  - ../howto/configure-bootstrap-and-inspect-reflection.md
  - ../howto/export-and-restore-runtime-handoff.md
  - ../tutorials/run-rusty-clawd-on-loopback-mesh.md
  - ../concepts/topology-neutral-runtime-kernel.md
  - ../concepts/truthful-runtime-metadata.md
---

# Runtime contracts reference

## Overview

Simard v1 exposes two supported surfaces in this repository:

- the local CLI bootstrap path through `cargo run --quiet`
- the in-process Rust runtime/bootstrap types re-exported from `src/lib.rs`

Simard v1 does **not** currently expose:

- an HTTP API
- a remote service contract
- a database schema contract

The stable contract in this repository is the local CLI/bootstrap behavior and the in-process Rust runtime/kernel API described below.

## Contents

- [CLI configuration](#cli-configuration)
- [Supported base types and topology combinations](#supported-base-types-and-topology-combinations)
- [Bootstrap and runtime APIs](#bootstrap-and-runtime-apis)
- [Lifecycle and state](#lifecycle-and-state)
- [Base-type session contract](#base-type-session-contract)
- [Handoff contract](#handoff-contract)
- [Reflection contract](#reflection-contract)
- [Errors](#errors)
- [Security and non-goals](#security-and-non-goals)

## CLI configuration

### Environment variables

| Variable | Required | Default | Description |
| --- | --- | --- | --- |
| `SIMARD_PROMPT_ROOT` | Yes in `explicit-config` | none | Root directory for prompt assets. |
| `SIMARD_OBJECTIVE` | Yes in `explicit-config` | none | Objective passed to `run()`. Live execution uses the real objective. Persisted scratch/summary memory and live reflection summaries use redacted `objective-metadata(...)`. Current handoff export still clones the latest `SessionRecord`, so `RuntimeHandoffSnapshot.session` may contain the raw objective until handoff redaction lands. |
| `SIMARD_BOOTSTRAP_MODE` | No | `explicit-config` | Startup mode. Accepted values: `explicit-config`, `builtin-defaults`. |
| `SIMARD_IDENTITY` | Yes in `explicit-config` | none in `explicit-config`; `simard-engineer` in `builtin-defaults` | Identity to load before runtime composition. Non-UTF-8 values fail bootstrap instead of being treated as missing. |
| `SIMARD_BASE_TYPE` | Yes in `explicit-config` | none in `explicit-config`; `local-harness` in `builtin-defaults` | Base type selected for the runtime request. Unsupported or unregistered selections fail explicitly. |
| `SIMARD_RUNTIME_TOPOLOGY` | Yes in `explicit-config` | none in `explicit-config`; `single-process` in `builtin-defaults` | Runtime topology selected for the runtime request. Accepted values: `single-process`, `multi-process`, `distributed`. Parsing a value does not guarantee the assembled runtime currently supports it end to end. |

### Bootstrap modes

| Mode | Behavior |
| --- | --- |
| `explicit-config` | Requires prompt root, objective, identity, base type, and topology from configuration. |
| `builtin-defaults` | Allows builtin prompt root, builtin objective, builtin identity, builtin base type (`local-harness`), and builtin topology (`single-process`), but only because startup opted in explicitly. |

### Example: explicit CLI run with the real `rusty-clawd` backend

```bash
SIMARD_PROMPT_ROOT="$PWD/prompt_assets" SIMARD_OBJECTIVE="exercise the rusty-clawd runtime path" SIMARD_IDENTITY="simard-engineer" SIMARD_BASE_TYPE="rusty-clawd" SIMARD_RUNTIME_TOPOLOGY="single-process" cargo run --quiet
```

Expected output shape:

```text
Simard local runtime executed successfully.
Bootstrap mode: explicit-config
Config sources: prompt_root=env:SIMARD_PROMPT_ROOT, objective=env:SIMARD_OBJECTIVE, base_type=env:SIMARD_BASE_TYPE, topology=env:SIMARD_RUNTIME_TOPOLOGY
Bootstrap selection: identity=simard-engineer, base_type=rusty-clawd, topology=single-process
Snapshot: state=ready, topology=single-process, base_type=rusty-clawd
Adapter implementation: rusty-clawd::session-backend
Shutdown: stopped
```

### Example: opt in to builtin defaults

```bash
SIMARD_BOOTSTRAP_MODE=builtin-defaults cargo run --quiet
```

Expected output shape:

```text
Bootstrap mode: builtin-defaults
Bootstrap selection: identity=simard-engineer, base_type=local-harness, topology=single-process
```

Defaults are startup choices, not runtime recovery behavior.

## Supported base types and topology combinations

### Builtin base-type registrations

| Base type selection | Reflected backend implementation | CLI topology support | In-process runtime support | Notes |
| --- | --- | --- | --- | --- |
| `local-harness` | `local-harness` | `single-process` | `single-process` | Default local scaffold. |
| `rusty-clawd` | `rusty-clawd::session-backend` | `single-process` | `single-process`, `multi-process` | Real backend with a second topology path in the runtime API. |
| `copilot-sdk` | `local-harness` | `single-process` | `single-process` | Explicit alias. Selection remains `copilot-sdk`; implementation remains `local-harness` until a distinct backend is registered. |

### Runtime topology drivers

| Runtime assembly path | Topology driver | Transport | Supervisor | Supported topologies |
| --- | --- | --- | --- | --- |
| `RuntimePorts::new(...)` | `topology::in-process` | `transport::in-memory-mailbox` | `supervisor::in-process` | `single-process` |
| `RuntimePorts::with_runtime_services(...)` | caller-injected | caller-injected | caller-injected | whatever the injected services advertise |
| `RuntimePorts::with_runtime_services_and_program(...)` | caller-injected | caller-injected | caller-injected | whatever the injected services advertise |

Repository-provided runtime services currently include:

- `InProcessTopologyDriver` for `single-process`
- `LoopbackMeshTopologyDriver` for `multi-process`, and for `distributed` requests when callers inject the rest of a compatible runtime
- `InMemoryMailboxTransport` for `inmemory://...` addresses
- `LoopbackMailboxTransport` for `loopback://...` addresses
- `InProcessSupervisor` and `CoordinatedSupervisor`

Important boundaries:

- the default CLI path always uses `RuntimePorts::new(...)`, so CLI runs remain `single-process` even though the in-process runtime API can inject alternate topology services
- repository-provided registered base types cover `single-process` and `multi-process` end to end today; `distributed` remains a contract/configuration value until callers inject a compatible base type and runtime service set

## Bootstrap and runtime APIs

### Primary bootstrap types

| Surface | Purpose |
| --- | --- |
| `BootstrapInputs::from_env()` | Reads the CLI bootstrap inputs from environment variables. |
| `BootstrapConfig::from_env()` | Resolves validated bootstrap config for the CLI path. |
| `assemble_local_runtime(&BootstrapConfig)` | Composes the local runtime for CLI execution. |
| `run_local_session(&BootstrapConfig)` | Starts, runs, snapshots, stops, and returns the local session execution bundle. |
| `bootstrap_entrypoint()` | Returns the reflected bootstrap assembly boundary: `simard::bootstrap::assemble_local_runtime`. |

### Runtime assembly types

| Surface | Purpose |
| --- | --- |
| `BaseTypeRegistry` | Registers `BaseTypeFactory` implementations by `BaseTypeId`. |
| `RuntimePorts` | Injects prompt assets, memory, evidence, base-type factories, topology driver, transport, supervisor, agent program, handoff store, and session ID strategy. |
| `RuntimeRequest::new(manifest, selected_base_type, topology)` | Describes the identity/base-type/topology request to compose. |
| `RuntimeKernel::compose(ports, request)` | Composes a fresh runtime from injected ports and a validated runtime request. |
| `RuntimeKernel::compose_from_handoff(ports, request, snapshot)` | Rehydrates a fresh runtime from a previously exported `RuntimeHandoffSnapshot`. |
| `LocalRuntime` | Type alias for `RuntimeKernel` used by the local runtime path. |

### `RuntimePorts` constructors

| Constructor | Behavior |
| --- | --- |
| `RuntimePorts::new(...)` | Injects `InProcessTopologyDriver`, `InMemoryMailboxTransport`, `InProcessSupervisor`, `ObjectiveRelayProgram`, and `InMemoryHandoffStore`. |
| `RuntimePorts::with_session_ids(...)` | Same runtime-service defaults as `new(...)`, but makes session-ID injection explicit at the call site. |
| `RuntimePorts::with_runtime_services(...)` | Lets callers inject topology, transport, and supervisor while keeping the default objective-relay agent program and in-memory handoff store. |
| `RuntimePorts::with_runtime_services_and_program(...)` | Lets callers inject all runtime services, including the agent program and handoff store. Use this for alternate topologies, migration tests, and custom orchestration. |

### Example: compose the alternate topology path in-process

```rust
use std::sync::Arc;

use simard::{
    BaseTypeId, BaseTypeRegistry, IdentityManifest, InMemoryEvidenceStore, InMemoryHandoffStore,
    InMemoryMemoryStore, InMemoryPromptAssetStore, InProcessSupervisor,
    LoopbackMailboxTransport, LoopbackMeshTopologyDriver, LocalRuntime, ManifestContract,
    MemoryPolicy, OperatingMode, PromptAsset, PromptAssetRef, Provenance, RuntimePorts,
    RuntimeRequest, RuntimeTopology, RustyClawdAdapter, UuidSessionIdGenerator,
    capability_set, BaseTypeCapability, Freshness,
};

let prompts = Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
    "engineer-system",
    "simard/engineer_system.md",
    "You are Simard.",
)]));
let memory = Arc::new(InMemoryMemoryStore::try_default()?);
let evidence = Arc::new(InMemoryEvidenceStore::try_default()?);
let handoff = Arc::new(InMemoryHandoffStore::try_default()?);
let mut base_types = BaseTypeRegistry::default();
base_types.register(RustyClawdAdapter::registered("rusty-clawd")?);

let request = RuntimeRequest::new(
    IdentityManifest::new(
        "simard-engineer",
        env!("CARGO_PKG_VERSION"),
        vec![PromptAssetRef::new("engineer-system", "simard/engineer_system.md")],
        vec![BaseTypeId::new("rusty-clawd")],
        capability_set([
            BaseTypeCapability::PromptAssets,
            BaseTypeCapability::SessionLifecycle,
            BaseTypeCapability::Memory,
            BaseTypeCapability::Evidence,
            BaseTypeCapability::Reflection,
        ]),
        OperatingMode::Engineer,
        MemoryPolicy::default(),
        ManifestContract::new(
            simard::bootstrap_entrypoint(),
            "bootstrap-config -> identity-loader -> runtime-ports -> local-runtime",
            vec!["docs:runtime-contracts".to_string()],
            Provenance::new("docs", "reference::runtime-contracts"),
            Freshness::now()?,
        )?,
    )?,
    BaseTypeId::new("rusty-clawd"),
    RuntimeTopology::MultiProcess,
);

let runtime = LocalRuntime::compose(
    RuntimePorts::with_runtime_services_and_program(
        prompts,
        memory,
        evidence,
        base_types,
        Arc::new(LoopbackMeshTopologyDriver::try_default()?),
        Arc::new(LoopbackMailboxTransport::try_default()?),
        Arc::new(InProcessSupervisor::try_default()?),
        Arc::new(simard::ObjectiveRelayProgram::try_default()?),
        handoff,
        Arc::new(UuidSessionIdGenerator),
    ),
    request,
)?;
```

## Lifecycle and state

### `RuntimeKernel` lifecycle methods

| Method | Meaning |
| --- | --- |
| `start()` | Loads prompt assets and transitions the runtime into `Ready`. |
| `run(objective)` | Executes a session for the selected base type. On success the runtime returns to `Ready`. On failure the runtime becomes `Failed`. |
| `snapshot()` | Returns the current `ReflectionSnapshot`. Snapshot inspection remains valid after failure and after stop. |
| `export_handoff()` | Exports the latest session boundary as currently stored, plus memory records and evidence records, into a `RuntimeHandoffSnapshot` and persists it through the injected handoff store. |
| `stop()` | Transitions the runtime into `Stopped`. After stop, `start()`, `run()`, and repeated `stop()` calls fail explicitly. |

### `RuntimeState`

| State | Meaning |
| --- | --- |
| `Initializing` | Runtime has been composed but not started. |
| `Ready` | Prompt assets are loaded and the runtime can execute. |
| `Active` | Base-type session work is in progress. |
| `Reflecting` | Reflection metadata is being assembled. |
| `Persisting` | Memory and evidence summaries are being persisted. |
| `Failed` | Execution failed before completion. |
| `Stopping` | Shutdown is in progress. |
| `Stopped` | Runtime has been shut down and cannot accept more work. |

### Stopped and failed behavior

After `stop()` succeeds:

- `snapshot()` remains valid
- `run()` fails with `RuntimeStopped { action: "run" }`
- `start()` fails with `RuntimeStopped { action: "start" }`
- repeated `stop()` fails with `RuntimeStopped { action: "stop" }`

After `run()` fails:

- `snapshot()` remains valid
- the last session remains visible with `session_phase == Some(Failed)`
- manifest freshness is reported as `Stale`
- `start()` fails with `RuntimeFailed`
- `run()` fails with `RuntimeFailed`
- `stop()` is still the explicit way to close the lifecycle boundary

## Base-type session contract

### Core types

| Type | Meaning |
| --- | --- |
| `BaseTypeDescriptor` | Describes the selected base type, the reflected backend descriptor, required capabilities, and supported topologies. |
| `BaseTypeFactory` | Opens base-type sessions for a given runtime request. |
| `BaseTypeSessionRequest` | Carries the session ID, operating mode, topology, prompt assets, runtime node, and mailbox address into the base-type session. |
| `BaseTypeTurnInput` | Carries the objective for the next turn. |
| `BaseTypeOutcome` | Returns `plan`, `execution_summary`, and execution-time evidence lines. |

### Contract rules

- identity selection and backend implementation are distinct facts
- `selected_base_type` preserves what the caller asked for
- `adapter_backend.identity` reports the implementation that actually ran
- topology support is checked twice: once against the injected runtime topology driver, then against the selected base-type descriptor
- unsupported combinations fail explicitly; Simard does not silently change the base type or topology for you

## Handoff contract

### `RuntimeHandoffSnapshot`

| Field | Meaning |
| --- | --- |
| `exported_state` | Runtime state at export time. |
| `identity_name` | Identity name that must match the restore request. |
| `selected_base_type` | Base type that must match the restore request. |
| `topology` | Topology of the source runtime. |
| `source_runtime_node` | Runtime node reported by the source topology driver. |
| `source_mailbox_address` | Mailbox address reported by the source transport. |
| `session` | Latest session boundary. Current export clones the stored `SessionRecord`, so `session.objective` remains verbatim until handoff redaction is implemented. |
| `memory_records` | Session-scoped memory records rehydrated into the destination runtime. |
| `evidence_records` | Session-scoped evidence records rehydrated into the destination runtime. |

### Restore behavior

`RuntimeKernel::compose_from_handoff(...)` enforces these rules:

- `snapshot.identity_name` must match `request.manifest.name`
- `snapshot.selected_base_type` must match `request.selected_base_type`
- `snapshot.topology` is preserved in the snapshot, but current restore does not reject a request solely because the topology differs
- memory and evidence records are copied into the destination stores before the runtime starts
- the last session boundary is restored so `snapshot()` can report the carried-over session phase immediately
- the restored runtime remains in `Initializing` until `start()` is called

### Example: export and restore

```rust
let snapshot = source_runtime.export_handoff()?;
let restored = simard::LocalRuntime::compose_from_handoff(restored_ports, request, snapshot)?;

assert_eq!(restored.snapshot()?.runtime_state, simard::RuntimeState::Initializing);
assert_eq!(restored.snapshot()?.session_phase, Some(simard::SessionPhase::Complete));
```

Treat exported handoff payloads as sensitive. They contain session identifiers, memory summaries, evidence records, runtime-node data, mailbox addresses, and today the stored session objective as well.

## Reflection contract

### `ReflectionSnapshot`

```rust
pub struct ReflectionSnapshot {
    pub identity_name: String,
    pub selected_base_type: BaseTypeId,
    pub topology: RuntimeTopology,
    pub runtime_state: RuntimeState,
    pub runtime_node: RuntimeNodeId,
    pub mailbox_address: RuntimeAddress,
    pub session_phase: Option<SessionPhase>,
    pub prompt_assets: Vec<PromptAssetId>,
    pub manifest_contract: ManifestContract,
    pub evidence_records: usize,
    pub memory_records: usize,
    pub agent_program_backend: BackendDescriptor,
    pub handoff_backend: BackendDescriptor,
    pub adapter_backend: BackendDescriptor,
    pub topology_backend: BackendDescriptor,
    pub transport_backend: BackendDescriptor,
    pub supervisor_backend: BackendDescriptor,
    pub memory_backend: BackendDescriptor,
    pub evidence_backend: BackendDescriptor,
}
```

Reflection rules:

- `manifest_contract` carries entrypoint, provenance, precedence, composition, and freshness together
- `runtime_node` and `mailbox_address` come from the injected topology and transport services
- `agent_program_backend` comes from the injected agent-program contract, not from hardcoded runtime logic
- `handoff_backend` comes from the injected handoff store used for export/import
- `adapter_backend` comes from the selected base-type factory/session descriptor, not from a bootstrap shortcut
- `topology_backend`, `transport_backend`, and `supervisor_backend` come from the live runtime services
- `memory_backend` and `evidence_backend` come from the live stores
- stopped or failed snapshots mark manifest freshness as `Stale`

## Errors

### Configuration and identity errors

| Error | Meaning |
| --- | --- |
| `MissingRequiredConfig` | Required startup configuration is absent. |
| `NonUnicodeConfigValue` | A configuration value exists but cannot be decoded as UTF-8. |
| `InvalidConfigValue` | A configuration value is present but invalid. |
| `UnknownIdentity` | Requested identity is not registered. |
| `InvalidManifestContract` | Manifest contract metadata is incomplete or untruthful. |
| `ClockBeforeUnixEpoch` | Runtime metadata could not record a truthful observation time. |
| `InvalidSessionId` | A supplied session ID is not a valid UUID-based Simard session ID. |
| `UnsupportedBaseType` | The chosen identity does not allow the requested base type. |
| `AdapterNotRegistered` | No base-type factory has been registered for the requested base type. |
| `MissingCapability` | The selected base-type backend exists but does not satisfy the manifest's required capabilities. |
| `UnsupportedRuntimeTopology` | The injected topology driver does not support the requested topology. |
| `UnsupportedTopology` | The selected base-type backend does not support the requested topology. |
| `UnsupportedMemoryPolicy` | The manifest asked for a memory policy Simard v1 does not allow, including `allow_project_writes=true`. |

### Lifecycle, handoff, and session errors

| Error | Meaning |
| --- | --- |
| `InvalidRuntimeTransition` | A runtime lifecycle transition is invalid. |
| `InvalidBaseTypeSessionState` | A base-type session was opened, used, or closed out of order. |
| `AdapterInvocationFailed` | The selected base-type backend failed while executing a turn. |
| `InvalidHandoffSnapshot` | A handoff snapshot could not be restored into the requested runtime. |
| `RuntimeStopped` | Caller attempted `start`, `run`, or `stop` after shutdown was already in effect. |
| `RuntimeFailed` | Caller attempted `start` or `run` after a failed execution but before shutdown. |
| `InvalidSessionTransition` | A session phase transition is invalid. |
| `StoragePoisoned` | An in-memory store lock was poisoned. |

### Prompt and asset errors

| Error | Meaning |
| --- | --- |
| `PromptAssetMissing` | A referenced prompt asset was not found. |
| `PromptAssetRead` | A prompt asset could not be read. |
| `InvalidPromptAssetPath` | A prompt asset path was rejected because it escaped the allowed prompt root contract. |

## Security and non-goals

- Simard documents a local CLI and in-process Rust contract only. There is no HTTP, auth-token, TLS, or database contract in this feature.
- Exported handoff payloads are sensitive. [PLANNED] objective redaction for exported session text has not landed yet, so store and transfer them as full runtime artifacts.
- `MemoryPolicy.allow_project_writes=true` is rejected explicitly in v1.
- Unsupported identities, base types, and topologies fail explicitly. Simard does not downgrade or substitute behind the caller's back.

## See also

- [How to configure bootstrap and inspect reflection](../howto/configure-bootstrap-and-inspect-reflection.md)
- [How to export and restore runtime handoff](../howto/export-and-restore-runtime-handoff.md)
- [Tutorial: Run RustyClawd on the loopback mesh](../tutorials/run-rusty-clawd-on-loopback-mesh.md)
- [Concept: topology-neutral runtime kernel](../concepts/topology-neutral-runtime-kernel.md)
- [Concept: truthful runtime metadata](../concepts/truthful-runtime-metadata.md)
- [Documentation index](../index.md)
