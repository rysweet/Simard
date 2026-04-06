use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use simard::{
    AgentProgram, AgentProgramContext, BackendDescriptor, BaseTypeCapability, BaseTypeDescriptor,
    BaseTypeFactory, BaseTypeId, BaseTypeOutcome, BaseTypeRegistry, BaseTypeSession,
    BaseTypeSessionRequest, BaseTypeTurnInput, EvidenceStore, Freshness, FreshnessState,
    IdentityManifest, InMemoryEvidenceStore, InMemoryGoalStore, InMemoryHandoffStore,
    InMemoryMailboxTransport, InMemoryMemoryStore, InMemoryPromptAssetStore, InProcessSupervisor,
    InProcessTopologyDriver, LocalRuntime, LoopbackMailboxTransport, LoopbackMeshTopologyDriver,
    ManifestContract, MemoryPolicy, MemoryScope, MemoryStore, OperatingMode, PromptAsset,
    PromptAssetRef, Provenance, ReflectiveRuntime, RuntimeHandoffStore, RuntimePorts,
    RuntimeRequest, RuntimeState, RuntimeTopology, SessionPhase, SimardError, SimardResult,
    TestAdapter, UuidSessionIdGenerator, capability_set,
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

#[derive(Debug)]
struct FailingHarnessAdapter {
    descriptor: BaseTypeDescriptor,
    close_count: Arc<AtomicUsize>,
}

impl FailingHarnessAdapter {
    fn with_close_count(id: &str, close_count: Arc<AtomicUsize>) -> Self {
        Self {
            descriptor: BaseTypeDescriptor {
                id: BaseTypeId::new(id),
                backend: BackendDescriptor::new(
                    id,
                    Provenance::injected("test:failing-adapter"),
                    Freshness::now().expect("freshness should be observable"),
                ),
                capabilities: capability_set([
                    BaseTypeCapability::PromptAssets,
                    BaseTypeCapability::SessionLifecycle,
                    BaseTypeCapability::Memory,
                    BaseTypeCapability::Evidence,
                    BaseTypeCapability::Reflection,
                ]),
                supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
            },
            close_count,
        }
    }
}

impl BaseTypeFactory for FailingHarnessAdapter {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open_session(
        &self,
        _request: BaseTypeSessionRequest,
    ) -> SimardResult<Box<dyn BaseTypeSession>> {
        Ok(Box::new(FailingHarnessSession {
            descriptor: self.descriptor.clone(),
            is_open: false,
            close_count: Arc::clone(&self.close_count),
        }))
    }
}

#[derive(Debug)]
struct FailingHarnessSession {
    descriptor: BaseTypeDescriptor,
    is_open: bool,
    close_count: Arc<AtomicUsize>,
}

impl BaseTypeSession for FailingHarnessSession {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open(&mut self) -> SimardResult<()> {
        self.is_open = true;
        Ok(())
    }

    fn run_turn(&mut self, _input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        if !self.is_open {
            return Err(SimardError::InvalidBaseTypeSessionState {
                base_type: self.descriptor.id.to_string(),
                action: "run_turn".to_string(),
                reason: "session must be opened before turns can run".to_string(),
            });
        }

        Err(SimardError::AdapterInvocationFailed {
            base_type: self.descriptor.id.to_string(),
            reason: "simulated adapter failure".to_string(),
        })
    }

    fn close(&mut self) -> SimardResult<()> {
        self.is_open = false;
        self.close_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Debug)]
struct TaggingAgentProgram {
    descriptor: BackendDescriptor,
}

impl TaggingAgentProgram {
    fn new(identity: &str) -> Self {
        Self {
            descriptor: BackendDescriptor::new(
                identity,
                Provenance::injected("test:agent-program"),
                Freshness::now().expect("freshness should be observable"),
            ),
        }
    }
}

impl AgentProgram for TaggingAgentProgram {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn plan_turn(&self, context: &AgentProgramContext) -> SimardResult<BaseTypeTurnInput> {
        Ok(BaseTypeTurnInput::objective_only(format!(
            "agent-program-turn::{}",
            context.objective
        )))
    }

    fn reflection_summary(
        &self,
        context: &AgentProgramContext,
        _outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        Ok(format!(
            "reflection-by={}::{}",
            self.descriptor.identity, context.selected_base_type
        ))
    }

    fn persistence_summary(
        &self,
        context: &AgentProgramContext,
        _outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        Ok(format!(
            "persistence-by={}::{}",
            self.descriptor.identity, context.topology
        ))
    }
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
    base_types
        .register(TestAdapter::single_process("local-harness").expect("adapter should initialize"));

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
    assert_eq!(evidence_records.len(), 4);
    assert_eq!(
        evidence
            .count_for_session(&outcome.session.id)
            .expect("evidence counts should be queryable"),
        4
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
    assert!(scratch_records[0].value.contains("objective-metadata("));
    assert!(
        !scratch_records[0]
            .value
            .contains("exercise the local runtime")
    );
    assert!(
        !summary_records[0]
            .value
            .contains("exercise the local runtime")
    );
    assert!(
        !outcome
            .reflection
            .summary
            .contains("exercise the local runtime")
    );
    assert!(outcome.reflection.summary.contains("objective-metadata("));
    assert_eq!(
        memory
            .count_for_session(&outcome.session.id)
            .expect("memory counts should be queryable"),
        2
    );

    let snapshot = runtime.snapshot().expect("snapshot should succeed");
    assert_eq!(snapshot.runtime_state, RuntimeState::Ready);
    assert_eq!(snapshot.session_phase, Some(SessionPhase::Complete));
    assert_eq!(snapshot.evidence_records, 4);
    assert_eq!(snapshot.memory_records, 2);
    assert_eq!(
        snapshot.agent_program_backend.identity,
        "agent-program::objective-relay"
    );
    assert_eq!(snapshot.handoff_backend.identity, "handoff::in-memory");
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
    base_types
        .register(TestAdapter::single_process("local-harness").expect("adapter should initialize"));

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
    base_types
        .register(TestAdapter::single_process("local-harness").expect("adapter should initialize"));

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

#[test]
fn failed_runs_preserve_failed_session_metadata_until_shutdown() {
    let prompts = Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
        "engineer-system",
        "simard/engineer_system.md",
        "You are Simard.",
    )]));
    let memory = Arc::new(InMemoryMemoryStore::try_default().expect("store should initialize"));
    let evidence = Arc::new(InMemoryEvidenceStore::try_default().expect("store should initialize"));
    let close_count = Arc::new(AtomicUsize::new(0));
    let mut base_types = BaseTypeRegistry::default();
    base_types.register(FailingHarnessAdapter::with_close_count(
        "local-harness",
        Arc::clone(&close_count),
    ));

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
    let error = runtime.run("exercise failure handling").unwrap_err();
    assert_eq!(
        error,
        SimardError::AdapterInvocationFailed {
            base_type: "local-harness".to_string(),
            reason: "simulated adapter failure".to_string(),
        }
    );
    assert_eq!(
        close_count.load(Ordering::SeqCst),
        1,
        "failing session runs must still close the base-type session"
    );

    assert_eq!(runtime.state(), RuntimeState::Failed);

    let failed_snapshot = runtime.snapshot().expect("snapshot should still work");
    assert_eq!(failed_snapshot.runtime_state, RuntimeState::Failed);
    assert_eq!(failed_snapshot.session_phase, Some(SessionPhase::Failed));
    assert_eq!(failed_snapshot.memory_records, 1);
    assert_eq!(failed_snapshot.evidence_records, 0);
    assert_eq!(
        failed_snapshot.agent_program_backend.identity,
        "agent-program::objective-relay"
    );
    assert_eq!(
        failed_snapshot.handoff_backend.identity,
        "handoff::in-memory"
    );
    assert_eq!(
        failed_snapshot.adapter_backend.provenance,
        Provenance::injected("test:failing-adapter")
    );
    assert_eq!(
        failed_snapshot.manifest_contract.freshness.state,
        FreshnessState::Stale
    );

    let start_error = runtime.start().unwrap_err();
    assert_eq!(
        start_error,
        SimardError::RuntimeFailed {
            action: "start".to_string(),
        }
    );

    let run_error = runtime.run("retry after failure").unwrap_err();
    assert_eq!(
        run_error,
        SimardError::RuntimeFailed {
            action: "run".to_string(),
        }
    );

    runtime
        .stop()
        .expect("failed runtimes should still allow an explicit stop boundary");
    let stopped_snapshot = runtime
        .snapshot()
        .expect("stopped snapshot should remain visible");
    assert_eq!(stopped_snapshot.runtime_state, RuntimeState::Stopped);
    assert_eq!(stopped_snapshot.session_phase, Some(SessionPhase::Failed));
    assert_eq!(
        stopped_snapshot.manifest_contract.freshness.state,
        FreshnessState::Stale
    );
}

