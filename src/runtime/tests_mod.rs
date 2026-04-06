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
