use std::sync::Arc;

use simard::{
    BaseTypeCapability, BaseTypeId, BaseTypeRegistry, IdentityManifest, InMemoryEvidenceStore,
    InMemoryMemoryStore, InMemoryPromptAssetStore, LocalProcessHarnessAdapter, LocalRuntime,
    ManifestContract, MemoryPolicy, MemoryScope, OperatingMode, PromptAsset, PromptAssetRef,
    Provenance, RuntimePorts, RuntimeRequest, RuntimeTopology, SessionId, SessionIdGenerator,
    SessionPhase, SessionRecord, SimardError, UuidSessionIdGenerator, capability_set,
};
use uuid::Uuid;

fn prompt_store() -> Arc<InMemoryPromptAssetStore> {
    Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
        "engineer-system",
        "simard/engineer_system.md",
        "You are Simard.",
    )]))
}

fn manifest(base_type: &str) -> IdentityManifest {
    manifest_with_policy(base_type, MemoryPolicy::default())
}

fn manifest_with_policy(base_type: &str, memory_policy: MemoryPolicy) -> IdentityManifest {
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
        memory_policy,
        ManifestContract::new(
            simard::bootstrap_entrypoint(),
            "bootstrap-config -> identity-loader -> runtime-ports -> local-runtime",
            vec!["tests:contracts".to_string()],
            Provenance::new("test", "contracts::manifest"),
            simard::Freshness::now().expect("freshness should be observable"),
        )
        .expect("contract should be valid"),
    )
    .expect("manifest should be valid")
}

