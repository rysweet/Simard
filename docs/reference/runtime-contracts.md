---
title: Runtime contracts reference
description: Reference for the current Simard runtime surface, reflection contracts, and lifecycle errors.
last_updated: 2026-03-27
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

## Configuration

### Environment variables

| Variable | Required | Default | Description |
| --- | --- | --- | --- |
| `SIMARD_PROMPT_ROOT` | Yes in `explicit-config` | none | Root directory for prompt assets. |
| `SIMARD_OBJECTIVE` | Yes in `explicit-config` | none | Objective passed to `run()`. |
| `SIMARD_BOOTSTRAP_MODE` | No | `explicit-config` | Startup mode. Accepted values: `explicit-config`, `builtin-defaults`. |
| `SIMARD_IDENTITY` | Yes in `explicit-config` | none in `explicit-config`; `simard-engineer` in `builtin-defaults` | Identity to load before runtime composition. Non-UTF-8 values fail bootstrap instead of being treated as missing. |

### Bootstrap modes

| Mode | Behavior |
| --- | --- |
| `explicit-config` | Requires prompt root, objective, and identity from configuration. |
| `builtin-defaults` | Allows builtin prompt root, builtin objective, and builtin identity, but only because startup opted in explicitly. |

### Config value sources

`ConfigValueSource` records where a resolved value came from.

| Variant | Meaning |
| --- | --- |
| `Environment(&'static str)` | The value came directly from an environment variable. |
| `ExplicitOptIn(&'static str)` | The value came from an explicit startup mode that permits builtin defaults. |

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
- `adapter_backend` comes from the runtime-selected adapter descriptor
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
