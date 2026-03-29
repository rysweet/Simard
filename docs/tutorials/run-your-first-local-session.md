---
title: "Tutorial: Run your first local session"
description: Learn the Simard local runtime flow, from bootstrap through reflection and shutdown.
last_updated: 2026-03-28
review_schedule: as-needed
owner: simard
doc_type: tutorial
related:
  - ../index.md
  - ../howto/configure-bootstrap-and-inspect-reflection.md
  - ../reference/runtime-contracts.md
---

# Tutorial: Run your first local session

This tutorial follows the runtime path that exists in the repository today.

## What you'll learn

- How the local runtime starts with explicit configuration
- How explicit opt-in defaults behave
- What reflection reports after a run
- How runtime node, mailbox, and backend wiring appear in reflection
- What stop semantics look like in practice

## Prerequisites

- Rust and Cargo installed
- A shell in the repository root

## Step 1: Run the current local runtime with explicit configuration

From the repository root, start Simard with a real prompt asset directory and an explicit objective.

For the builtin `simard-engineer` identity, you can currently choose `local-harness`, `rusty-clawd`, or `copilot-sdk` here. `rusty-clawd` is now a distinct backend, while `copilot-sdk` remains an explicit alias of the local harness implementation. The CLI bootstrap path still keeps `SIMARD_RUNTIME_TOPOLOGY="single-process"` because it injects the in-process topology driver.

```bash
SIMARD_PROMPT_ROOT="$PWD/prompt_assets" \
SIMARD_OBJECTIVE="exercise the local runtime" \
SIMARD_IDENTITY="simard-engineer" \
SIMARD_BASE_TYPE="local-harness" \
SIMARD_RUNTIME_TOPOLOGY="single-process" \
cargo run --quiet
```

You should see output shaped like this:

```text
Simard local runtime executed successfully.
Bootstrap mode: explicit-config
Config sources: prompt_root=env:SIMARD_PROMPT_ROOT, objective=env:SIMARD_OBJECTIVE, base_type=env:SIMARD_BASE_TYPE, topology=env:SIMARD_RUNTIME_TOPOLOGY
Bootstrap selection: identity=simard-engineer, base_type=local-harness, topology=single-process
Snapshot: state=ready, topology=single-process, base_type=local-harness
Adapter implementation: local-harness
Shutdown: stopped
```

**Checkpoint**: this is the real CLI path. `src/main.rs` is the thin wrapper; `bootstrap::run_local_session` owns the run loop, and `simard::bootstrap::assemble_local_runtime` remains the reflected assembly boundary.

## Step 2: Switch to another built-in base type

Run the same bootstrap path again, but select `copilot-sdk` explicitly.

```bash
SIMARD_PROMPT_ROOT="$PWD/prompt_assets" \
SIMARD_OBJECTIVE="exercise the copilot-sdk runtime path" \
SIMARD_IDENTITY="simard-engineer" \
SIMARD_BASE_TYPE="copilot-sdk" \
SIMARD_RUNTIME_TOPOLOGY="single-process" \
cargo run --quiet
```

Look for these lines:

```text
Bootstrap selection: identity=simard-engineer, base_type=copilot-sdk, topology=single-process
Snapshot: state=ready, topology=single-process, base_type=copilot-sdk
Adapter implementation: local-harness
```

**Checkpoint**: the runtime contract is explicit. `copilot-sdk` is selectable now, but the v1 scaffold still only supports `single-process`, and the underlying implementation stays `local-harness`. Simard preserves the selected base type without pretending it is already a distinct backend integration.

## Step 3: Opt in to builtin defaults

Builtin defaults exist for local bootstrap convenience, but they are only used when startup opts in.

```bash
SIMARD_BOOTSTRAP_MODE=builtin-defaults \
cargo run --quiet
```

You should see:

- `Bootstrap mode: builtin-defaults`
- `prompt_root=opt-in:SIMARD_BOOTSTRAP_MODE`
- `objective=opt-in:SIMARD_BOOTSTRAP_MODE`
- `base_type=opt-in:SIMARD_BOOTSTRAP_MODE`
- `topology=opt-in:SIMARD_BOOTSTRAP_MODE`
- the builtin identity `simard-engineer`

**Checkpoint**: defaults are a startup choice, not a recovery path. This part of the audited contract already exists.

## Step 4: Observe stopped-state behavior

The runtime preserves its snapshot after shutdown and surfaces a dedicated stopped-state error:

```rust
use simard::{RuntimeState, SimardError};

runtime.stop()?;

let snapshot = runtime.snapshot()?;
assert_eq!(snapshot.runtime_state, RuntimeState::Stopped);

let error = runtime.run("should fail after stop").unwrap_err();
assert_eq!(
    error,
    SimardError::RuntimeStopped {
        action: "run".to_string(),
    }
);
```

**Checkpoint**: stop is an observable lifecycle boundary. Snapshot inspection still works, but execution does not resume.

After shutdown, the reflected manifest freshness becomes `Stale` so callers can tell they are looking at post-stop metadata instead of a live runtime.

## Step 5: Inspect truthful reflection metadata

After a successful run, reflection reports the assembled contract and backend descriptors:

```rust
use simard::{FreshnessState, ReflectiveRuntime};

let snapshot = runtime.snapshot()?;

assert_eq!(
    snapshot.manifest_contract.entrypoint,
    "simard::bootstrap::assemble_local_runtime"
);
assert_eq!(snapshot.manifest_contract.provenance.source, "bootstrap");
assert_eq!(snapshot.manifest_contract.freshness.state, FreshnessState::Current);
assert_eq!(snapshot.runtime_node.to_string(), "node-local");
assert_eq!(snapshot.mailbox_address.to_string(), "inmemory://node-local");
assert_eq!(snapshot.agent_program_backend.identity, "agent-program::objective-relay");
assert_eq!(snapshot.handoff_backend.identity, "handoff::in-memory");
assert_eq!(snapshot.adapter_backend.identity, "local-harness");
assert_eq!(snapshot.transport_backend.identity, "transport::in-memory-mailbox");
```

If you launched with `SIMARD_BASE_TYPE="copilot-sdk"`, `snapshot.selected_base_type` still shows the explicit selection while `snapshot.adapter_backend.identity` remains `local-harness`. If you launched with `SIMARD_BASE_TYPE="rusty-clawd"`, reflection now reports `rusty-clawd::session-backend`. The runtime-side wiring is also explicit: the current scaffold reports `node-local`, `inmemory://node-local`, `handoff::in-memory`, and the injected in-process topology, transport, and supervisor backends instead of implying future distributed support.

## Summary

You now know:

- how to run the local runtime with explicit config
- how to switch between built-in base types without hidden inference
- how `copilot-sdk` still aliases `local-harness` while `rusty-clawd` now reports a distinct backend honestly
- how opt-in defaults are recorded
- how reflection reports truthful runtime metadata
- how stop semantics behave after shutdown

## Next steps

- Use the [bootstrap and reflection how-to](../howto/configure-bootstrap-and-inspect-reflection.md) to inspect the reflection surface in more detail.
- Use the [runtime contracts reference](../reference/runtime-contracts.md) when you need exact API details.
- Read [truthful runtime metadata](../concepts/truthful-runtime-metadata.md) for the design rationale behind the contract.

See the [documentation index](../index.md) for the rest of the Simard docs.
