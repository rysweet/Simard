use std::sync::Arc;

use simard::{
    BaseTypeCapability, BaseTypeId, BaseTypeRegistry, Freshness, IdentityManifest,
    InMemoryEvidenceStore, InMemoryGoalStore, InMemoryHandoffStore, InMemoryMailboxTransport,
    InMemoryMemoryStore, InMemoryPromptAssetStore, InProcessSupervisor, InProcessTopologyDriver,
    LocalRuntime, ManifestContract, MemoryPolicy, ObjectiveRelayProgram, OperatingMode,
    PromptAsset, PromptAssetRef, Provenance, RuntimeHandoffStore, RuntimePorts, RuntimeRequest,
    RuntimeTopology, SessionPhase, TestAdapter, UuidSessionIdGenerator, bootstrap_entrypoint,
    capability_set,
};

fn manifest() -> IdentityManifest {
    IdentityManifest::new(
        "simard-engineer",
        "0.1.0",
        vec![PromptAssetRef::new(
            "engineer-system",
            "simard/engineer_system.md",
        )],
        vec![BaseTypeId::new("local-harness")],
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
            bootstrap_entrypoint(),
            "bootstrap-config -> identity-loader -> runtime-ports -> local-runtime",
            vec!["tests:handoff-redaction".to_string()],
            Provenance::new("test", "handoff_redaction::manifest"),
            Freshness::now().expect("freshness should be observable"),
        )
        .expect("contract should be valid"),
    )
    .expect("manifest should be valid")
}

fn expected_objective_metadata(objective: &str) -> String {
    let chars = objective.chars().count();
    let words = objective.split_whitespace().count();
    let lines = if objective.is_empty() {
        0
    } else {
        objective.lines().count()
    };

    format!("objective-metadata(chars={chars}, words={words}, lines={lines})")
}

fn compose_runtime(handoff: Arc<InMemoryHandoffStore>) -> LocalRuntime {
    let prompts = Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
        "engineer-system",
        "simard/engineer_system.md",
        "You are Simard.",
    )]));
    let memory =
        Arc::new(InMemoryMemoryStore::try_default().expect("memory store should initialize"));
    let evidence =
        Arc::new(InMemoryEvidenceStore::try_default().expect("evidence store should initialize"));
    let mut base_types = BaseTypeRegistry::default();
    base_types
        .register(TestAdapter::single_process("local-harness").expect("adapter should initialize"));

    LocalRuntime::compose(
        RuntimePorts::with_runtime_services_and_program(
            prompts,
            memory,
            evidence,
            Arc::new(InMemoryGoalStore::try_default().expect("goal store should initialize")),
            base_types,
            Arc::new(InProcessTopologyDriver::try_default().expect("driver should initialize")),
            Arc::new(InMemoryMailboxTransport::try_default().expect("transport should initialize")),
            Arc::new(InProcessSupervisor::try_default().expect("supervisor should initialize")),
            Arc::new(
                ObjectiveRelayProgram::try_default().expect("agent program should initialize"),
            ),
            handoff,
            Arc::new(UuidSessionIdGenerator),
        ),
        RuntimeRequest::new(
            manifest(),
            BaseTypeId::new("local-harness"),
            RuntimeTopology::SingleProcess,
        ),
    )
    .expect("runtime should compose")
}

#[test]
fn export_handoff_redacts_session_objective_before_persisting_snapshot() {
    let handoff = Arc::new(InMemoryHandoffStore::try_default().expect("handoff should initialize"));
    let mut runtime = compose_runtime(handoff.clone());
    let objective = "ship the runtime handoff without leaking raw objectives";

    runtime.start().expect("runtime should start");
    let outcome = runtime.run(objective).expect("run should succeed");
    let snapshot = runtime.export_handoff().expect("handoff should export");

    let exported_session = snapshot
        .session
        .as_ref()
        .expect("handoff should preserve the last session boundary");
    assert_eq!(exported_session.id, outcome.session.id);
    assert_eq!(exported_session.phase, SessionPhase::Complete);
    assert_eq!(
        exported_session.objective,
        expected_objective_metadata(objective),
        "handoff exports must redact the persisted session objective"
    );
    assert_ne!(exported_session.objective, objective);
    assert_eq!(
        handoff.latest().expect("handoff latest should work"),
        Some(snapshot)
    );
}

#[test]
fn restored_handoff_keeps_only_redacted_session_objective() {
    let handoff = Arc::new(InMemoryHandoffStore::try_default().expect("handoff should initialize"));
    let mut runtime = compose_runtime(handoff);
    let objective = "restore from handoff without reviving the raw objective";

    runtime.start().expect("runtime should start");
    runtime.run(objective).expect("run should succeed");
    let snapshot = runtime.export_handoff().expect("handoff should export");

    let restored_handoff =
        Arc::new(InMemoryHandoffStore::try_default().expect("handoff should initialize"));
    let restored = LocalRuntime::compose_from_handoff(
        RuntimePorts::with_runtime_services_and_program(
            Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
                "engineer-system",
                "simard/engineer_system.md",
                "You are Simard.",
            )])),
            Arc::new(InMemoryMemoryStore::try_default().expect("memory store should initialize")),
            Arc::new(
                InMemoryEvidenceStore::try_default().expect("evidence store should initialize"),
            ),
            Arc::new(InMemoryGoalStore::try_default().expect("goal store should initialize")),
            {
                let mut base_types = BaseTypeRegistry::default();
                base_types.register(
                    TestAdapter::single_process("local-harness")
                        .expect("adapter should initialize"),
                );
                base_types
            },
            Arc::new(InProcessTopologyDriver::try_default().expect("driver should initialize")),
            Arc::new(InMemoryMailboxTransport::try_default().expect("transport should initialize")),
            Arc::new(InProcessSupervisor::try_default().expect("supervisor should initialize")),
            Arc::new(
                ObjectiveRelayProgram::try_default().expect("agent program should initialize"),
            ),
            restored_handoff.clone(),
            Arc::new(UuidSessionIdGenerator),
        ),
        RuntimeRequest::new(
            manifest(),
            BaseTypeId::new("local-harness"),
            RuntimeTopology::SingleProcess,
        ),
        snapshot,
    )
    .expect("restored runtime should compose");

    let restored_snapshot = restored
        .export_handoff()
        .expect("restored handoff should export");
    let restored_session = restored_snapshot
        .session
        .as_ref()
        .expect("restored handoff should preserve the session boundary");
    assert_eq!(
        restored_session.objective,
        expected_objective_metadata(objective),
        "restored handoff should not reintroduce the raw objective"
    );
    assert_eq!(
        restored_handoff
            .latest()
            .expect("handoff latest should work"),
        Some(restored_snapshot)
    );
}
