---
title: "How to configure bootstrap and inspect reflection"
description: Verify the Simard bootstrap path and inspect the truthful reflection snapshot exposed by the runtime.
last_updated: 2026-03-28
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../index.md
  - ../reference/runtime-contracts.md
  - ../concepts/truthful-runtime-metadata.md
---

# How to configure bootstrap and inspect reflection

Use this guide when you need to answer two questions:

- what bootstrap inputs did Simard actually use?
- what does the live runtime report through reflection?

## Prerequisites

- [ ] You are in the repository root
- [ ] `cargo run --quiet` works in your environment
- [ ] You know whether you want explicit config or opt-in builtin defaults

## 1. Use explicit configuration by default

Provide both the prompt root and objective yourself.

For the builtin `simard-engineer` identity, the current local scaffold accepts `local-harness`, `rusty-clawd`, or `copilot-sdk` as explicit base-type choices. All three currently run through the same single-process adapter shape, so `SIMARD_RUNTIME_TOPOLOGY` must still be `single-process`.

```bash
SIMARD_PROMPT_ROOT="$PWD/prompt_assets" \
SIMARD_OBJECTIVE="verify current reflection metadata" \
SIMARD_IDENTITY="simard-engineer" \
SIMARD_BASE_TYPE="local-harness" \
SIMARD_RUNTIME_TOPOLOGY="single-process" \
cargo run --quiet
```

In the current repo:

- missing `SIMARD_PROMPT_ROOT` fails bootstrap
- missing `SIMARD_OBJECTIVE` fails bootstrap
- missing `SIMARD_IDENTITY` fails bootstrap
- missing `SIMARD_BASE_TYPE` fails bootstrap
- missing `SIMARD_RUNTIME_TOPOLOGY` fails bootstrap
- unknown `SIMARD_IDENTITY` fails identity loading
- invalid `SIMARD_RUNTIME_TOPOLOGY` values fail bootstrap config resolution
- identity/base-type mismatches fail runtime composition with `UnsupportedBaseType`
- manifest-supported but unregistered `SIMARD_BASE_TYPE` values fail runtime composition with `AdapterNotRegistered`
- valid but unsupported `SIMARD_RUNTIME_TOPOLOGY` values fail runtime composition with `UnsupportedTopology`

No missing value is replaced after startup.

### Variation: exercise a non-default builtin base type

Use this when you want to prove that bootstrap is not silently snapping back to `local-harness`.

```bash
SIMARD_PROMPT_ROOT="$PWD/prompt_assets" \
SIMARD_OBJECTIVE="verify copilot-sdk bootstrap selection" \
SIMARD_IDENTITY="simard-engineer" \
SIMARD_BASE_TYPE="copilot-sdk" \
SIMARD_RUNTIME_TOPOLOGY="single-process" \
cargo run --quiet
```

In the current repo, you should see output shaped like:

```text
Bootstrap selection: identity=simard-engineer, base_type=copilot-sdk, topology=single-process
Snapshot: state=ready, topology=single-process, base_type=copilot-sdk
```

That is the important contract boundary: the runtime records the explicit selection you asked for, and it does not reinterpret `copilot-sdk` as `local-harness`.

## 2. Opt in to builtin defaults only when you mean it

For local bootstrap, Simard supports explicit opt-in defaults.

```bash
SIMARD_BOOTSTRAP_MODE=builtin-defaults \
cargo run --quiet
```

In the current repo:

- `SIMARD_PROMPT_ROOT` resolves to the repository `prompt_assets/` directory
- `SIMARD_OBJECTIVE` resolves to the builtin engineer-loop objective
- `SIMARD_IDENTITY` resolves to `simard-engineer`
- `SIMARD_BASE_TYPE` resolves to `local-harness`
- `SIMARD_RUNTIME_TOPOLOGY` resolves to `single-process`
- the configuration source is recorded as `opt-in:SIMARD_BOOTSTRAP_MODE`

Builtin defaults are startup choices. They are not recovery behavior.

## 3. Inspect the reflection fields

`ReflectionSnapshot` exposes the truth-bearing runtime metadata directly:

- `manifest_contract`
- `adapter_backend`
- `memory_backend`
- `evidence_backend`

For the CLI bootstrap path, the manifest entrypoint is the bootstrap assembly boundary, not the thin binary wrapper.

