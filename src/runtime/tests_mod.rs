use super::*;
use crate::base_types::BaseTypeId;
use crate::evidence::InMemoryEvidenceStore;
use crate::identity::{ManifestContract, MemoryPolicy, OperatingMode};
use crate::memory::InMemoryMemoryStore;
use crate::metadata::Provenance;
use crate::prompt_assets::{InMemoryPromptAssetStore, PromptAsset};
use crate::session::{SessionId, SessionIdGenerator, SessionPhase};
use crate::test_support::TestAdapter;
use std::sync::atomic::{AtomicU64, Ordering};

struct TestSessionIds(AtomicU64);

impl TestSessionIds {
    fn new() -> Self {
        Self(AtomicU64::new(1))
    }
}

impl SessionIdGenerator for TestSessionIds {
    fn next_id(&self) -> SessionId {
        let n = self.0.fetch_add(1, Ordering::Relaxed);
        SessionId::parse(format!("session-00000000-0000-0000-0000-{n:012}")).unwrap()
    }
}

fn test_contract() -> ManifestContract {
    ManifestContract::new(
        "test::entrypoint",
        "a -> b",
        vec!["key:value".to_string()],
        Provenance::new("test-source", "test-locator"),
        crate::metadata::Freshness::now().unwrap(),
    )
    .unwrap()
}

fn test_manifest() -> crate::identity::IdentityManifest {
    crate::identity::IdentityManifest::new(
        "test-identity",
        "0.1.0",
        vec![crate::prompt_assets::PromptAssetRef::new(
            "test-system",
            "test.md",
        )],
        vec![BaseTypeId::new("local-harness")],
        crate::base_types::capability_set([
            crate::base_types::BaseTypeCapability::PromptAssets,
            crate::base_types::BaseTypeCapability::SessionLifecycle,
            crate::base_types::BaseTypeCapability::Memory,
            crate::base_types::BaseTypeCapability::Evidence,
            crate::base_types::BaseTypeCapability::Reflection,
        ]),
        OperatingMode::Engineer,
        MemoryPolicy::default(),
        test_contract(),
    )
    .unwrap()
}

fn test_ports() -> RuntimePorts {
    let prompt_store = Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
        "test-system",
        "test.md",
        "You are a test system.",
    )]));
    let memory_store = Arc::new(InMemoryMemoryStore::try_default().unwrap());
    let evidence_store = Arc::new(InMemoryEvidenceStore::try_default().unwrap());
    let mut registry = BaseTypeRegistry::default();
    registry.register(TestAdapter::single_process("local-harness").unwrap());
    RuntimePorts::new(
        prompt_store,
        memory_store,
        evidence_store,
        registry,
        Arc::new(TestSessionIds::new()),
    )
    .unwrap()
}

fn test_request() -> RuntimeRequest {
    RuntimeRequest::new(
        test_manifest(),
        BaseTypeId::new("local-harness"),
        RuntimeTopology::SingleProcess,
    )
}

// --- Kernel compose tests ---

#[test]
fn compose_initializes_in_initializing_state() {
    let kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
    assert_eq!(kernel.state(), RuntimeState::Initializing);
}

#[test]
fn compose_rejects_unregistered_base_type() {
    let mut request = test_request();
    request.selected_base_type = BaseTypeId::new("unknown-adapter");
    request
        .manifest
        .supported_base_types
        .push(BaseTypeId::new("unknown-adapter"));
    let result = RuntimeKernel::compose(test_ports(), request);
    assert!(matches!(
        result,
        Err(SimardError::AdapterNotRegistered { .. })
    ));
}

#[test]
fn compose_rejects_unsupported_base_type_for_identity() {
    let mut request = test_request();
    request.selected_base_type = BaseTypeId::new("not-in-manifest");
    let result = RuntimeKernel::compose(test_ports(), request);
    assert!(matches!(
        result,
        Err(SimardError::UnsupportedBaseType { .. })
    ));
}

// --- Lifecycle tests ---

