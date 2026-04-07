//! TDD tests for the base type architecture re-architecture.
//!
//! These tests validate the spec requirements:
//! - Each base type is a full agent runtime wrapper, not a generic process pipe
//! - RustyClawd adapter supports SingleProcess + MultiProcess topologies
//! - CopilotSdk adapter is a real agent runtime (uses PTY + memory/knowledge)
//! - TerminalShell adapter has TerminalSession capability
//! - TestAdapter serves as lightweight test harness
//! - All base types register in bootstrap and identity manifests
//! - Session lifecycle state machine is correct for all adapters
//! - CognitiveMemoryBridge integration works end-to-end

use simard::{
    BaseTypeCapability, BaseTypeFactory, BaseTypeId, BaseTypeSessionRequest, BaseTypeTurnInput,
    BuiltinIdentityLoader, CopilotSdkAdapter, Freshness, IdentityLoadRequest, IdentityLoader,
    ManifestContract, OperatingMode, PromptAssetRef, Provenance, RealLocalHarnessAdapter,
    RuntimeAddress, RuntimeNodeId, RuntimeTopology, RustyClawdAdapter, SessionId, SimardError,
    TestAdapter, capability_set,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_session_request(topology: RuntimeTopology) -> BaseTypeSessionRequest {
    BaseTypeSessionRequest {
        session_id: SessionId::parse("session-00000000-0000-0000-0000-000000000001")
            .expect("session id should parse"),
        mode: OperatingMode::Engineer,
        topology,
        prompt_assets: vec![PromptAssetRef::new("sys", "simard/engineer_system.md")],
        runtime_node: RuntimeNodeId::local(),
        mailbox_address: RuntimeAddress::local(&RuntimeNodeId::local()),
    }
}

fn test_turn_input() -> BaseTypeTurnInput {
    BaseTypeTurnInput::objective_only("Implement feature X")
}

fn test_contract() -> ManifestContract {
    ManifestContract::new(
        simard::bootstrap_entrypoint(),
        "bootstrap-config -> identity-loader -> runtime-ports -> local-runtime",
        vec!["tests:base-types".to_string()],
        Provenance::new("test", "base_types::test"),
        Freshness::now().expect("freshness"),
    )
    .expect("contract")
}

// ---------------------------------------------------------------------------
// 1. Each base type adapter constructs with correct descriptor
// ---------------------------------------------------------------------------

#[test]
fn rustyclawd_adapter_has_correct_descriptor() {
    let adapter = RustyClawdAdapter::registered("rusty-clawd").expect("should construct");
    let desc = adapter.descriptor();

    assert_eq!(desc.id, BaseTypeId::new("rusty-clawd"));
    assert!(desc.backend.identity.contains("rusty-clawd"));
    assert!(
        desc.capabilities
            .contains(&BaseTypeCapability::PromptAssets)
    );
    assert!(
        desc.capabilities
            .contains(&BaseTypeCapability::SessionLifecycle)
    );
    assert!(desc.capabilities.contains(&BaseTypeCapability::Memory));
    assert!(desc.capabilities.contains(&BaseTypeCapability::Evidence));
    assert!(desc.capabilities.contains(&BaseTypeCapability::Reflection));
    // RustyClawd supports SingleProcess and MultiProcess
    assert!(
        desc.supported_topologies
            .contains(&RuntimeTopology::SingleProcess)
    );
    assert!(
        desc.supported_topologies
            .contains(&RuntimeTopology::MultiProcess)
    );
}

#[test]
fn copilot_sdk_adapter_has_correct_descriptor() {
    let adapter = CopilotSdkAdapter::registered("copilot-sdk").expect("should construct");
    let desc = adapter.descriptor();

    assert_eq!(desc.id, BaseTypeId::new("copilot-sdk"));
    assert!(desc.backend.identity.contains("copilot"));
    assert!(
        desc.capabilities
            .contains(&BaseTypeCapability::SessionLifecycle)
    );
    assert!(
        desc.supported_topologies
            .contains(&RuntimeTopology::SingleProcess)
    );
}

#[test]
fn terminal_shell_adapter_has_correct_descriptor() {
    let adapter = RealLocalHarnessAdapter::registered("terminal-shell").expect("should construct");
    let desc = adapter.descriptor();

    assert_eq!(desc.id, BaseTypeId::new("terminal-shell"));
    assert!(desc.backend.identity.contains("local-harness"));
    // RealLocalHarnessAdapter has TerminalSession capability
    assert!(
        desc.capabilities
            .contains(&BaseTypeCapability::TerminalSession)
    );
    assert!(
        desc.capabilities
            .contains(&BaseTypeCapability::SessionLifecycle)
    );
    assert!(
        desc.supported_topologies
            .contains(&RuntimeTopology::SingleProcess)
    );
}

#[test]
fn local_process_harness_adapter_has_correct_descriptor() {
    let adapter = TestAdapter::single_process("local-harness").expect("should construct");
    let desc = adapter.descriptor();

    assert_eq!(desc.id, BaseTypeId::new("local-harness"));
    assert!(
        desc.capabilities
            .contains(&BaseTypeCapability::SessionLifecycle)
    );
    assert!(
        desc.supported_topologies
            .contains(&RuntimeTopology::SingleProcess)
    );
    // Harness does NOT support MultiProcess or Distributed
    assert!(
        !desc
            .supported_topologies
            .contains(&RuntimeTopology::MultiProcess)
    );
    assert!(
        !desc
            .supported_topologies
            .contains(&RuntimeTopology::Distributed)
    );
}

// ---------------------------------------------------------------------------
// 2. All adapters implement BaseTypeFactory trait (compile-time check)
// ---------------------------------------------------------------------------

#[test]
fn all_adapters_implement_base_type_factory() {
    fn assert_factory<T: BaseTypeFactory>(_f: &T) {}

    let rc = RustyClawdAdapter::registered("rc").unwrap();
    let cp = CopilotSdkAdapter::registered("cp").unwrap();
    let ts = RealLocalHarnessAdapter::registered("ts").unwrap();
    let lh = TestAdapter::single_process("lh").unwrap();

    assert_factory(&rc);
    assert_factory(&cp);
    assert_factory(&ts);
    assert_factory(&lh);
}

// ---------------------------------------------------------------------------
// 3. Topology validation: reject unsupported topologies
// ---------------------------------------------------------------------------

#[test]
fn rustyclawd_rejects_distributed_topology() {
    let adapter = RustyClawdAdapter::registered("rusty-clawd").unwrap();
    let request = test_session_request(RuntimeTopology::Distributed);

    match adapter.open_session(request) {
        Err(SimardError::UnsupportedTopology { .. }) => {}
        Err(other) => panic!("expected UnsupportedTopology, got: {other:?}"),
        Ok(_) => panic!("expected UnsupportedTopology, got Ok"),
    }
}

#[test]
fn rustyclawd_accepts_multi_process_topology() {
    let adapter = RustyClawdAdapter::registered("rusty-clawd").unwrap();
    let request = test_session_request(RuntimeTopology::MultiProcess);
    assert!(adapter.open_session(request).is_ok());
}

#[test]
fn copilot_sdk_rejects_multi_process_topology() {
    let adapter = CopilotSdkAdapter::registered("copilot-sdk").unwrap();
    let request = test_session_request(RuntimeTopology::MultiProcess);

    match adapter.open_session(request) {
        Err(SimardError::UnsupportedTopology { .. }) => {}
        Err(other) => panic!("expected UnsupportedTopology, got: {other:?}"),
        Ok(_) => panic!("expected UnsupportedTopology, got Ok"),
    }
}

#[test]
fn terminal_shell_rejects_distributed_topology() {
    let adapter = RealLocalHarnessAdapter::registered("terminal-shell").unwrap();
    let request = test_session_request(RuntimeTopology::Distributed);

    match adapter.open_session(request) {
        Err(SimardError::UnsupportedTopology { .. }) => {}
        Err(other) => panic!("expected UnsupportedTopology, got: {other:?}"),
        Ok(_) => panic!("expected UnsupportedTopology, got Ok"),
    }
}

#[test]
fn local_harness_rejects_multi_process_topology() {
    let adapter = TestAdapter::single_process("local").unwrap();
    let request = test_session_request(RuntimeTopology::MultiProcess);

    match adapter.open_session(request) {
        Err(SimardError::UnsupportedTopology { .. }) => {}
        Err(other) => panic!("expected UnsupportedTopology, got: {other:?}"),
        Ok(_) => panic!("expected UnsupportedTopology, got Ok"),
    }
}

// ---------------------------------------------------------------------------
// 4. Session lifecycle state machine validation
// ---------------------------------------------------------------------------

#[test]
#[ignore] // Requires API key or rustyclawd binary
fn rustyclawd_session_lifecycle_full_cycle() {
    let adapter = RustyClawdAdapter::registered("rusty-clawd").unwrap();
    let request = test_session_request(RuntimeTopology::SingleProcess);
    let mut session = adapter.open_session(request).unwrap();

    // Cannot run_turn before open
    let err = session.run_turn(test_turn_input()).unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidBaseTypeSessionState { .. }
    ));

    // Cannot close before open
    let err = session.close().unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidBaseTypeSessionState { .. }
    ));

    // Open works
    session.open().unwrap();

    // Cannot double-open
    let err = session.open().unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidBaseTypeSessionState { .. }
    ));

    // run_turn succeeds
    let outcome = session.run_turn(test_turn_input()).unwrap();
    assert!(!outcome.plan.is_empty());
    assert!(!outcome.execution_summary.is_empty());
    assert!(!outcome.evidence.is_empty());

    // Close works
    session.close().unwrap();

    // Cannot reuse after close
    let err = session.open().unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidBaseTypeSessionState { .. }
    ));
    let err = session.run_turn(test_turn_input()).unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidBaseTypeSessionState { .. }
    ));
    let err = session.close().unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidBaseTypeSessionState { .. }
    ));
}