#[test]
fn runtime_uses_injected_agent_program_for_reflection_and_persistence() {
    let prompts = Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
        "engineer-system",
        "simard/engineer_system.md",
        "You are Simard.",
    )]));
    let memory = Arc::new(InMemoryMemoryStore::try_default().expect("store should initialize"));
    let evidence = Arc::new(InMemoryEvidenceStore::try_default().expect("store should initialize"));
    let mut base_types = BaseTypeRegistry::default();
    base_types
        .register(TestAdapter::single_process("local-harness").expect("adapter should initialize"));

    let request = RuntimeRequest::new(
        manifest(),
        BaseTypeId::new("local-harness"),
        RuntimeTopology::SingleProcess,
    );

    let mut runtime = LocalRuntime::compose(
        RuntimePorts::with_runtime_services_and_program(
            prompts,
            memory.clone(),
            evidence,
            Arc::new(InMemoryGoalStore::try_default().expect("goal store should initialize")),
            base_types,
            Arc::new(InProcessTopologyDriver::try_default().expect("driver should initialize")),
            Arc::new(InMemoryMailboxTransport::try_default().expect("transport should initialize")),
            Arc::new(InProcessSupervisor::try_default().expect("supervisor should initialize")),
            Arc::new(TaggingAgentProgram::new("agent-program::tagging")),
            Arc::new(InMemoryHandoffStore::try_default().expect("handoff should initialize")),
            Arc::new(UuidSessionIdGenerator),
        ),
        request,
    )
    .expect("composition should succeed");

    runtime.start().expect("startup should succeed");
    let outcome = runtime
        .run("exercise custom agent program")
        .expect("run should succeed");

    assert_eq!(
        outcome.reflection.summary,
        "reflection-by=agent-program::tagging::local-harness"
    );

    let summary_records = memory
        .list(MemoryScope::SessionSummary)
        .expect("summary memory should be queryable");
    assert_eq!(summary_records.len(), 1);
    assert_eq!(
        summary_records[0].value,
        "persistence-by=agent-program::tagging::single-process"
    );

    let snapshot = runtime.snapshot().expect("snapshot should succeed");
    assert_eq!(
        snapshot.agent_program_backend.identity,
        "agent-program::tagging"
    );
    assert_eq!(
        snapshot.agent_program_backend.provenance,
        Provenance::injected("test:agent-program")
    );
    assert_eq!(snapshot.handoff_backend.identity, "handoff::in-memory");
}

