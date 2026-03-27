use std::sync::Arc;

use simard::{
    BaseTypeCapability, BaseTypeId, BaseTypeRegistry, IdentityManifest, InMemoryEvidenceStore,
    InMemoryMemoryStore, InMemoryPromptAssetStore, LocalProcessHarnessAdapter, LocalRuntime,
    MemoryPolicy, OperatingMode, PromptAsset, PromptAssetRef, RuntimePorts, RuntimeRequest,
    RuntimeTopology, SessionIdGenerator, SessionPhase, SessionRecord, SimardError,
    UuidSessionIdGenerator, capability_set,
};

fn prompt_store() -> Arc<InMemoryPromptAssetStore> {
    Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
        "engineer-system",
        "simard/engineer_system.md",
        "You are Simard.",
    )]))
}

fn manifest(base_type: &str) -> IdentityManifest {
    IdentityManifest::new(
        "simard-engineer",
        "0.1.0",
        vec![PromptAssetRef::new(
            "engineer-system",
            "simard/engineer_system.md",
        )],
        vec![
            BaseTypeId::new(base_type),
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
fn compose_rejects_missing_capability() {
    let prompts = prompt_store();
    let memory = Arc::new(InMemoryMemoryStore::default());
    let evidence = Arc::new(InMemoryEvidenceStore::default());
    let mut base_types = BaseTypeRegistry::default();
    base_types.register(LocalProcessHarnessAdapter::new(
        "limited-harness",
        [
            BaseTypeCapability::PromptAssets,
            BaseTypeCapability::SessionLifecycle,
        ],
        [RuntimeTopology::SingleProcess],
    ));

    let request = RuntimeRequest::new(
        manifest("limited-harness"),
        BaseTypeId::new("limited-harness"),
        RuntimeTopology::SingleProcess,
    );

    let error = match LocalRuntime::compose(
        RuntimePorts::new(prompts, memory, evidence, base_types),
        request,
    ) {
        Ok(_) => panic!("composition should have failed"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        SimardError::MissingCapability {
            base_type: "limited-harness".to_string(),
            capability: BaseTypeCapability::Memory,
        }
    );
}

#[test]
fn start_rejects_missing_prompt_asset() {
    let prompts = Arc::new(InMemoryPromptAssetStore::default());
    let memory = Arc::new(InMemoryMemoryStore::default());
    let evidence = Arc::new(InMemoryEvidenceStore::default());
    let mut base_types = BaseTypeRegistry::default();
    base_types.register(LocalProcessHarnessAdapter::single_process("local-harness"));

    let request = RuntimeRequest::new(
        manifest("local-harness"),
        BaseTypeId::new("local-harness"),
        RuntimeTopology::SingleProcess,
    );

    let mut runtime = LocalRuntime::compose(
        RuntimePorts::new(prompts, memory, evidence, base_types),
        request,
    )
    .expect("composition should succeed before prompt loading");

    let error = runtime.start().unwrap_err();

    assert_eq!(
        error,
        SimardError::PromptAssetMissing {
            asset_id: "engineer-system".to_string(),
            path: "simard/engineer_system.md".into(),
        }
    );
}

#[test]
fn session_phase_rejects_skipped_transition() {
    let mut session = SessionRecord::new(
        OperatingMode::Engineer,
        "validate transitions",
        BaseTypeId::new("local-harness"),
        &UuidSessionIdGenerator,
    );

    let error = session.advance(SessionPhase::Execution).unwrap_err();

    assert_eq!(
        error,
        SimardError::InvalidSessionTransition {
            from: SessionPhase::Intake,
            to: SessionPhase::Execution,
        }
    );
}

#[derive(Debug)]
struct FixedSessionIds;

impl SessionIdGenerator for FixedSessionIds {
    fn next_id(&self) -> simard::SessionId {
        simard::SessionId::new("session-fixed")
    }
}

#[test]
fn runtime_uses_injected_session_id_strategy() {
    let prompts = prompt_store();
    let memory = Arc::new(InMemoryMemoryStore::default());
    let evidence = Arc::new(InMemoryEvidenceStore::default());
    let mut base_types = BaseTypeRegistry::default();
    base_types.register(LocalProcessHarnessAdapter::single_process("local-harness"));

    let request = RuntimeRequest::new(
        manifest("local-harness"),
        BaseTypeId::new("local-harness"),
        RuntimeTopology::SingleProcess,
    );

    let mut runtime = LocalRuntime::compose(
        RuntimePorts::with_session_ids(
            prompts,
            memory,
            evidence,
            base_types,
            Arc::new(FixedSessionIds),
        ),
        request,
    )
    .expect("composition should succeed");

    runtime.start().expect("startup should succeed");
    let outcome = runtime.run("inject ids").expect("run should succeed");

    assert_eq!(outcome.session.id.to_string(), "session-fixed");
}