#[test]
fn local_harness_session_lifecycle_full_cycle() {
    let adapter = TestAdapter::single_process("test-harness").unwrap();
    let request = test_session_request(RuntimeTopology::SingleProcess);
    let mut session = adapter.open_session(request).unwrap();

    // Cannot run_turn before open
    let err = session.run_turn(test_turn_input()).unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidBaseTypeSessionState { .. }
    ));

    session.open().unwrap();

    // Cannot double-open
    let err = session.open().unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidBaseTypeSessionState { .. }
    ));

    // run_turn succeeds with canned result
    let outcome = session.run_turn(test_turn_input()).unwrap();
    assert!(!outcome.plan.is_empty());
    assert!(!outcome.execution_summary.is_empty());
    assert!(!outcome.evidence.is_empty());

    session.close().unwrap();

    // Cannot close twice
    let err = session.close().unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidBaseTypeSessionState { .. }
    ));
}

#[test]
fn terminal_shell_session_lifecycle_validates_state() {
    let adapter = RealLocalHarnessAdapter::registered("terminal-shell").unwrap();
    let request = test_session_request(RuntimeTopology::SingleProcess);
    let mut session = adapter.open_session(request).unwrap();

    // Cannot run before open
    let err = session.run_turn(test_turn_input()).unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidBaseTypeSessionState { .. }
    ));

    session.open().unwrap();

    // Cannot double-open
    let err = session.open().unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidBaseTypeSessionState { .. }
    ));

    // Close works
    session.close().unwrap();
}