```rust
use simard::{FreshnessState, ReflectiveRuntime};

let snapshot = runtime.snapshot()?;

assert_eq!(
    snapshot.manifest_contract.entrypoint,
    "simard::bootstrap::assemble_local_runtime"
);
assert_eq!(snapshot.manifest_contract.provenance.source, "bootstrap");
assert_eq!(
    snapshot.manifest_contract.freshness.state,
    FreshnessState::Current
);
assert_eq!(snapshot.adapter_backend.identity, "local-harness");
assert_eq!(snapshot.memory_backend.identity, "memory::session-cache");
assert_eq!(snapshot.evidence_backend.identity, "evidence::append-only-log");
```

If you launched with `SIMARD_BASE_TYPE="copilot-sdk"` or `SIMARD_BASE_TYPE="rusty-clawd"`, the same rule applies: `snapshot.adapter_backend.identity` must match the chosen base type exactly.

## 4. Validate stopped-state behavior

Stopping the runtime is already observable.

```rust
use simard::{RuntimeState, SimardError};

runtime.stop()?;

assert_eq!(runtime.snapshot()?.runtime_state, RuntimeState::Stopped);
assert_eq!(
    runtime.snapshot()?.manifest_contract.freshness.state,
    simard::FreshnessState::Stale
);
assert_eq!(
    runtime.start().unwrap_err(),
    SimardError::RuntimeStopped {
        action: "start".to_string(),
    }
);
```

The same rule applies to `run()` and repeated `stop()`: once a runtime is stopped, callers must compose a new instance.

## Variations

### For a custom identity

Set `SIMARD_IDENTITY` before startup:

```bash
SIMARD_PROMPT_ROOT="$PWD/prompt_assets" \
SIMARD_OBJECTIVE="run custom identity" \
SIMARD_IDENTITY="simard-engineer" \
SIMARD_BASE_TYPE="local-harness" \
SIMARD_RUNTIME_TOPOLOGY="single-process" \
cargo run --quiet
```

If the identity is unknown, Simard returns `SimardError::UnknownIdentity`. It does not substitute another identity.

### For injected session IDs

A valid session ID is canonicalized to `session-<uuid>`, and the local UUID strategy is injected explicitly through bootstrap/runtime composition instead of being created as a hidden runtime default.

```rust
use simard::SessionId;

let session_id = SessionId::parse("session-018f1f7e-4c5d-7b2a-8f10-b5c0d4f7b123")?;
assert!(session_id.as_str().starts_with("session-"));
```

Invalid values fail with `SimardError::InvalidSessionId`.

## Troubleshooting

### Missing required bootstrap config

**Symptom**: startup fails before the runtime is composed.

**Solution**:

```bash
export SIMARD_PROMPT_ROOT="$PWD/prompt_assets"
export SIMARD_OBJECTIVE="verify current reflection metadata"
export SIMARD_IDENTITY="simard-engineer"
export SIMARD_BASE_TYPE="local-harness"
export SIMARD_RUNTIME_TOPOLOGY="single-process"
```

Or opt in explicitly:

```bash
export SIMARD_BOOTSTRAP_MODE=builtin-defaults
```

### Prompt assets fail to load

**Symptom**: `PromptAssetMissing` or `PromptAssetRead`.

**Solution**: fix the asset path or contents. Simard does not continue with a different prompt asset.

### Base type or topology selection fails

**Symptom**: bootstrap resolves, but runtime composition returns `UnsupportedBaseType`, `AdapterNotRegistered`, or `UnsupportedTopology`.

**Solution**: pick a base type the identity allows, make sure the adapter is registered for that identity, and choose a topology the selected adapter supports. Simard does not substitute a different base type or downgrade the topology silently.

Today, the builtin base types `local-harness`, `rusty-clawd`, and `copilot-sdk` all require `SIMARD_RUNTIME_TOPOLOGY=single-process`.

### Reflection metadata is truthful but incomplete

**Symptom**: the reflection values do not match the runtime you actually assembled.

**Solution**: inspect the bootstrap inputs and the selected base type. Reflection reports the active wiring, so incorrect metadata usually means the runtime was assembled differently than expected.

### Calls fail after stop

**Symptom**: `run()`, `start()`, or a repeated `stop()` returns `RuntimeStopped`.

**Solution**: compose a new runtime instance. Stopped runtimes remain inspectable, but they are not reusable.

## See also

- [Runtime contracts reference](../reference/runtime-contracts.md)
- [Concept: truthful runtime metadata](../concepts/truthful-runtime-metadata.md)
- [Tutorial: Run your first local session](../tutorials/run-your-first-local-session.md)

See the [documentation index](../index.md) for the full set of Simard docs.
