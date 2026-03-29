---
title: Runtime contracts reference
description: Reference for the current Simard runtime surface, reflection contracts, and lifecycle errors.
last_updated: 2026-03-28
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../index.md
  - ../howto/configure-bootstrap-and-inspect-reflection.md
  - ../concepts/truthful-runtime-metadata.md
---

# Runtime contracts reference

## Status

This file describes the API shape that exists in the repository today.

## Public surfaces

Simard v1 currently exposes four surfaces:

- the local CLI bootstrap path through `cargo run --quiet`
- the benchmark gym CLI through `cargo run --quiet --bin simard-gym -- ...`
- the operator/runtime probe through `cargo run --quiet --bin simard_operator_probe -- ...`
- the in-process Rust runtime/bootstrap types in `src/bootstrap.rs`, `src/runtime.rs`, and related modules

Simard v1 does **not** currently expose:

- an HTTP API
- a network service contract
- a database schema contract

The stable contract in this repository is the bootstrap/runtime and benchmark-gym behavior described below.

## Meeting-mode operator flow

The shipped operator probe also supports a meeting-specific path:

- `cargo run --quiet --bin simard_operator_probe -- meeting-run <base-type> <topology> <structured-objective>`

Use a structured objective with lines such as:

- `agenda: ...`
- `update: ...`
- `decision: ...`
- `risk: ...`
- `next-step: ...`
- `open-question: ...`

The v1 meeting contract is intentionally narrow:

- meeting mode uses the facilitator agent program backend `agent-program::meeting-facilitator`
- it persists concise decision memory under the durable state root
- it does not mutate code paths or pretend implementation work happened

## Benchmark gym CLI

The shipped benchmark CLI currently supports:

- `cargo run --quiet --bin simard-gym -- list`
- `cargo run --quiet --bin simard-gym -- run <scenario-id>`
- `cargo run --quiet --bin simard-gym -- run-suite starter`

The starter suite is intentionally small and exercises:

- `local-harness`
- `copilot-sdk`
- `rusty-clawd`
- the dedicated `simard-gym` identity
- the composite `simard-composite-engineer` identity

Artifacts are written under `target/simard-gym/` as JSON and text reports.

## Configuration

### Environment variables

| Variable | Required | Default | Description |
| --- | --- | --- | --- |
| `SIMARD_PROMPT_ROOT` | Yes in `explicit-config` | none | Root directory for prompt assets. |
| `SIMARD_OBJECTIVE` | Yes in `explicit-config` | none | Objective passed to `run()`. Live execution keeps the real objective in memory while persisted scratch, summary, reflection, and exported handoff session text store objective metadata instead of the raw objective text. |
| `SIMARD_STATE_ROOT` | Yes in `explicit-config` | none in `explicit-config`; `target/simard-state` in `builtin-defaults` | Root directory for the durable local memory, evidence, and latest handoff snapshot files written by the bootstrap path. |
| `SIMARD_BOOTSTRAP_MODE` | No | `explicit-config` | Startup mode. Accepted values: `explicit-config`, `builtin-defaults`. |
| `SIMARD_IDENTITY` | Yes in `explicit-config` | none in `explicit-config`; `simard-engineer` in `builtin-defaults` | Identity to load before runtime composition. Non-UTF-8 values fail bootstrap instead of being treated as missing. |
| `SIMARD_BASE_TYPE` | Yes in `explicit-config` | none in `explicit-config`; `local-harness` in `builtin-defaults` | Base type selected for the runtime request. Unsupported or unregistered choices fail explicitly. |
| `SIMARD_RUNTIME_TOPOLOGY` | Yes in `explicit-config` | none in `explicit-config`; `single-process` in `builtin-defaults` | Runtime topology selected for the runtime request. Accepted values: `single-process`, `multi-process`, `distributed`. |

### Current builtin base-type registrations

The builtin identities currently advertised by the loader are `simard-engineer`, `simard-meeting`, `simard-gym`, and the composite `simard-composite-engineer`. Their common builtin base-type registrations are:

| Base type selection | Current session backend implementation | Supported topologies in this scaffold |
| --- | --- | --- |
| `local-harness` | `local-harness` single-process local process harness session backend | `single-process` |
| `rusty-clawd` | `rusty-clawd::session-backend` real session backend | `single-process`, `multi-process` |
| `copilot-sdk` | `local-harness` single-process local process harness session backend (alias) | `single-process` |

Notes:

