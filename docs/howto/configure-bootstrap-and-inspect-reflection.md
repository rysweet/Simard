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

Provide the prompt root, objective, and state root yourself.

For the builtin identities in this repo, the current scaffold accepts `local-harness`, `rusty-clawd`, or `copilot-sdk` as explicit base-type choices. `rusty-clawd` is a distinct session backend, while `copilot-sdk` remains an explicit alias of `local-harness`. The bootstrap path now injects either the in-process runtime services for `single-process` or the loopback mesh services for `multi-process`, so unsupported topology/base-type pairs fail explicitly instead of being rewritten.

```bash
SIMARD_PROMPT_ROOT="$PWD/prompt_assets" \
SIMARD_OBJECTIVE="verify current reflection metadata" \
SIMARD_STATE_ROOT="$PWD/target/simard-state" \
SIMARD_IDENTITY="simard-engineer" \
SIMARD_BASE_TYPE="local-harness" \
SIMARD_RUNTIME_TOPOLOGY="single-process" \
cargo run --quiet
```

In the current repo:

- missing `SIMARD_PROMPT_ROOT` fails bootstrap
- missing `SIMARD_OBJECTIVE` fails bootstrap
- missing `SIMARD_STATE_ROOT` fails bootstrap
- missing `SIMARD_IDENTITY` fails bootstrap
- missing `SIMARD_BASE_TYPE` fails bootstrap
- missing `SIMARD_RUNTIME_TOPOLOGY` fails bootstrap
- unknown `SIMARD_IDENTITY` fails identity loading
- invalid `SIMARD_RUNTIME_TOPOLOGY` values fail bootstrap config resolution
- identity/base-type mismatches fail runtime composition with `UnsupportedBaseType`
- manifest-supported but unregistered `SIMARD_BASE_TYPE` values fail runtime composition with `AdapterNotRegistered`
- valid but unsupported `SIMARD_RUNTIME_TOPOLOGY` values fail runtime composition explicitly, usually with `UnsupportedRuntimeTopology` on the local bootstrap path and `UnsupportedTopology` if a runtime driver supports the topology but the selected base type does not

No missing value is replaced after startup.

### Variation: exercise a non-default builtin base type

Use this when you want to prove that bootstrap is not silently snapping back to `local-harness`.

```bash
SIMARD_PROMPT_ROOT="$PWD/prompt_assets" \
SIMARD_OBJECTIVE="verify copilot-sdk bootstrap selection" \
SIMARD_STATE_ROOT="$PWD/target/simard-state" \
SIMARD_IDENTITY="simard-engineer" \
SIMARD_BASE_TYPE="copilot-sdk" \
SIMARD_RUNTIME_TOPOLOGY="single-process" \
cargo run --quiet
```

In the current repo, you should see output shaped like:

```text
Bootstrap selection: identity=simard-engineer, base_type=copilot-sdk, topology=single-process
Snapshot: state=ready, topology=single-process, base_type=copilot-sdk
Adapter implementation: local-harness
```

That is the important contract boundary: the runtime records the explicit selection you asked for, and it also reports the honest v1 implementation identity. Simard does not silently rewrite your selection, but it also does not pretend the alias is already a distinct backend.

## 2. Opt in to builtin defaults only when you mean it

For local bootstrap, Simard supports explicit opt-in defaults.

```bash
SIMARD_BOOTSTRAP_MODE=builtin-defaults \
cargo run --quiet
```

In the current repo:

- `SIMARD_PROMPT_ROOT` resolves to the repository `prompt_assets/` directory
- `SIMARD_OBJECTIVE` resolves to the builtin engineer-loop objective
- `SIMARD_STATE_ROOT` resolves to the repository `target/simard-state` directory
- `SIMARD_IDENTITY` resolves to `simard-engineer`
- `SIMARD_BASE_TYPE` resolves to `local-harness`
- `SIMARD_RUNTIME_TOPOLOGY` resolves to the topology you selected, with builtin defaults still opting into `single-process`
- the configuration source is recorded as `opt-in:SIMARD_BOOTSTRAP_MODE`

Builtin defaults are startup choices. They are not recovery behavior.

## 3. Inspect the reflection fields

`ReflectionSnapshot` exposes the truth-bearing runtime metadata directly:

- `manifest_contract`
- `runtime_node`
- `mailbox_address`
- `agent_program_backend`
- `handoff_backend`
- `adapter_backend`
- `topology_backend`
- `transport_backend`
- `supervisor_backend`
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

If you launched with `SIMARD_BASE_TYPE="copilot-sdk"`, `snapshot.selected_base_type` still shows the alias you chose while `snapshot.adapter_backend.identity` remains `local-harness`. If you launched with `SIMARD_BASE_TYPE="rusty-clawd"`, reflection truthfully reports `snapshot.adapter_backend.identity == "rusty-clawd::session-backend"`. Composite identities also surface `snapshot.identity_components` so operator tooling can see which roles were assembled.

The same redaction rule applies to persisted session text: scratch memory, session summaries, and reflection summaries record `objective-metadata(...)` instead of the raw `SIMARD_OBJECTIVE` string.

## 5. Exercise meeting mode without switching into engineer mode

Use the operator probe when you want to validate facilitator behavior directly:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  meeting-run local-harness single-process \
  "$(cat <<'EOF'
agenda: align the next Simard block
update: durable memory is now merged
decision: prioritize meeting-mode validation before remote orchestration
risk: workflow automation is still unreliable in clean worktrees
next-step: ship operator-visible meeting coverage
open-question: when should meeting decisions influence engineer planning?
EOF
)"
```

Look for:

- `Probe mode: meeting-run`
- `Identity: simard-meeting`
- `Decision records: 1`
- a durable decision record containing the decision, risk, next step, and open question

This is the current honest v1 behavior: meeting mode captures structured planning output and writes concise decision memory, but it does not mutate code.

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
SIMARD_STATE_ROOT="$PWD/target/simard-state" \
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
export SIMARD_STATE_ROOT="$PWD/target/simard-state"
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

**Symptom**: bootstrap resolves, but runtime composition returns `UnsupportedBaseType`, `AdapterNotRegistered`, `UnsupportedRuntimeTopology`, or `UnsupportedTopology`.

**Solution**: pick a base type the identity allows, make sure the base-type factory is registered for that identity, and choose a topology supported by both the injected runtime services and the selected base-type backend. Simard does not substitute a different base type or downgrade the topology silently.

Today, builtin defaults still choose `single-process`, `copilot-sdk` still reports `Adapter implementation: local-harness`, and `rusty-clawd` reports `Adapter implementation: rusty-clawd::session-backend`. If you request `multi-process`, bootstrap now injects the loopback mesh topology/transport/supervisor path, and composition still fails explicitly if the selected base type does not support that topology.

### Project writes are rejected in v1

**Symptom**: manifest construction or runtime composition returns `UnsupportedMemoryPolicy`.

**Solution**: keep `MemoryPolicy.allow_project_writes=false` until Simard ships an explicit project-write contract. The current runtime accepts summary scope selection, but not repository mutation through memory policy.

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
