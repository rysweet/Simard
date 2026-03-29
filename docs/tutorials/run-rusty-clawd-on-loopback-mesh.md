---
title: "Tutorial: Run RustyClawd on the loopback mesh"
description: Learn the alternate Simard runtime path by composing the topology-neutral runtime kernel with the RustyClawd backend and the loopback mesh services.
last_updated: 2026-03-29
review_schedule: as-needed
owner: simard
doc_type: tutorial
related:
  - ../index.md
  - ../reference/runtime-contracts.md
  - ../howto/export-and-restore-runtime-handoff.md
  - ../concepts/topology-neutral-runtime-kernel.md
---

# Tutorial: Run RustyClawd on the loopback mesh

This tutorial walks the second runtime path in Simard: a multi-process style runtime assembled in-process with `rusty-clawd`, `LoopbackMeshTopologyDriver`, and `LoopbackMailboxTransport`.

It intentionally uses `RuntimeTopology::MultiProcess`. Although the runtime contract also includes `Distributed`, the repository's registered base types and examples do not yet provide an end-to-end distributed run.

## What you'll learn

- how the topology-neutral runtime kernel is composed without the CLI bootstrap shortcut
- how to select `rusty-clawd` as a real backend instead of an alias
- what reflection reports for the loopback mesh runtime
- why the alternate topology path is available through the Rust API but not through the default CLI path

## Prerequisites

- Rust and Cargo installed
- A shell in the repository root
- Familiarity with the [local session tutorial](./run-your-first-local-session.md)

## Step 1: Compose the alternate runtime services

The default CLI path always assembles `single-process` services. For the loopback mesh path, inject the topology, transport, supervisor, and handoff services yourself.

```rust
use std::sync::Arc;

use simard::{
    BaseTypeCapability, BaseTypeId, BaseTypeRegistry, Freshness, IdentityManifest,
    InMemoryEvidenceStore, InMemoryHandoffStore, InMemoryMemoryStore, InMemoryPromptAssetStore,
    InProcessSupervisor, LoopbackMailboxTransport, LoopbackMeshTopologyDriver, LocalRuntime,
    ManifestContract, MemoryPolicy, ObjectiveRelayProgram, OperatingMode, PromptAsset,
    PromptAssetRef, Provenance, RuntimePorts, RuntimeRequest, RuntimeTopology,
    RustyClawdAdapter, UuidSessionIdGenerator, capability_set,
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
            vec!["docs:loopback-tutorial".to_string()],
            Provenance::new("docs", "tutorial::loopback-mesh"),
            Freshness::now()?,
        )?,
    )?,
    BaseTypeId::new("rusty-clawd"),
    RuntimeTopology::MultiProcess,
);

let mut runtime = LocalRuntime::compose(
    RuntimePorts::with_runtime_services_and_program(
        prompts,
        memory,
        evidence,
        base_types,
        Arc::new(LoopbackMeshTopologyDriver::try_default()?),
        Arc::new(LoopbackMailboxTransport::try_default()?),
        Arc::new(InProcessSupervisor::try_default()?),
        Arc::new(ObjectiveRelayProgram::try_default()?),
        handoff,
        Arc::new(UuidSessionIdGenerator),
    ),
    request,
)?;
```

**Checkpoint**: you now have a runtime that requested `RuntimeTopology::MultiProcess` instead of the CLI default `single-process` path.

## Step 2: Start the runtime and run a session

```rust
runtime.start()?;
let outcome = runtime.run("exercise the loopback mesh runtime")?;
```

Inspect the high-level result:

```rust
assert_eq!(outcome.session.selected_base_type, BaseTypeId::new("rusty-clawd"));
assert_eq!(outcome.reflection.snapshot.topology, RuntimeTopology::MultiProcess);
assert!(outcome.reflection.summary.contains("rusty-clawd"));
```

**Checkpoint**: the runtime executed with the selected base type you asked for. It did not silently fall back to `local-harness`.

## Step 3: Inspect truthful reflection

The reflected metadata should come from the live runtime wiring.

```rust
let snapshot = runtime.snapshot()?;

assert_eq!(snapshot.topology, RuntimeTopology::MultiProcess);
assert_eq!(snapshot.runtime_node.to_string(), "node-loopback-mesh");
assert_eq!(snapshot.mailbox_address.to_string(), "loopback://node-loopback-mesh");
assert_eq!(snapshot.adapter_backend.identity, "rusty-clawd::session-backend");
assert_eq!(snapshot.topology_backend.identity, "topology::loopback-mesh");
assert_eq!(snapshot.transport_backend.identity, "transport::loopback-mailbox");
assert_eq!(snapshot.supervisor_backend.identity, "supervisor::in-process");
assert_eq!(snapshot.handoff_backend.identity, "handoff::in-memory");
```

**Checkpoint**: reflection is reporting the actual runtime services, not inferred labels.

## Step 4: Stop the runtime cleanly

```rust
runtime.stop()?;
let stopped = runtime.snapshot()?;
assert_eq!(stopped.runtime_state, simard::RuntimeState::Stopped);
```

After stop, the runtime remains inspectable, but it is no longer reusable.

## Summary

You now know:

- how to compose Simard without the default CLI topology shortcut
- how to run the real `rusty-clawd` backend on the loopback mesh path
- how topology, mailbox, handoff, and adapter identities appear in reflection
- why the alternate topology path lives in the in-process Rust API instead of the default CLI path

## Next steps

- Use [How to export and restore runtime handoff](../howto/export-and-restore-runtime-handoff.md) to move this runtime boundary into fresh ports.
- Use the [runtime contracts reference](../reference/runtime-contracts.md) when you need exact field names and constructor behavior.
- Read [Concept: topology-neutral runtime kernel](../concepts/topology-neutral-runtime-kernel.md) for the design rationale.
- Return to the [documentation index](../index.md) for the rest of the Simard docs.