- bootstrap registers base-type factories from the manifest-advertised base-type list instead of assuming a single hardcoded local backend
- for `single-process`, bootstrap injects `topology::in-process`, `transport::in-memory-mailbox`, and `supervisor::in-process`
- for `multi-process` and `distributed`, bootstrap injects `topology::loopback-mesh`, `transport::loopback-mailbox`, and `supervisor::coordinated`
- unsupported topology/base-type pairs still fail explicitly; for example, `local-harness + multi-process` returns `UnsupportedTopology`
- if a future identity advertises a base type without a registered factory, runtime composition still fails explicitly with `AdapterNotRegistered`
- the descriptors remain truthful: `selected_base_type` preserves the explicit choice, while `adapter_backend.identity` exposes the actual backend (`rusty-clawd::session-backend` for `rusty-clawd`, `local-harness` for the current `copilot-sdk` alias)
- `runtime_node`, `mailbox_address`, `topology_backend`, `transport_backend`, `supervisor_backend`, and `handoff_backend` expose the actual runtime assembly rather than inferred labels
- `MemoryPolicy.allow_project_writes=true` is rejected explicitly in v1 rather than being ignored

### Bootstrap modes

| Mode | Behavior |
| --- | --- |
| `explicit-config` | Requires prompt root, objective, state root, identity, base type, and topology from configuration. |
| `builtin-defaults` | Allows builtin prompt root, builtin objective, builtin state root (`target/simard-state`), builtin identity, builtin base type (`local-harness`), and builtin topology (`single-process`), but only because startup opted in explicitly. |

### Config value sources

`ConfigValueSource` records where a resolved value came from.

| Variant | Meaning |
| --- | --- |
| `Environment(&'static str)` | The value came directly from an environment variable. |
| `ExplicitOptIn(&'static str)` | The value came from an explicit startup mode that permits builtin defaults. |

### Example: explicit bootstrap with `copilot-sdk`

This is the canonical non-default base-type example for the current scaffold:

```bash
SIMARD_PROMPT_ROOT="$PWD/prompt_assets" \
SIMARD_OBJECTIVE="inspect copilot-sdk bootstrap wiring" \
SIMARD_STATE_ROOT="$PWD/target/simard-state" \
SIMARD_IDENTITY="simard-engineer" \
SIMARD_BASE_TYPE="copilot-sdk" \
SIMARD_RUNTIME_TOPOLOGY="single-process" \
cargo run --quiet
```

Expected output shape:

```text
Simard local runtime executed successfully.
Bootstrap mode: explicit-config
Config sources: prompt_root=env:SIMARD_PROMPT_ROOT, objective=env:SIMARD_OBJECTIVE, state_root=env:SIMARD_STATE_ROOT, base_type=env:SIMARD_BASE_TYPE, topology=env:SIMARD_RUNTIME_TOPOLOGY
Bootstrap selection: identity=simard-engineer, base_type=copilot-sdk, topology=single-process
Adapter implementation: local-harness
...
Snapshot: state=ready, topology=single-process, base_type=copilot-sdk
Shutdown: stopped
```

This confirms three important contract points:

- bootstrap only uses the selected base type because you passed it explicitly
- the current v1 runtime accepts `copilot-sdk` immediately, but only with `single-process`
- the current v1 implementation behind that alias is still `local-harness`

### Persisted session text

Simard keeps the live objective available while the run is executing, but persisted session text is redacted down to objective metadata.

- session scratch records store `objective-metadata(chars=..., words=..., lines=...)`
- reflection summaries describe completion with objective metadata instead of raw objective text
- persisted session summaries reuse sanitized plan and execution strings rather than copying the raw objective back out
- exported handoff snapshots preserve the session boundary while replacing `RuntimeHandoffSnapshot.session.objective` with the same objective metadata string
- bootstrap persists the latest exported handoff snapshot under `SIMARD_STATE_ROOT/latest_handoff.json`

## Identity metadata

### `IdentityManifest`

`IdentityManifest` stores contract truth in `contract: ManifestContract`.

### `ManifestContract`

```rust
pub struct ManifestContract {
    pub entrypoint: String,
    pub composition: String,
    pub precedence: Vec<String>,
    pub provenance: Provenance,
    pub freshness: Freshness,
}
```

Notes:

- the CLI bootstrap path uses `simard::bootstrap::assemble_local_runtime` as the entrypoint
- provenance and freshness stay inside the contract so reflection carries a single source of truth
- invalid empty fields fail with `SimardError::InvalidManifestContract`

