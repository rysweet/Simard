---
title: "Tutorial: Run your first local session"
description: Learn the Simard local runtime flow, from bootstrap through reflection and shutdown.
last_updated: 2026-03-27
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
- What stop semantics look like in practice

## Prerequisites

- Rust and Cargo installed
- A shell in the repository root

## Step 1: Run the current local runtime with explicit configuration

From the repository root, start Simard with a real prompt asset directory and an explicit objective.

```bash
SIMARD_PROMPT_ROOT="$PWD/prompt_assets" \
SIMARD_OBJECTIVE="exercise the local runtime" \
cargo run --quiet
```

You should see output shaped like this:

```text
Simard local runtime executed successfully.
Bootstrap mode: explicit-config
Config sources: prompt_root=env:SIMARD_PROMPT_ROOT, objective=env:SIMARD_OBJECTIVE
Plan: ...
Execution: ...
Reflection: ...
Snapshot: state=ready, topology=single-process, base_type=local-harness
Shutdown: stopped
```

**Checkpoint**: this is the real CLI path. `src/main.rs` is the thin wrapper; `bootstrap::assemble_local_runtime` performs runtime assembly, then the binary runs once, prints the snapshot, and stops cleanly.

## Step 2: Opt in to builtin defaults

Builtin defaults exist for local bootstrap convenience, but they are only used when startup opts in.

```bash
SIMARD_BOOTSTRAP_MODE=builtin-defaults \
cargo run --quiet
```

You should see:

- `Bootstrap mode: builtin-defaults`
- `prompt_root=opt-in:SIMARD_BOOTSTRAP_MODE`
- `objective=opt-in:SIMARD_BOOTSTRAP_MODE`
- the builtin identity `simard-engineer`

**Checkpoint**: defaults are a startup choice, not a recovery path. This part of the audited contract already exists.

## Step 3: Observe stopped-state behavior

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

## Step 4: Inspect truthful reflection metadata

After a successful run, reflection reports the assembled contract and backend descriptors:

```rust
use simard::{FreshnessState, ReflectiveRuntime};

let snapshot = runtime.snapshot()?;

assert_eq!(
    snapshot.manifest_contract.entrypoint,
    "bootstrap::assemble_local_runtime"
);
assert_eq!(snapshot.manifest_contract.provenance.source, "bootstrap");
assert_eq!(snapshot.manifest_contract.freshness.state, FreshnessState::Current);
assert_eq!(snapshot.adapter_backend.identity, "local-harness");
```

## Summary

You now know:

- how to run the local runtime with explicit config
- how opt-in defaults are recorded
- how reflection reports truthful runtime metadata
- how stop semantics behave after shutdown

## Next steps

- Use the [bootstrap and reflection how-to](../howto/configure-bootstrap-and-inspect-reflection.md) to inspect the reflection surface in more detail.
- Use the [runtime contracts reference](../reference/runtime-contracts.md) when you need exact API details.
- Read [truthful runtime metadata](../concepts/truthful-runtime-metadata.md) for the design rationale behind the contract.

See the [documentation index](../index.md) for the rest of the Simard docs.