#[test]
fn compose_rejects_missing_capability() {
    let prompts = prompt_store();
    let memory = Arc::new(InMemoryMemoryStore::try_default().expect("store should initialize"));
    let evidence = Arc::new(InMemoryEvidenceStore::try_default().expect("store should initialize"));
    let mut base_types = BaseTypeRegistry::default();
    base_types.register(
        LocalProcessHarnessAdapter::new(
            "limited-harness",
            "limited-harness",
            [
                BaseTypeCapability::PromptAssets,
                BaseTypeCapability::SessionLifecycle,
            ],
            [RuntimeTopology::SingleProcess],
        )
        .expect("adapter should initialize"),
    );

    let request = RuntimeRequest::new(
        manifest("limited-harness"),
        BaseTypeId::new("limited-harness"),
        RuntimeTopology::SingleProcess,
    );

    let error = match LocalRuntime::compose(
        RuntimePorts::new(
            prompts,
            memory,
            evidence,
            base_types,
            Arc::new(UuidSessionIdGenerator),
        ),
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
    let memory = Arc::new(InMemoryMemoryStore::try_default().expect("store should initialize"));
    let evidence = Arc::new(InMemoryEvidenceStore::try_default().expect("store should initialize"));
    let mut base_types = BaseTypeRegistry::default();
    base_types.register(
        LocalProcessHarnessAdapter::single_process("local-harness")
            .expect("adapter should initialize"),
    );

    let request = RuntimeRequest::new(
        manifest("local-harness"),
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
fn prompt_asset_error_display_redacts_absolute_paths() {
    let missing = SimardError::PromptAssetMissing {
        asset_id: "engineer-system".to_string(),
        path: "/tmp/private/prompt.md".into(),
    };
    assert!(!missing.to_string().contains("/tmp/private/prompt.md"));

    let invalid = SimardError::InvalidPromptAssetPath {
        asset_id: "engineer-system".to_string(),
        path: "/tmp/private/prompt.md".into(),
        reason: "expected a relative path inside the configured prompt root".to_string(),
    };
    assert!(!invalid.to_string().contains("/tmp/private/prompt.md"));
}

#[test]
fn compose_rejects_manifest_supported_base_types_without_registered_adapters() {
    let prompts = prompt_store();
    let memory = Arc::new(InMemoryMemoryStore::try_default().expect("store should initialize"));
    let evidence = Arc::new(InMemoryEvidenceStore::try_default().expect("store should initialize"));
    let mut base_types = BaseTypeRegistry::default();
    base_types.register(
        LocalProcessHarnessAdapter::single_process("local-harness")
            .expect("adapter should initialize"),
    );

    let request = RuntimeRequest::new(
        manifest("future-distributed-adapter"),
        BaseTypeId::new("future-distributed-adapter"),
        RuntimeTopology::SingleProcess,
    );

    let error = match LocalRuntime::compose(
        RuntimePorts::new(
            prompts,
            memory,
            evidence,
            base_types,
            Arc::new(UuidSessionIdGenerator),
        ),
        request,
    ) {
        Ok(_) => panic!("composition should have failed"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        SimardError::AdapterNotRegistered {
            base_type: "future-distributed-adapter".to_string(),
        }
    );
}

#[test]
fn manifest_rejects_project_write_policy_in_v1() {
    let error = IdentityManifest::new(
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
        MemoryPolicy {
            allow_project_writes: true,
            summary_scope: MemoryScope::SessionSummary,
        },
        ManifestContract::new(
            simard::bootstrap_entrypoint(),
            "bootstrap-config -> identity-loader -> runtime-ports -> local-runtime",
            vec!["tests:contracts".to_string()],
            Provenance::new("test", "contracts::project-writes"),
            simard::Freshness::now().expect("freshness should be observable"),
        )
        .expect("contract should be valid"),
    )
    .expect_err("v1 should reject project-write policies");

    assert_eq!(
        error,
        SimardError::UnsupportedMemoryPolicy {
            field: "memory_policy.allow_project_writes".to_string(),
            reason: "v1 only supports read-only project boundaries".to_string(),
        }
    );
}

#[test]
fn runtime_compose_rejects_project_write_policy_even_if_manifest_is_mutated() {
    let prompts = prompt_store();
    let memory = Arc::new(InMemoryMemoryStore::try_default().expect("store should initialize"));
    let evidence = Arc::new(InMemoryEvidenceStore::try_default().expect("store should initialize"));
    let mut base_types = BaseTypeRegistry::default();
    base_types.register(
        LocalProcessHarnessAdapter::single_process("local-harness")
            .expect("adapter should initialize"),
    );

    let mut mutated_manifest = manifest("local-harness");
    mutated_manifest.memory_policy = MemoryPolicy {
        allow_project_writes: true,
        summary_scope: MemoryScope::SessionSummary,
    };

    let request = RuntimeRequest::new(
        mutated_manifest,
        BaseTypeId::new("local-harness"),
        RuntimeTopology::SingleProcess,
    );

    let error = match LocalRuntime::compose(
        RuntimePorts::new(
            prompts,
            memory,
            evidence,
            base_types,
            Arc::new(UuidSessionIdGenerator),
        ),
        request,
    ) {
        Ok(_) => panic!("runtime composition should reject unsupported memory policies"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        SimardError::UnsupportedMemoryPolicy {
            field: "memory_policy.allow_project_writes".to_string(),
            reason: "v1 only supports read-only project boundaries".to_string(),
        }
    );
}

#[test]
fn session_phase_rejects_skipped_transition() {
    let mut session = SessionRecord::new(
        OperatingMode::Engineer,
        "validate transitions",
        BaseTypeId::new("local-harness"),
        &simard::UuidSessionIdGenerator,
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
    fn next_id(&self) -> SessionId {
        SessionId::parse("session-018f1f85-86f4-7ef8-9d4d-69a79d7ddf85")
            .expect("fixed session id should be valid")
    }
}

#[test]
fn runtime_uses_injected_session_id_strategy() {
    let prompts = prompt_store();
    let memory = Arc::new(InMemoryMemoryStore::try_default().expect("store should initialize"));
    let evidence = Arc::new(InMemoryEvidenceStore::try_default().expect("store should initialize"));
    let mut base_types = BaseTypeRegistry::default();
    base_types.register(
        LocalProcessHarnessAdapter::single_process("local-harness")
            .expect("adapter should initialize"),
    );

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

    assert_eq!(
        outcome.session.id.to_string(),
        "session-018f1f85-86f4-7ef8-9d4d-69a79d7ddf85"
    );
}

#[test]
fn session_id_parsing_rejects_non_uuid_values() {
    let error = SessionId::parse("session-fixed").unwrap_err();

    assert!(matches!(error, SimardError::InvalidSessionId { .. }));
}

#[test]
fn session_ids_are_not_exposed_as_open_string_wrappers() {
    let session_rs = include_str!("../src/session.rs");

    assert!(
        !session_rs.contains("pub fn new(value: impl Into<String>) -> Self"),
        "distributed-safe session ids should not expose an unchecked public string constructor"
    );
}

#[test]
fn reflection_snapshot_exposes_resolved_adapter_backend_descriptor() {
    let reflection_rs = include_str!("../src/reflection.rs");

    assert!(
        reflection_rs.contains("pub adapter_backend: BackendDescriptor"),
        "reflection snapshots need an adapter_backend descriptor so backend identity comes from the runtime-selected adapter"
    );
}

#[test]
fn reflection_snapshot_exposes_agent_program_backend_descriptor() {
    let reflection_rs = include_str!("../src/reflection.rs");

    assert!(
        reflection_rs.contains("pub agent_program_backend: BackendDescriptor"),
        "reflection snapshots need an agent_program_backend descriptor so agent logic wiring stays inspectable"
    );
}

#[test]
fn reflection_snapshot_exposes_handoff_backend_descriptor() {
    let reflection_rs = include_str!("../src/reflection.rs");

    assert!(
        reflection_rs.contains("pub handoff_backend: BackendDescriptor"),
        "reflection snapshots need a handoff_backend descriptor so export/import wiring stays inspectable"
    );
}

#[test]
fn manifest_contract_carries_provenance_and_freshness_directly() {
    let identity_rs = include_str!("../src/identity.rs");
    let manifest_contract_section = identity_rs
        .split("pub struct ManifestContract")
        .nth(1)
        .expect("identity.rs should define ManifestContract");

    assert!(
        manifest_contract_section.contains("pub provenance:"),
        "ManifestContract should carry provenance directly instead of splitting truth across separate placeholder fields"
    );
    assert!(
        manifest_contract_section.contains("pub freshness:"),
        "ManifestContract should carry freshness directly so callers can reason about metadata truth from one contract object"
    );
}

#[test]
fn freshness_model_tracks_state_and_not_just_observation_time() {
    let metadata_rs = include_str!("../src/metadata.rs");

    assert!(
        metadata_rs.contains("enum FreshnessState"),
        "freshness should include an explicit state such as Current or Stale"
    );
    assert!(
        metadata_rs.contains("state: FreshnessState"),
        "Freshness should carry explicit state in addition to the observed timestamp"
    );
}

#[test]
fn manifest_contract_rejects_placeholder_or_thin_fields() {
    let entrypoint_error = ManifestContract::new(
        "inline-manifest",
        "bootstrap-config -> runtime",
        vec!["mode:explicit-config".to_string()],
        Provenance::new("bootstrap", "contracts::placeholder"),
        simard::Freshness::now().expect("freshness should be observable"),
    )
    .expect_err("placeholder entrypoints should fail");
    assert_eq!(
        entrypoint_error,
        SimardError::InvalidManifestContract {
            field: "entrypoint".to_string(),
            reason: "expected a Rust-style module::function path".to_string(),
        }
    );

    let provenance_error = ManifestContract::new(
        "simard::bootstrap::assemble_local_runtime",
        "bootstrap-config -> runtime",
        vec!["mode:explicit-config".to_string()],
        Provenance::new("inline", "contracts::placeholder"),
        simard::Freshness::now().expect("freshness should be observable"),
    )
    .expect_err("placeholder provenance should fail");
    assert_eq!(
        provenance_error,
        SimardError::InvalidManifestContract {
            field: "provenance.source".to_string(),
            reason: "placeholder provenance sources are not allowed".to_string(),
        }
    );
}

#[test]
fn session_ids_can_be_canonicalized_from_uuid_strings() {
    let uuid = Uuid::parse_str("018f1f85-86f4-7ef8-9d4d-69a79d7ddf85").expect("uuid should parse");

    assert_eq!(
        SessionId::parse("018f1f85-86f4-7ef8-9d4d-69a79d7ddf85")
            .expect("bare uuid should be accepted"),
        SessionId::from_uuid(uuid)
    );
}

#[test]
fn session_id_generator_is_not_hidden_inside_runtime_ports() {
    let runtime_rs = include_str!("../src/runtime.rs");
    let bootstrap_rs = include_str!("../src/bootstrap.rs");

    assert!(
        !runtime_rs.contains("Arc::new(UuidSessionIdGenerator)"),
        "runtime ports should not silently create a process-local session id generator"
    );
    assert!(
        bootstrap_rs.contains("Arc::new(UuidSessionIdGenerator)"),
        "the local bootstrap path should opt in explicitly to the local UUID session id strategy"
    );
}