### Current precedence rules

`precedence` is ordered from highest to lowest influence within the bootstrap path. A typical local sequence is:

```text
mode:explicit-config
identity:simard-engineer
base-type:local-harness
topology:single-process
prompt-root:env:SIMARD_PROMPT_ROOT
state-root:env:SIMARD_STATE_ROOT
objective:env:SIMARD_OBJECTIVE
```

If builtin defaults are used intentionally, the prompt-root, state-root, and objective entries record `opt-in:SIMARD_BOOTSTRAP_MODE`.

## Provenance and freshness

### `Provenance`

```rust
pub struct Provenance {
    pub source: String,
    pub locator: String,
}
```

Helpers currently provided:

| Helper | Meaning |
| --- | --- |
| `Provenance::new(source, locator)` | Build an explicit provenance value. |
| `Provenance::builtin(locator)` | Mark a builtin metadata source. |
| `Provenance::injected(locator)` | Mark an injected metadata source. |
| `Provenance::runtime(locator)` | Mark runtime-derived metadata. |

### `Freshness`

```rust
pub enum FreshnessState {
    Current,
    Stale,
}

pub struct Freshness {
    pub state: FreshnessState,
    pub observed_at_unix_ms: u64,
}
```

Notes:

- `Freshness::now()` returns `SimardResult<Freshness>` and fails explicitly if the system clock is before the Unix epoch
- freshness can explicitly represent stale metadata when a caller needs to surface it

## Session identity

### `SessionId`

```rust
pub struct SessionId(String);
```

Rules:

- `UuidSessionIdGenerator` emits `session-<uuid-v7>`
- `SessionId::parse(...)` accepts a bare UUID or a `session-<uuid>` value and canonicalizes to `session-<uuid>`
- invalid values fail with `SimardError::InvalidSessionId`
- custom `SessionIdGenerator` implementations must return valid `SessionId` values
- the session ID strategy is injected through `RuntimePorts`; the local bootstrap path opts into `UuidSessionIdGenerator` explicitly

## Runtime lifecycle

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

### Stopped-state behavior

After `stop()` succeeds:

- `snapshot()` remains valid
- `run()` fails with `RuntimeStopped { action: "run" }`
- `start()` fails with `RuntimeStopped { action: "start" }`
- a repeated `stop()` fails with `RuntimeStopped { action: "stop" }`
- callers must compose a new runtime instead of reusing the stopped one

### Failed-state behavior

After `run()` fails:

- `snapshot()` remains valid
- the last session remains visible with `session_phase == Some(Failed)`
- manifest freshness is reported as `Stale`
- `start()` fails with `RuntimeFailed`
- `run()` fails with `RuntimeFailed`
- `stop()` is still the explicit way to close the lifecycle boundary

## Reflection snapshot

### `ReflectionSnapshot`