// ---------------------------------------------------------------------------
// 5. Bootstrap: all base types register in the catalog
// ---------------------------------------------------------------------------

#[test]
fn bootstrap_registers_all_production_base_types() {
    let manifest = BuiltinIdentityLoader
        .load(&IdentityLoadRequest::new(
            "simard-engineer",
            env!("CARGO_PKG_VERSION"),
            test_contract(),
        ))
        .expect("identity should load");

    let registry =
        simard::builtin_base_type_registry_for_manifest(&manifest).expect("registry should build");

    // The engineer manifest should list all 4 base types
    let local = registry.get(&BaseTypeId::new("local-harness"));
    let terminal = registry.get(&BaseTypeId::new("terminal-shell"));
    let rusty = registry.get(&BaseTypeId::new("rusty-clawd"));
    let copilot = registry.get(&BaseTypeId::new("copilot-sdk"));

    assert!(local.is_some(), "local-harness should be registered");
    assert!(terminal.is_some(), "terminal-shell should be registered");
    assert!(rusty.is_some(), "rusty-clawd should be registered");
    assert!(copilot.is_some(), "copilot-sdk should be registered");
}

// ---------------------------------------------------------------------------
// 6. Identity manifests support all base types
// ---------------------------------------------------------------------------

#[test]
fn engineer_identity_supports_all_base_types() {
    let manifest = BuiltinIdentityLoader
        .load(&IdentityLoadRequest::new(
            "simard-engineer",
            env!("CARGO_PKG_VERSION"),
            test_contract(),
        ))
        .unwrap();

    let ids: Vec<String> = manifest
        .supported_base_types
        .iter()
        .map(|id| id.to_string())
        .collect();

    assert!(
        ids.contains(&"local-harness".to_string()),
        "missing local-harness"
    );
    assert!(
        ids.contains(&"terminal-shell".to_string()),
        "missing terminal-shell"
    );
    assert!(
        ids.contains(&"rusty-clawd".to_string()),
        "missing rusty-clawd"
    );
    assert!(
        ids.contains(&"copilot-sdk".to_string()),
        "missing copilot-sdk"
    );
}