#[test]
fn start_transitions_to_ready() {
    let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
    kernel.start().unwrap();
    assert_eq!(kernel.state(), RuntimeState::Ready);
}

#[test]
fn stop_transitions_to_stopped() {
    let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
    kernel.start().unwrap();
    kernel.stop().unwrap();
    assert_eq!(kernel.state(), RuntimeState::Stopped);
}

#[test]
fn stop_on_stopped_runtime_returns_error() {
    let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
    kernel.start().unwrap();
    kernel.stop().unwrap();
    let err = kernel.stop().unwrap_err();
    assert!(matches!(err, SimardError::RuntimeStopped { .. }));
}

#[test]
fn run_on_stopped_runtime_returns_error() {
    let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
    kernel.start().unwrap();
    kernel.stop().unwrap();
    let err = kernel.run("test").unwrap_err();
    assert!(matches!(err, SimardError::RuntimeStopped { .. }));
}

// --- Session orchestration integration test ---

#[test]
fn full_session_lifecycle_produces_outcome_and_returns_to_ready() {
    let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
    kernel.start().unwrap();
    let outcome = kernel.run("Implement feature X").unwrap();
    assert_eq!(kernel.state(), RuntimeState::Ready);
    assert!(!outcome.plan.is_empty());
    assert!(!outcome.execution_summary.is_empty());
    assert!(!outcome.reflection.summary.is_empty());
    assert_eq!(outcome.session.phase, SessionPhase::Complete);
}

#[test]
fn multiple_sessions_each_return_to_ready() {
    let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
    kernel.start().unwrap();
    kernel.run("First objective").unwrap();
    assert_eq!(kernel.state(), RuntimeState::Ready);
    kernel.run("Second objective").unwrap();
    assert_eq!(kernel.state(), RuntimeState::Ready);
}

// --- RuntimePorts construction tests ---

#[test]
fn ports_new_initializes_successfully() {
    let ports = test_ports();
    // Verify core stores are wired — session_ids generates an id.
    let id = ports.session_ids.next_id();
    assert!(id.as_str().starts_with("session-"));
}

#[test]
fn ports_with_session_ids_delegates_to_new() {
    let prompt_store = Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
        "test-system",
        "test.md",
        "You are a test system.",
    )]));
    let memory_store = Arc::new(InMemoryMemoryStore::try_default().unwrap());
    let evidence_store = Arc::new(InMemoryEvidenceStore::try_default().unwrap());
    let mut registry = BaseTypeRegistry::default();
    registry.register(TestAdapter::single_process("local-harness").unwrap());
    let ports = RuntimePorts::with_session_ids(
        prompt_store,
        memory_store,
        evidence_store,
        registry,
        Arc::new(TestSessionIds::new()),
    )
    .unwrap();
    let id = ports.session_ids.next_id();
    assert!(id.as_str().starts_with("session-"));
}

#[test]
fn ports_with_runtime_services_uses_injected_stores() {
    let prompt_store = Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
        "test-system",
        "test.md",
        "content",
    )]));
    let memory_store = Arc::new(InMemoryMemoryStore::try_default().unwrap());
    let evidence_store = Arc::new(InMemoryEvidenceStore::try_default().unwrap());
    let goal_store = Arc::new(crate::goals::InMemoryGoalStore::try_default().unwrap());
    let mut registry = BaseTypeRegistry::default();
    registry.register(TestAdapter::single_process("test-bt").unwrap());
    let topology_driver = Arc::new(crate::runtime::InProcessTopologyDriver::try_default().unwrap());
    let transport = Arc::new(crate::runtime::InMemoryMailboxTransport::try_default().unwrap());
    let supervisor = Arc::new(crate::runtime::InProcessSupervisor::try_default().unwrap());

    let ports = RuntimePorts::with_runtime_services(
        prompt_store,
        memory_store,
        evidence_store,
        goal_store,
        registry,
        topology_driver,
        transport,
        supervisor,
        Arc::new(TestSessionIds::new()),
    )
    .unwrap();
    // Verify the agent_program port was auto-wired.
    let desc = ports.agent_program.descriptor();
    assert!(!desc.identity.is_empty());
}

