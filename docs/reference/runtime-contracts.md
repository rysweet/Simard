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

Simard v1 currently exposes two surfaces:

- the local CLI bootstrap path through `cargo run --quiet`
- the in-process Rust runtime/bootstrap types in `src/bootstrap.rs`, `src/runtime.rs`, and related modules

Simard v1 does **not** currently expose:

- an HTTP API
- a network service contract
- a database schema contract

The stable contract in this repository is the bootstrap/runtime behavior described below.

## Configuration

### Environment variables

| Variable | Required | Default | Description |
| --- | --- | --- | --- |
| `SIMARD_PROMPT_ROOT` | Yes in `explicit-config` | none | Root directory for prompt assets. |
| `SIMARD_OBJECTIVE` | Yes in `explicit-config` | none | Objective passed to `run()`. |
| `SIMARD_BOOTSTRAP_MODE` | No | `explicit-config` | Startup mode. Accepted values: `explicit-config`, `builtin-defaults`. |
| `SIMARD_IDENTITY` | Yes in `explicit-config` | none in `explicit-config`; `simard-engineer` in `builtin-defaults` | Identity to load before runtime composition. Non-UTF-8 values fail bootstrap instead of being treated as missing. |
| `SIMARD_BASE_TYPE` | Yes in `explicit-config` | none in `explicit-config`; `local-harness` in `builtin-defaults` | Base type selected for the runtime request. Unsupported or unregistered choices fail explicitly. |
| `SIMARD_RUNTIME_TOPOLOGY` | Yes in `explicit-config` | none in `explicit-config`; `single-process` in `builtin-defaults` | Runtime topology selected for the runtime request. Accepted values: `single-process`, `multi-process`, `distributed`. |

### Current builtin base-type registrations

The builtin `simard-engineer` identity currently advertises and local bootstrap registers these base types:

| Base type | Current adapter shape | Supported topologies in this scaffold |
| --- | --- | --- |
| `local-harness` | single-process local process harness adapter | `single-process` |
| `rusty-clawd` | single-process local process harness adapter | `single-process` |
| `copilot-sdk` | single-process local process harness adapter | `single-process` |

Notes:

- bootstrap registers adapters from the manifest-advertised base-type list instead of assuming a single hardcoded local adapter
- for v1, `multi-process` and `distributed` are explicit configuration values but not supported builtin deployment modes; selecting either with the builtin adapters fails explicitly with `UnsupportedTopology`
- if a future identity advertises a base type without a registered adapter, runtime composition still fails explicitly with `AdapterNotRegistered`
- the descriptors remain truthful: `adapter_backend` is copied from the selected adapter instance's descriptor, so `adapter_backend.identity` reflects the chosen base type (`local-harness`, `rusty-clawd`, or `copilot-sdk`) even though the current v1 builtin adapters are all instantiated from the same local harness implementation shape

### Bootstrap modes

| Mode | Behavior |
| --- | --- |
| `explicit-config` | Requires prompt root, objective, identity, base type, and topology from configuration. |
| `builtin-defaults` | Allows builtin prompt root, builtin objective, builtin identity, builtin base type (`local-harness`), and builtin topology (`single-process`), but only because startup opted in explicitly. |

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
SIMARD_IDENTITY="simard-engineer" \
SIMARD_BASE_TYPE="copilot-sdk" \
SIMARD_RUNTIME_TOPOLOGY="single-process" \
cargo run --quiet
```

Expected output shape:

```text
Simard local runtime executed successfully.
Bootstrap mode: explicit-config
Config sources: prompt_root=env:SIMARD_PROMPT_ROOT, objective=env:SIMARD_OBJECTIVE, base_type=env:SIMARD_BASE_TYPE, topology=env:SIMARD_RUNTIME_TOPOLOGY
Bootstrap selection: identity=simard-engineer, base_type=copilot-sdk, topology=single-process
...
Snapshot: state=ready, topology=single-process, base_type=copilot-sdk
Shutdown: stopped
```

This confirms two important contract points:

- bootstrap only uses the selected base type because you passed it explicitly
- the current v1 runtime accepts `copilot-sdk` immediately, but only with `single-process`

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
objective:env:SIMARD_OBJECTIVE
```

If builtin defaults are used intentionally, the prompt-root and objective entries record `opt-in:SIMARD_BOOTSTRAP_MODE`.

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
| `Active` | Adapter invocation is in progress. |
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
    pub selected_base_type: BaseTypeId,
    pub topology: RuntimeTopology,
    pub runtime_state: RuntimeState,
    pub session_phase: Option<SessionPhase>,
    pub prompt_assets: Vec<PromptAssetId>,
    pub manifest_contract: ManifestContract,
    pub evidence_records: usize,
    pub memory_records: usize,
    pub adapter_backend: BackendDescriptor,
    pub memory_backend: BackendDescriptor,
    pub evidence_backend: BackendDescriptor,
}
```

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
- `adapter_backend` comes from `descriptor()` on the selected adapter instance, not from a shared implementation label or bootstrap shortcut
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
| `InvalidManifestContract` | Manifest contract metadata is incomplete or untruthful. |
| `ClockBeforeUnixEpoch` | Runtime metadata could not record a truthful observation time. |
| `InvalidSessionId` | A supplied session ID is not a valid distributed-safe identifier. |
| `UnsupportedBaseType` | The chosen identity does not allow the requested base type. |
| `AdapterNotRegistered` | No adapter has been registered for the requested base type. |
| `MissingCapability` | The selected adapter exists but does not satisfy the manifest's required capabilities. |
| `UnsupportedTopology` | The selected adapter does not support the requested topology. |

### Lifecycle and session errors

| Error | Meaning |
| --- | --- |
| `InvalidRuntimeTransition` | A runtime lifecycle transition is invalid. |
| `RuntimeStopped` | Caller attempted `start`, `run`, or `stop` after shutdown was already in effect. |
| `RuntimeFailed` | Caller attempted `start` or `run` after a failed execution but before shutdown. |
| `InvalidSessionTransition` | A session phase transition is invalid. |

### Prompt and storage errors

| Error | Meaning |
| --- | --- |
| `PromptAssetMissing` | A referenced prompt asset was not found. |
| `PromptAssetRead` | A prompt asset could not be read. |
| `StoragePoisoned` | An in-memory store lock was poisoned. |
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
assert_eq!(snapshot.adapter_backend.identity, "local-harness");
assert_eq!(snapshot.memory_backend.identity, "memory::session-cache");
```

## See also

- [How to configure bootstrap and inspect reflection](../howto/configure-bootstrap-and-inspect-reflection.md)
- [Concept: truthful runtime metadata](../concepts/truthful-runtime-metadata.md)
- [Tutorial: Run your first local session](../tutorials/run-your-first-local-session.md)

See the [documentation index](../index.md) for the full set of Simard docs.