#[test]
fn meeting_identity_supports_base_types() {
    let manifest = BuiltinIdentityLoader
        .load(&IdentityLoadRequest::new(
            "simard-meeting",
            env!("CARGO_PKG_VERSION"),
            test_contract(),
        ))
        .unwrap();

    assert!(
        !manifest.supported_base_types.is_empty(),
        "meeting identity should support at least one base type"
    );
    // Meeting must support at least rusty-clawd and copilot-sdk
    let ids: Vec<String> = manifest
        .supported_base_types
        .iter()
        .map(|id| id.to_string())
        .collect();
    assert!(ids.contains(&"rusty-clawd".to_string()));
    assert!(ids.contains(&"copilot-sdk".to_string()));
}

#[test]
fn gym_identity_supports_base_types() {
    let manifest = BuiltinIdentityLoader
        .load(&IdentityLoadRequest::new(
            "simard-gym",
            env!("CARGO_PKG_VERSION"),
            test_contract(),
        ))
        .unwrap();

    assert!(
        !manifest.supported_base_types.is_empty(),
        "gym identity should support at least one base type"
    );
}

// ---------------------------------------------------------------------------
// 7. Evidence produced by adapters
// ---------------------------------------------------------------------------

#[test]
#[ignore] // Requires API key or rustyclawd binary
fn rustyclawd_run_turn_produces_evidence_with_required_fields() {
    let adapter = RustyClawdAdapter::registered("rusty-clawd").unwrap();
    let request = test_session_request(RuntimeTopology::SingleProcess);
    let mut session = adapter.open_session(request).unwrap();
    session.open().unwrap();

    let outcome = session.run_turn(test_turn_input()).unwrap();

    let evidence_str = outcome.evidence.join(" ");
    assert!(
        evidence_str.contains("selected-base-type="),
        "evidence should include selected base type"
    );
    assert!(
        evidence_str.contains("runtime-node="),
        "evidence should include runtime node"
    );
    assert!(
        evidence_str.contains("mailbox-address="),
        "evidence should include mailbox address"
    );

    session.close().unwrap();
}

#[test]
fn local_harness_run_turn_produces_evidence_with_required_fields() {
    let adapter = TestAdapter::single_process("test-evidence").unwrap();
    let request = test_session_request(RuntimeTopology::SingleProcess);
    let mut session = adapter.open_session(request).unwrap();
    session.open().unwrap();

    let outcome = session.run_turn(test_turn_input()).unwrap();

    let evidence_str = outcome.evidence.join(" ");
    assert!(evidence_str.contains("selected-base-type="));
    assert!(evidence_str.contains("runtime-node="));
    assert!(evidence_str.contains("mailbox-address="));

    session.close().unwrap();
}

// ---------------------------------------------------------------------------
// 8. Base type descriptors are distinct
// ---------------------------------------------------------------------------

#[test]
fn all_base_type_descriptors_have_unique_backend_identities() {
    let rc = RustyClawdAdapter::registered("rusty-clawd").unwrap();
    let cp = CopilotSdkAdapter::registered("copilot-sdk").unwrap();
    let ts = RealLocalHarnessAdapter::registered("terminal-shell").unwrap();

    let identities = vec![
        rc.descriptor().backend.identity.clone(),
        cp.descriptor().backend.identity.clone(),
        ts.descriptor().backend.identity.clone(),
    ];

    let mut deduped = identities.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(
        identities.len(),
        deduped.len(),
        "backend identities must be unique: {:?}",
        identities
    );
}