#[test]
fn runtime_runs_same_agent_program_on_multi_process_mesh_driver() {
    let prompts = Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
        "engineer-system",
        "simard/engineer_system.md",
        "You are Simard.",
    )]));
    let memory = Arc::new(InMemoryMemoryStore::try_default().expect("store should initialize"));
    let evidence = Arc::new(InMemoryEvidenceStore::try_default().expect("store should initialize"));
    let mut base_types = BaseTypeRegistry::default();
    base_types.register(
        TestAdapter::new(
            "local-harness",
            "local-harness",
            simard::standard_session_capabilities(),
            [
                RuntimeTopology::SingleProcess,
                RuntimeTopology::MultiProcess,
            ],
        )
        .expect("test adapter should initialize"),
    );

    let request = RuntimeRequest::new(
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
                simard::bootstrap_entrypoint(),
                "bootstrap-config -> identity-loader -> runtime-ports -> local-runtime",
                vec!["tests:lifecycle-multiprocess".to_string()],
                Provenance::new("test", "lifecycle::multiprocess"),
                Freshness::now().expect("freshness should be observable"),
            )
            .expect("contract should be valid"),
        )
        .expect("manifest should be valid"),
        BaseTypeId::new("local-harness"),
        RuntimeTopology::MultiProcess,
    );

    let mut runtime = LocalRuntime::compose(
        RuntimePorts::with_runtime_services_and_program(
            prompts,
            memory,
            evidence,
            Arc::new(InMemoryGoalStore::try_default().expect("goal store should initialize")),
            base_types,
            Arc::new(LoopbackMeshTopologyDriver::try_default().expect("driver should initialize")),
            Arc::new(LoopbackMailboxTransport::try_default().expect("transport should initialize")),
            Arc::new(InProcessSupervisor::try_default().expect("supervisor should initialize")),
            Arc::new(TaggingAgentProgram::new("agent-program::tagging")),
            Arc::new(InMemoryHandoffStore::try_default().expect("handoff should initialize")),
            Arc::new(UuidSessionIdGenerator),
        ),
        request,
    )
    .expect("composition should succeed");

    runtime.start().expect("startup should succeed");
    let outcome = runtime
        .run("exercise mesh runtime")
        .expect("run should succeed");

    assert_eq!(
        outcome.reflection.summary,
        "reflection-by=agent-program::tagging::local-harness"
    );
    let snapshot = runtime.snapshot().expect("snapshot should succeed");
    assert_eq!(snapshot.topology, RuntimeTopology::MultiProcess);
    assert_eq!(snapshot.runtime_node.to_string(), "node-loopback-mesh");
    assert_eq!(
        snapshot.mailbox_address.to_string(),
        "loopback://node-loopback-mesh"
    );
    assert_eq!(snapshot.adapter_backend.identity, "local-harness");
    assert_eq!(
        snapshot.topology_backend.identity,
        "topology::loopback-mesh"
    );
    assert_eq!(
        snapshot.transport_backend.identity,
        "transport::loopback-mailbox"
    );
}

