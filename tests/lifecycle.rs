use std::sync::Arc;

use simard::{
    BackendDescriptor, BaseTypeCapability, BaseTypeId, BaseTypeRegistry, EvidenceStore, Freshness,
    FreshnessState, IdentityManifest, InMemoryEvidenceStore, InMemoryMemoryStore,
    InMemoryPromptAssetStore, LocalProcessHarnessAdapter, LocalRuntime, ManifestContract,
    MemoryPolicy, MemoryScope, MemoryStore, OperatingMode, PromptAsset, PromptAssetRef, Provenance,
    ReflectiveRuntime, RuntimePorts, RuntimeRequest, RuntimeState, RuntimeTopology, SessionPhase,
    SimardError, UuidSessionIdGenerator, capability_set,
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
        ManifestContract::new(
            simard::bootstrap_entrypoint(),
            "bootstrap-config -> identity-loader -> runtime-ports -> local-runtime",
            vec!["tests:lifecycle".to_string()],
            Provenance::new("test", "lifecycle::manifest"),
            Freshness::now().expect("freshness should be observable"),
        )
        .expect("contract should be valid"),
    )
    .expect("manifest should be valid")
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
        Freshness::now().expect("freshness should be observable"),
    )));
    let evidence = Arc::new(InMemoryEvidenceStore::new(BackendDescriptor::new(
        "evidence::append-only-log",
        Provenance::injected("test:evidence-store"),
        Freshness::now().expect("freshness should be observable"),
    )));
    let mut base_types = BaseTypeRegistry::default();
    base_types.register(
        LocalProcessHarnessAdapter::single_process("local-harness")
            .expect("adapter should initialize"),
    );

    let request = RuntimeRequest::new(
        manifest(),
        BaseTypeId::new("local-harness"),
        RuntimeTopology::SingleProcess,
    );

    let mut runtime = LocalRuntime::compose(
        RuntimePorts::new(
            prompts,
            memory.clone(),
            evidence.clone(),
            base_types,
            Arc::new(UuidSessionIdGenerator),
        ),
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
    assert_eq!(
        evidence
            .count_for_session(&outcome.session.id)
            .expect("evidence counts should be queryable"),
        2
    );
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
    assert_eq!(
        memory
            .count_for_session(&outcome.session.id)
            .expect("memory counts should be queryable"),
        2
    );

    let snapshot = runtime.snapshot().expect("snapshot should succeed");
    assert_eq!(snapshot.runtime_state, RuntimeState::Ready);
    assert_eq!(snapshot.session_phase, Some(SessionPhase::Complete));
    assert_eq!(snapshot.evidence_records, 2);
    assert_eq!(snapshot.memory_records, 2);
    assert_eq!(snapshot.adapter_backend.identity, "local-harness");
    assert!(
        snapshot
            .adapter_backend
            .provenance
            .locator
            .contains("local-harness"),
        "adapter backend should come from the runtime-selected adapter descriptor"
    );
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
    assert!(
        snapshot.manifest_contract.entrypoint.contains("bootstrap"),
        "reflection should report the real bootstrap entrypoint instead of placeholder metadata"
    );
    assert_ne!(
        snapshot.manifest_contract.entrypoint, "inline-manifest",
        "inline placeholder entrypoints hide the real runtime assembly boundary"
    );
    assert_ne!(
        snapshot.manifest_contract.provenance.source, "inline",
        "reflection provenance should describe the true manifest source"
    );
    assert_eq!(
        snapshot.manifest_contract.freshness.state,
        FreshnessState::Current
    );

    runtime
        .stop()
        .expect("stop should succeed when runtime is idle");
    let stopped = runtime.snapshot().expect("snapshot should still work");
    assert_eq!(stopped.runtime_state, RuntimeState::Stopped);
    assert_eq!(
        stopped.manifest_contract.freshness.state,
        FreshnessState::Stale
    );

    let error = runtime.run("should fail after stop").unwrap_err();
    assert_eq!(
        error,
        SimardError::RuntimeStopped {
            action: "run".to_string(),
        }
    );
}

#[test]
fn stopped_runtime_surfaces_dedicated_lifecycle_errors() {
    let prompts = Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
        "engineer-system",
        "simard/engineer_system.md",
        "You are Simard.",
    )]));
    let memory = Arc::new(InMemoryMemoryStore::try_default().expect("store should initialize"));
    let evidence = Arc::new(InMemoryEvidenceStore::try_default().expect("store should initialize"));
    let mut base_types = BaseTypeRegistry::default();
    base_types.register(
        LocalProcessHarnessAdapter::single_process("local-harness")
            .expect("adapter should initialize"),
    );

    let request = RuntimeRequest::new(
        manifest(),
        BaseTypeId::new("local-harness"),
        RuntimeTopology::SingleProcess,
    );

    let mut runtime = LocalRuntime::compose(
        RuntimePorts::new(
            prompts,
            memory,
            evidence,
            base_types,
            Arc::new(UuidSessionIdGenerator),
        ),
        request,
    )
    .expect("composition should succeed");

    runtime.start().expect("startup should succeed");
    runtime
        .run("exercise stop semantics")
        .expect("run should succeed before shutdown");
    runtime.stop().expect("first stop should succeed");

    let start_error = runtime.start().unwrap_err();
    assert_eq!(
        start_error,
        SimardError::RuntimeStopped {
            action: "start".to_string(),
        }
    );

    let run_error = runtime.run("after stop").unwrap_err();
    assert_eq!(
        run_error,
        SimardError::RuntimeStopped {
            action: "run".to_string(),
        }
    );

    let stop_error = runtime.stop().unwrap_err();
    assert_eq!(
        stop_error,
        SimardError::RuntimeStopped {
            action: "stop".to_string(),
        }
    );
}

#[test]
fn runtime_can_stop_before_start_and_preserve_a_stale_snapshot() {
    let prompts = Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
        "engineer-system",
        "simard/engineer_system.md",
        "You are Simard.",
    )]));
    let memory = Arc::new(InMemoryMemoryStore::try_default().expect("store should initialize"));
    let evidence = Arc::new(InMemoryEvidenceStore::try_default().expect("store should initialize"));
    let mut base_types = BaseTypeRegistry::default();
    base_types.register(
        LocalProcessHarnessAdapter::single_process("local-harness")
            .expect("adapter should initialize"),
    );

    let request = RuntimeRequest::new(
        manifest(),
        BaseTypeId::new("local-harness"),
        RuntimeTopology::SingleProcess,
    );

    let mut runtime = LocalRuntime::compose(
        RuntimePorts::new(
            prompts,
            memory,
            evidence,
            base_types,
            Arc::new(UuidSessionIdGenerator),
        ),
        request,
    )
    .expect("composition should succeed");

    runtime
        .stop()
        .expect("stopping before start should still be a valid lifecycle boundary");

    let snapshot = runtime
        .snapshot()
        .expect("snapshot should remain available");
    assert_eq!(snapshot.runtime_state, RuntimeState::Stopped);
    assert_eq!(
        snapshot.manifest_contract.freshness.state,
        FreshnessState::Stale
    );
}