#[test]
fn ports_with_runtime_services_and_program_sets_all_fields() {
    let prompt_store = Arc::new(InMemoryPromptAssetStore::new([]));
    let memory_store = Arc::new(InMemoryMemoryStore::try_default().unwrap());
    let evidence_store = Arc::new(InMemoryEvidenceStore::try_default().unwrap());
    let goal_store = Arc::new(crate::goals::InMemoryGoalStore::try_default().unwrap());
    let registry = BaseTypeRegistry::default();
    let topology_driver = Arc::new(crate::runtime::InProcessTopologyDriver::try_default().unwrap());
    let transport = Arc::new(crate::runtime::InMemoryMailboxTransport::try_default().unwrap());
    let supervisor = Arc::new(crate::runtime::InProcessSupervisor::try_default().unwrap());
    let agent_program =
        Arc::new(crate::agent_program::ObjectiveRelayProgram::try_default().unwrap());
    let handoff_store = Arc::new(crate::handoff::InMemoryHandoffStore::try_default().unwrap());
    let session_ids = Arc::new(TestSessionIds::new());

    let ports = RuntimePorts::with_runtime_services_and_program(
        prompt_store,
        memory_store,
        evidence_store,
        goal_store,
        registry,
        topology_driver,
        transport,
        supervisor,
        agent_program,
        handoff_store,
        session_ids,
    );
    // Verify descriptors are present on injected ports.
    assert!(!ports.topology_driver.descriptor().identity.is_empty());
    assert!(!ports.transport.descriptor().identity.is_empty());
    assert!(!ports.supervisor.descriptor().identity.is_empty());
    assert!(!ports.handoff_store.descriptor().identity.is_empty());
}

#[test]
fn ports_registered_base_types_are_accessible() {
    let ports = test_ports();
    let ids = ports.base_types.registered_ids();
    assert!(ids.iter().any(|id| id.as_str() == "local-harness"));
}

// --- RuntimeKernel session method tests ---

#[test]
fn mark_last_session_failed_sets_phase() {
    let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
    kernel.start().unwrap();
    kernel.run("objective").unwrap();
    assert_eq!(
        kernel.last_session.as_ref().unwrap().phase,
        SessionPhase::Complete
    );
    kernel.mark_last_session_failed();
    assert_eq!(
        kernel.last_session.as_ref().unwrap().phase,
        SessionPhase::Failed
    );
}

#[test]
fn mark_last_session_failed_noop_when_already_failed() {
    let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
    kernel.start().unwrap();
    kernel.run("objective").unwrap();
    kernel.mark_last_session_failed();
    // Calling again should be idempotent.
    kernel.mark_last_session_failed();
    assert_eq!(
        kernel.last_session.as_ref().unwrap().phase,
        SessionPhase::Failed
    );
}

#[test]
fn mark_last_session_failed_noop_when_no_session() {
    let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
    // No session has been run yet.
    kernel.mark_last_session_failed();
    assert!(kernel.last_session.is_none());
}

#[test]
fn snapshot_without_session_has_zero_records() {
    let kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
    let snapshot = kernel.snapshot_for(None).unwrap();
    assert_eq!(snapshot.evidence_records, 0);
    assert_eq!(snapshot.memory_records, 0);
    assert!(snapshot.session_phase.is_none());
}

#[test]
fn snapshot_after_session_has_session_phase() {
    let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
    kernel.start().unwrap();
    kernel.run("test objective").unwrap();
    let session = kernel.last_session.as_ref().unwrap();
    let snapshot = kernel.snapshot_for(Some(session)).unwrap();
    assert_eq!(snapshot.session_phase, Some(SessionPhase::Complete));
    assert!(snapshot.evidence_records > 0);
    assert!(snapshot.memory_records > 0);
}

#[test]
fn session_produces_unique_ids_across_runs() {
    let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
    kernel.start().unwrap();
    let o1 = kernel.run("first").unwrap();
    let o2 = kernel.run("second").unwrap();
    assert_ne!(o1.session.id, o2.session.id);
}
