---
title: "How to export and restore runtime handoff"
description: Export the latest Simard session boundary and restore it into fresh runtime ports without losing memory, evidence, or truthful reflection.
last_updated: 2026-03-29
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../index.md
  - ../reference/runtime-contracts.md
  - ../tutorials/run-rusty-clawd-on-loopback-mesh.md
  - ../concepts/topology-neutral-runtime-kernel.md
---

# How to export and restore runtime handoff

Use this guide when you need to carry the latest Simard session boundary into a fresh runtime instance.

## Prerequisites

- [ ] You are working in-process with the Rust API, not only through `cargo run --quiet`
- [ ] You can already compose a runtime with the selected identity, base type, and topology
- [ ] You understand that exported handoff payloads are sensitive, and that exported session-objective redaction is planned rather than shipped today

## Steps

### 1. Compose a source runtime with an explicit handoff store

Handoff is part of the runtime kernel, so inject it at composition time.

```rust
use std::sync::Arc;

use simard::{
    BaseTypeCapability, BaseTypeId, BaseTypeRegistry, Freshness, IdentityManifest,
    InMemoryEvidenceStore, InMemoryHandoffStore, InMemoryMemoryStore, InMemoryPromptAssetStore,
    InProcessSupervisor, LocalRuntime, LoopbackMailboxTransport, LoopbackMeshTopologyDriver,
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
            vec!["docs:handoff-howto".to_string()],
            Provenance::new("docs", "howto::handoff"),
            Freshness::now()?,
        )?,
    )?,
    BaseTypeId::new("rusty-clawd"),
    RuntimeTopology::MultiProcess,
);

let mut runtime = LocalRuntime::compose(
    RuntimePorts::with_runtime_services_and_program(
        prompts,
        memory.clone(),
        evidence.clone(),
        base_types,
        Arc::new(LoopbackMeshTopologyDriver::try_default()?),
        Arc::new(LoopbackMailboxTransport::try_default()?),
        Arc::new(InProcessSupervisor::try_default()?),
        Arc::new(ObjectiveRelayProgram::try_default()?),
        handoff.clone(),
        Arc::new(UuidSessionIdGenerator),
    ),
    request.clone(),
)?;
```

### 2. Run a session and export the snapshot

```rust
runtime.start()?;
runtime.run("handoff the current session boundary")?;
let snapshot = runtime.export_handoff()?;
```

Check these facts immediately:

```rust
assert_eq!(snapshot.identity_name, "simard-engineer");
assert_eq!(snapshot.selected_base_type, BaseTypeId::new("rusty-clawd"));
assert_eq!(snapshot.topology, RuntimeTopology::MultiProcess);
assert_eq!(snapshot.source_runtime_node.to_string(), "node-loopback-mesh");
assert_eq!(snapshot.source_mailbox_address.to_string(), "loopback://node-loopback-mesh");
assert!(snapshot.session.is_some());
assert!(!snapshot.memory_records.is_empty());
assert!(!snapshot.evidence_records.is_empty());
```

### 3. Treat the snapshot as sensitive, not as disposable debug text

A handoff snapshot is not disposable debug output.

Today, `export_handoff()` clones the current `SessionRecord` into `RuntimeHandoffSnapshot`. That means `snapshot.session` can still carry the original objective text.

[PLANNED] handoff hardening will narrow exported session text to `objective-metadata(...)`, but until that lands you should treat the whole snapshot as a fully sensitive runtime artifact.

The exported payload includes:

- session identifiers and phase data
- selected base type and topology
- runtime-node and mailbox-address metadata
- memory records
- evidence records
- the current stored session boundary, including the objective string

### 4. Restore into fresh runtime ports

Use fresh memory, evidence, and handoff stores for the destination runtime. Restore does not mutate the source runtime in place.

