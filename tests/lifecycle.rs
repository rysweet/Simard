use std::sync::Arc;

use simard::{
    BackendDescriptor, BaseTypeCapability, BaseTypeId, BaseTypeRegistry, EvidenceStore, Freshness,
    IdentityManifest, InMemoryEvidenceStore, InMemoryMemoryStore, InMemoryPromptAssetStore,
    LocalProcessHarnessAdapter, LocalRuntime, MemoryPolicy, MemoryScope, MemoryStore,
    OperatingMode, PromptAsset, PromptAssetRef, Provenance, ReflectiveRuntime, RuntimePorts,
    RuntimeRequest, RuntimeState, RuntimeTopology, SessionPhase, SimardError, capability_set,
};

fn manifest() -> IdentityManifest {
    IdentityManifest::new(
        "simard-engineer",
        "0.1.0",
        vec![PromptAssetRef::new(
            "engineer-system",
            "simard/engineer_system.md",
        )],
        vec![
            BaseTypeId::new("local-harness"),
            BaseTypeId::new("future-distributed-adapter"),
        ],
        capability_set([
            BaseTypeCapability::PromptAssets,
            BaseTypeCapability::SessionLifecycle,
            BaseTypeCapability::Memory,
            BaseTypeCapability::Evidence,
            BaseTypeCapability::Reflection,
        ]),
        OperatingMode::Engineer,
        MemoryPolicy::default(),
    )
}

#[test]
fn local_runtime_runs_session_and_persists_boundaries() {
    let prompts = Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
        "engineer-system",
        "simard/engineer_system.md",
        "You are Simard.",
    )]));
    let memory = Arc::new(InMemoryMemoryStore::new(BackendDescriptor::new(
        "memory::session-cache",
        Provenance::injected("test:memory-store"),
        Freshness::now(),
    )));
    let evidence = Arc::new(InMemoryEvidenceStore::new(BackendDescriptor::new(
        "evidence::append-only-log",
        Provenance::injected("test:evidence-store"),
        Freshness::now(),
    )));
    let mut base_types = BaseTypeRegistry::default();
    base_types.register(LocalProcessHarnessAdapter::single_process("local-harness"));

    let request = RuntimeRequest::new(
        manifest(),
        BaseTypeId::new("local-harness"),
        RuntimeTopology::SingleProcess,
    );

    let mut runtime = LocalRuntime::compose(
        RuntimePorts::new(prompts, memory.clone(), evidence.clone(), base_types),
        request,
    )
    .expect("composition should succeed");

    runtime.start().expect("startup should load prompt assets");
    let outcome = runtime
        .run("exercise the local runtime")
        .expect("run should succeed");

    assert_eq!(runtime.state(), RuntimeState::Ready);
    assert_eq!(outcome.session.phase, SessionPhase::Complete);
    assert_eq!(
        outcome.reflection.snapshot.runtime_state,
        RuntimeState::Reflecting
    );
    assert_eq!(
        outcome.reflection.snapshot.topology,
        RuntimeTopology::SingleProcess
    );
    assert_eq!(
        outcome.reflection.snapshot.selected_base_type,
        BaseTypeId::new("local-harness")
    );

    let evidence_records = evidence
        .list_for_session(&outcome.session.id)
        .expect("evidence should be queryable");
    assert_eq!(evidence_records.len(), 2);
    assert!(
        evidence_records
            .iter()
            .all(|record| record.phase == SessionPhase::Execution)
    );

    let scratch_records = memory
        .list(MemoryScope::SessionScratch)
        .expect("scratch memory should be queryable");
    assert_eq!(scratch_records.len(), 1);
    assert_eq!(scratch_records[0].recorded_in, SessionPhase::Preparation);

    let summary_records = memory
        .list(MemoryScope::SessionSummary)
        .expect("summary memory should be queryable");
    assert_eq!(summary_records.len(), 1);
    assert_eq!(summary_records[0].recorded_in, SessionPhase::Persistence);

    let snapshot = runtime.snapshot().expect("snapshot should succeed");
    assert_eq!(snapshot.runtime_state, RuntimeState::Ready);
    assert_eq!(snapshot.session_phase, Some(SessionPhase::Complete));
    assert_eq!(snapshot.evidence_records, 2);
    assert_eq!(snapshot.memory_records, 2);
    assert_eq!(snapshot.memory_backend.identity, "memory::session-cache");
    assert_eq!(
        snapshot.memory_backend.provenance,
        Provenance::injected("test:memory-store")
    );
    assert_eq!(
        snapshot.evidence_backend.identity,
        "evidence::append-only-log"
    );
    assert_eq!(
        snapshot.evidence_backend.provenance,
        Provenance::injected("test:evidence-store")
    );
    assert_eq!(snapshot.manifest_contract.entrypoint, "inline-manifest");
    assert_eq!(
        snapshot.manifest_provenance,
        Provenance::new("inline", "identity:simard-engineer")
    );

    runtime
        .stop()
        .expect("stop should succeed when runtime is idle");
    let stopped = runtime.snapshot().expect("snapshot should still work");
    assert_eq!(stopped.runtime_state, RuntimeState::Stopped);

    let error = runtime.run("should fail after stop").unwrap_err();
    assert_eq!(
        error,
        SimardError::InvalidRuntimeTransition {
            from: RuntimeState::Stopped,
            to: RuntimeState::Active,
        }
    );
}