```rust
pub struct ReflectionSnapshot {
    pub identity_name: String,
    pub identity_components: Vec<String>,
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

For `simard-meeting`, reflection also reports `agent_program_backend.identity == "agent-program::meeting-facilitator"`.

### Backend descriptors

```rust
pub struct BackendDescriptor {
    pub identity: String,
    pub provenance: Provenance,
    pub freshness: Freshness,
}
```

Reflection rules:

- `manifest_contract` carries entrypoint, provenance, and freshness together
- `agent_program_backend` comes from the injected agent-program contract, not from hardcoded runtime logic
- `handoff_backend` comes from the injected handoff store used for export/import
- `adapter_backend` comes from the selected base-type factory/session descriptor, not from a bootstrap shortcut
- `runtime_node` and `mailbox_address` come from the injected topology and transport services
- `topology_backend`, `transport_backend`, and `supervisor_backend` come from the live runtime services
- `memory_backend` comes from the live memory store descriptor
- `evidence_backend` comes from the live evidence store descriptor
- reflection reports live wiring, not placeholder labels
- stopped or failed snapshots mark manifest freshness as `Stale`

## Errors

### Configuration and identity errors

| Error | Meaning |
| --- | --- |
| `MissingRequiredConfig` | Required startup configuration is absent. |
| `NonUnicodeConfigValue` | A configuration value exists but cannot be decoded as UTF-8. |
| `InvalidConfigValue` | A configuration value is present but invalid. |
| `UnknownIdentity` | Requested identity is not registered. |
| `InvalidIdentityComposition` | A composite identity definition is internally inconsistent. |
| `InvalidManifestContract` | Manifest contract metadata is incomplete or untruthful. |
| `ClockBeforeUnixEpoch` | Runtime metadata could not record a truthful observation time. |
| `InvalidSessionId` | A supplied session ID is not a valid distributed-safe identifier. |
| `UnsupportedBaseType` | The chosen identity does not allow the requested base type. |
| `AdapterNotRegistered` | No base-type factory has been registered for the requested base type. |
| `MissingCapability` | The selected base-type backend exists but does not satisfy the manifest's required capabilities. |
| `UnsupportedRuntimeTopology` | The injected topology driver does not support the requested topology. |
| `UnsupportedTopology` | The selected base-type backend does not support the requested topology. |

### Lifecycle and session errors

| Error | Meaning |
| --- | --- |
| `InvalidRuntimeTransition` | A runtime lifecycle transition is invalid. |
| `InvalidBaseTypeSessionState` | A base-type session was opened, used, or closed out of order. |
| `AdapterInvocationFailed` | The selected base-type backend failed while executing a turn. |
| `BaseTypeSessionCleanupFailed` | A base-type session failed during execution and then also failed to close cleanly. |
| `InvalidHandoffSnapshot` | A handoff snapshot could not be restored into the requested runtime. |
| `RuntimeStopped` | Caller attempted `start`, `run`, or `stop` after shutdown was already in effect. |
| `RuntimeFailed` | Caller attempted `start` or `run` after a failed execution but before shutdown. |
| `InvalidSessionTransition` | A session phase transition is invalid. |

### Prompt and storage errors

| Error | Meaning |
| --- | --- |
| `PromptAssetMissing` | A referenced prompt asset was not found. |
| `PromptAssetRead` | A prompt asset could not be read. |
| `StoragePoisoned` | A durable store lock was poisoned before Simard could read or persist state. |
## Example: truthful fields

```rust
use simard::{FreshnessState, ReflectiveRuntime};

let snapshot = runtime.snapshot()?;

assert_eq!(snapshot.runtime_state.to_string(), "ready");
assert_eq!(
    snapshot.manifest_contract.entrypoint,
    "simard::bootstrap::assemble_local_runtime"
);
assert_eq!(snapshot.manifest_contract.provenance.source, "bootstrap");
assert_eq!(snapshot.manifest_contract.freshness.state, FreshnessState::Current);
assert_eq!(snapshot.runtime_node.to_string(), "node-local");
assert_eq!(snapshot.mailbox_address.to_string(), "inmemory://node-local");
assert_eq!(snapshot.agent_program_backend.identity, "agent-program::objective-relay");
assert_eq!(snapshot.handoff_backend.identity, "handoff::json-file-store");
assert_eq!(snapshot.adapter_backend.identity, "local-harness");
assert_eq!(snapshot.topology_backend.identity, "topology::in-process");
assert_eq!(snapshot.transport_backend.identity, "transport::in-memory-mailbox");
assert_eq!(snapshot.supervisor_backend.identity, "supervisor::in-process");
assert_eq!(snapshot.memory_backend.identity, "memory::json-file-store");
assert_eq!(snapshot.evidence_backend.identity, "evidence::json-file-store");
```

## See also

- [How to configure bootstrap and inspect reflection](../howto/configure-bootstrap-and-inspect-reflection.md)
- [Concept: truthful runtime metadata](../concepts/truthful-runtime-metadata.md)
- [Tutorial: Run your first local session](../tutorials/run-your-first-local-session.md)

See the [documentation index](../index.md) for the full set of Simard docs.


## Handoff and migration

`RuntimeKernel::export_handoff()` exports the latest session metadata, memory records, and evidence records into a `RuntimeHandoffSnapshot` and persists it through the injected `RuntimeHandoffStore`.

Handoff notes for the current repository surface:

- the repository contract here is still the local CLI/operator path plus the in-process Rust runtime types; there is no HTTP, network-service, or database schema handoff contract in this branch
- `RuntimeHandoffSnapshot` should be treated as sensitive runtime state even after objective redaction because it still contains memory/evidence records and session linkage
- `RuntimeKernel::export_handoff()` preserves the latest session boundary but redacts `session.objective` down to `objective-metadata(...)` before persistence/export
- `RuntimeKernel::compose_from_handoff(...)` currently validates `identity_name` and `selected_base_type`, then rehydrates memory/evidence stores and preserves the redacted last-session boundary for a new process or node