```rust
let restored_memory = Arc::new(InMemoryMemoryStore::try_default()?);
let restored_evidence = Arc::new(InMemoryEvidenceStore::try_default()?);
let restored_handoff = Arc::new(InMemoryHandoffStore::try_default()?);
let mut restored_base_types = BaseTypeRegistry::default();
restored_base_types.register(RustyClawdAdapter::registered("rusty-clawd")?);

let restored = LocalRuntime::compose_from_handoff(
    RuntimePorts::with_runtime_services_and_program(
        Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
            "engineer-system",
            "simard/engineer_system.md",
            "You are Simard.",
        )])),
        restored_memory.clone(),
        restored_evidence.clone(),
        restored_base_types,
        Arc::new(LoopbackMeshTopologyDriver::try_default()?),
        Arc::new(LoopbackMailboxTransport::try_default()?),
        Arc::new(InProcessSupervisor::try_default()?),
        Arc::new(ObjectiveRelayProgram::try_default()?),
        restored_handoff.clone(),
        Arc::new(UuidSessionIdGenerator),
    ),
    request,
    snapshot,
)?;
```

### 5. Verify the restored runtime before you start it

A restored runtime should already expose the carried-over session boundary, but it stays in `Initializing` until you call `start()`.

```rust
let restored_snapshot = restored.snapshot()?;

assert_eq!(restored_snapshot.runtime_state, simard::RuntimeState::Initializing);
assert_eq!(restored_snapshot.session_phase, Some(simard::SessionPhase::Complete));
assert_eq!(restored_snapshot.memory_records, 2);
assert_eq!(restored_snapshot.evidence_records, 5);
assert_eq!(restored_snapshot.handoff_backend.identity, "handoff::in-memory");
```

## Variations

### For a single-process local runtime

Use `RuntimePorts::new(...)` or `RuntimePorts::with_session_ids(...)`, keep `RuntimeTopology::SingleProcess`, and register `local-harness` or `rusty-clawd` for single-process execution.

### For a custom handoff store

Inject your own `RuntimeHandoffStore` through `RuntimePorts::with_runtime_services_and_program(...)`. Reflection will report its `handoff_backend` descriptor directly.

## Troubleshooting

### `InvalidHandoffSnapshot`

**Symptom**: restore fails before composition completes.

**Cause**: `snapshot.identity_name` or `snapshot.selected_base_type` does not match the destination `RuntimeRequest`.

**Fix**: restore with the same identity/base-type pair that produced the snapshot.

### Snapshot topology does not match the restore request

**Symptom**: you expected restore to reject a topology mismatch, but `compose_from_handoff(...)` did not fail at the handoff-validation step.

**Cause**: current restore validates identity and selected base type, then composes with the destination request. It does not yet enforce `snapshot.topology == request.topology`.

**Fix**: compare `snapshot.topology` with the destination request in caller code until the kernel adds topology-equality enforcement.

### `UnsupportedRuntimeTopology` or `UnsupportedTopology`

**Symptom**: restore fails even though the handoff snapshot looks valid.

**Cause**: the destination topology driver or base-type adapter does not support the requested topology.

**Fix**: inject runtime services and a base-type backend that support the topology you are restoring into. In this repository, end-to-end examples cover `single-process` and `multi-process`; `distributed` still needs a compatible injected base type/runtime combination.

### Restored runtime looks empty

**Symptom**: `snapshot()` works, but `memory_records` or `evidence_records` are zero.

**Cause**: the source runtime exported before any session completed, or the destination stores were not wired through `compose_from_handoff(...)`.

**Fix**: export after a successful run, and restore through fresh injected stores instead of trying to rebuild the snapshot manually.

## See also

- [Runtime contracts reference](../reference/runtime-contracts.md)
- [Tutorial: Run RustyClawd on the loopback mesh](../tutorials/run-rusty-clawd-on-loopback-mesh.md)
- [Concept: topology-neutral runtime kernel](../concepts/topology-neutral-runtime-kernel.md)
- [Documentation index](../index.md)