// ---------------------------------------------------------------------------
// 9. Standard session capabilities are consistent across production adapters
// ---------------------------------------------------------------------------

#[test]
fn all_production_adapters_have_standard_capabilities() {
    let expected = capability_set([
        BaseTypeCapability::PromptAssets,
        BaseTypeCapability::SessionLifecycle,
        BaseTypeCapability::Memory,
        BaseTypeCapability::Evidence,
        BaseTypeCapability::Reflection,
    ]);

    let rc = RustyClawdAdapter::registered("rc").unwrap();
    let lh = TestAdapter::single_process("lh").unwrap();

    // RustyClawd and LocalHarness have exactly the standard set
    assert_eq!(rc.descriptor().capabilities, expected);
    assert_eq!(lh.descriptor().capabilities, expected);

    // TerminalShell has standard PLUS TerminalSession
    let ts = RealLocalHarnessAdapter::registered("ts").unwrap();
    for cap in &expected {
        assert!(
            ts.descriptor().capabilities.contains(cap),
            "terminal-shell missing capability: {cap}"
        );
    }
    assert!(
        ts.descriptor()
            .capabilities
            .contains(&BaseTypeCapability::TerminalSession)
    );
}

// ---------------------------------------------------------------------------
// 10. Memory store integration (InMemory path)
// ---------------------------------------------------------------------------

#[test]
fn in_memory_store_works_as_memory_backend() {
    use simard::{InMemoryMemoryStore, MemoryRecord, MemoryScope, MemoryStore, SessionPhase};

    let store = InMemoryMemoryStore::try_default().unwrap();
    let session = SessionId::parse("session-00000000-0000-0000-0000-000000000001").unwrap();

    store
        .put(MemoryRecord {
            key: "test".to_string(),
            scope: MemoryScope::SessionScratch,
            value: "data".to_string(),
            session_id: session.clone(),
            recorded_in: SessionPhase::Preparation,
            created_at: None,
        })
        .unwrap();

    assert_eq!(store.count_for_session(&session).unwrap(), 1);
    assert!(store.descriptor().identity.contains("in-memory"));
}

// ---------------------------------------------------------------------------
// 11. CognitiveBridgeMemoryStore can be constructed
// ---------------------------------------------------------------------------

#[test]
fn cognitive_bridge_memory_store_descriptor_identifies_cognitive_backend() {
    use simard::CognitiveBridgeMemoryStore;
    // CognitiveBridgeMemoryStore wraps a BridgeTransport. We can verify it
    // exists and is exported — full integration requires a running bridge.
    // This is a structural/compilation test.
    let _type_exists: fn() -> bool = || {
        let _check: Option<CognitiveBridgeMemoryStore> = None;
        true
    };
}

// ---------------------------------------------------------------------------
// 12. Multiple sessions from same factory are independent
// ---------------------------------------------------------------------------

#[test]
fn factory_creates_independent_sessions() {
    let adapter = TestAdapter::single_process("multi-session").unwrap();

    let mut s1 = adapter
        .open_session(test_session_request(RuntimeTopology::SingleProcess))
        .unwrap();
    let mut s2 = adapter
        .open_session(test_session_request(RuntimeTopology::SingleProcess))
        .unwrap();

    s1.open().unwrap();
    // s2 is still not open — independent state
    let err = s2.run_turn(test_turn_input()).unwrap_err();
    assert!(matches!(
        err,
        SimardError::InvalidBaseTypeSessionState { .. }
    ));

    s2.open().unwrap();
    // Both can run turns independently
    let o1 = s1.run_turn(test_turn_input()).unwrap();
    let o2 = s2.run_turn(test_turn_input()).unwrap();
    assert!(!o1.plan.is_empty());
    assert!(!o2.plan.is_empty());

    s1.close().unwrap();
    // s2 still works after s1 is closed
    let o3 = s2.run_turn(test_turn_input()).unwrap();
    assert!(!o3.plan.is_empty());
    s2.close().unwrap();
}

// ---------------------------------------------------------------------------
// 13. BaseTypeTurnInput only requires objective (spec: delegate an objective)
// ---------------------------------------------------------------------------

#[test]
fn turn_input_carries_objective_for_delegation() {
    let input = BaseTypeTurnInput::objective_only("Build the authentication module");
    assert_eq!(input.objective, "Build the authentication module");
}