#[test]
fn runtime_can_export_and_restore_handoff_snapshot() {
    let prompts = Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
        "engineer-system",
        "simard/engineer_system.md",
        "You are Simard.",
    )]));
    let memory = Arc::new(InMemoryMemoryStore::try_default().expect("store should initialize"));
    let evidence = Arc::new(InMemoryEvidenceStore::try_default().expect("store should initialize"));
    let handoff = Arc::new(InMemoryHandoffStore::try_default().expect("handoff should initialize"));
    let mut base_types = BaseTypeRegistry::default();
    base_types.register(
        TestAdapter::new(
            "local-harness",
            "local-harness",
            simard::standard_session_capabilities(),
            [
                RuntimeTopology::SingleProcess,
                RuntimeTopology::MultiProcess,
            ],
        )
        .expect("test adapter should initialize"),
    );

    let request = RuntimeRequest::new(
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
                simard::bootstrap_entrypoint(),
                "bootstrap-config -> identity-loader -> runtime-ports -> local-runtime",
                vec!["tests:lifecycle-handoff".to_string()],
                Provenance::new("test", "lifecycle::handoff"),
                Freshness::now().expect("freshness should be observable"),
            )
            .expect("contract should be valid"),
        )
        .expect("manifest should be valid"),
        BaseTypeId::new("local-harness"),
        RuntimeTopology::MultiProcess,
    );

    let mut runtime = LocalRuntime::compose(
        RuntimePorts::with_runtime_services_and_program(
            prompts.clone(),
            memory,
            evidence,
            Arc::new(InMemoryGoalStore::try_default().expect("goal store should initialize")),
            base_types,
            Arc::new(LoopbackMeshTopologyDriver::try_default().expect("driver should initialize")),
            Arc::new(LoopbackMailboxTransport::try_default().expect("transport should initialize")),
            Arc::new(InProcessSupervisor::try_default().expect("supervisor should initialize")),
            Arc::new(TaggingAgentProgram::new("agent-program::tagging")),
            handoff.clone(),
            Arc::new(UuidSessionIdGenerator),
        ),
        request.clone(),
    )
    .expect("composition should succeed");

    runtime.start().expect("startup should succeed");
    let outcome = runtime.run("export handoff").expect("run should succeed");
    let snapshot = runtime.export_handoff().expect("handoff should export");

    assert_eq!(
        snapshot.selected_base_type,
        BaseTypeId::new("local-harness")
    );
    assert_eq!(snapshot.topology, RuntimeTopology::MultiProcess);
    assert_eq!(snapshot.memory_records.len(), 2);
    assert!(
        snapshot.evidence_records.len() >= 4,
        "expected at least 4 evidence records, got {}",
        snapshot.evidence_records.len()
    );
    assert_eq!(
        handoff.latest().expect("handoff latest should work"),
        Some(snapshot.clone())
    );

    let restored_memory =
        Arc::new(InMemoryMemoryStore::try_default().expect("store should initialize"));
    let restored_evidence =
        Arc::new(InMemoryEvidenceStore::try_default().expect("store should initialize"));
    let restored_handoff =
        Arc::new(InMemoryHandoffStore::try_default().expect("handoff should initialize"));
    let mut restored_base_types = BaseTypeRegistry::default();
    restored_base_types.register(
        TestAdapter::new(
            "local-harness",
            "local-harness",
            simard::standard_session_capabilities(),
            [
                RuntimeTopology::SingleProcess,
                RuntimeTopology::MultiProcess,
            ],
        )
        .expect("test adapter should initialize"),
    );

    let restored = LocalRuntime::compose_from_handoff(
        RuntimePorts::with_runtime_services_and_program(
            prompts,
            restored_memory.clone(),
            restored_evidence.clone(),
            Arc::new(InMemoryGoalStore::try_default().expect("goal store should initialize")),
            restored_base_types,
            Arc::new(LoopbackMeshTopologyDriver::try_default().expect("driver should initialize")),
            Arc::new(LoopbackMailboxTransport::try_default().expect("transport should initialize")),
            Arc::new(InProcessSupervisor::try_default().expect("supervisor should initialize")),
            Arc::new(TaggingAgentProgram::new("agent-program::tagging")),
            restored_handoff.clone(),
            Arc::new(UuidSessionIdGenerator),
        ),
        request,
        snapshot,
    )
    .expect("restored runtime should compose");

    let restored_snapshot = restored.snapshot().expect("snapshot should succeed");
    assert_eq!(restored_snapshot.runtime_state, RuntimeState::Initializing);
    assert_eq!(
        restored_snapshot.session_phase,
        Some(SessionPhase::Complete)
    );
    assert_eq!(restored_snapshot.memory_records, 2);
    assert!(restored_snapshot.evidence_records >= 4);
    assert_eq!(
        restored_snapshot.handoff_backend.identity,
        "handoff::in-memory"
    );

    assert_eq!(
        restored_memory
            .count_for_session(&outcome.session.id)
            .expect("restored memory should be hydrated"),
        2
    );
    assert!(
        restored_evidence
            .count_for_session(&outcome.session.id)
            .expect("restored evidence should be hydrated")
            >= 4
    );
}
