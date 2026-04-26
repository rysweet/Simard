use std::sync::Arc;

use super::LOCAL_BASE_TYPE;
use super::config::BootstrapConfig;
use crate::agent_program::{
    AgentProgram, GoalCuratorProgram, ImprovementCuratorProgram, MeetingFacilitatorProgram,
    ObjectiveRelayProgram,
};
use crate::base_type_claude_agent_sdk::claude_agent_sdk_adapter;
use crate::base_type_ms_agent::ms_agent_framework_adapter;
use crate::base_type_rustyclawd::RustyClawdAdapter;
use crate::base_types::BaseTypeId;
use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use crate::error::{SimardError, SimardResult};
use crate::evidence::{EvidenceStore, FileBackedEvidenceStore};
use crate::goals::{FileBackedGoalStore, GoalStore};
use crate::handoff::{FileBackedHandoffStore, RuntimeHandoffSnapshot, RuntimeHandoffStore};
use crate::identity::{
    BuiltinIdentityLoader, IdentityLoadRequest, IdentityLoader, IdentityManifest, ManifestContract,
    OperatingMode,
};
use crate::memory::{FileBackedMemoryStore, MemoryStore};
use crate::memory_bridge_adapter::CognitiveBridgeMemoryStore;
use crate::metadata::{Freshness, Provenance};
use crate::prompt_assets::{FilePromptAssetStore, PromptAssetStore};
use crate::reflection::{ReflectionSnapshot, ReflectiveRuntime};
use crate::runtime::{
    BaseTypeRegistry, CoordinatedSupervisor, LocalRuntime, LoopbackMailboxTransport,
    LoopbackMeshTopologyDriver, RuntimePorts, RuntimeRequest, RuntimeTopology, SessionOutcome,
};
use crate::session::UuidSessionIdGenerator;
use crate::test_support::TestAdapter;

const TERMINAL_SHELL_BASE_TYPE: &str = "terminal-shell";
const RUSTY_CLAWD_BASE_TYPE: &str = "rusty-clawd";
const COPILOT_SDK_BASE_TYPE: &str = "copilot-sdk";
const CLAUDE_AGENT_SDK_BASE_TYPE: &str = "claude-agent-sdk";
const MS_AGENT_FRAMEWORK_BASE_TYPE: &str = "ms-agent-framework";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalSessionExecution {
    pub outcome: SessionOutcome,
    pub snapshot: ReflectionSnapshot,
    pub stopped_snapshot: ReflectionSnapshot,
}

/// Build the memory store and an optional native cognitive memory backend
/// for session lifecycle consolidation hooks.
///
/// In `BuiltinDefaults` mode (tests/dev), falls back to file-backed store
/// when LadybugDB is unavailable. In `ExplicitConfig` mode (production),
/// the cognitive memory MUST succeed or startup fails.
fn build_memory_store(
    config: &BootstrapConfig,
) -> SimardResult<(Arc<dyn MemoryStore>, Option<Box<dyn CognitiveMemoryOps>>)> {
    #![allow(clippy::type_complexity)]
    match NativeCognitiveMemory::open(&config.state_root.value) {
        Ok(native) => {
            let native_for_store = NativeCognitiveMemory::open(&config.state_root.value)?;
            let store =
                CognitiveBridgeMemoryStore::new(native_for_store, config.memory_store_path())?;
            store.hydrate_from_bridge()?;

            eprintln!("[simard] consolidation hooks active — session lifecycle hooks enabled");

            Ok((Arc::new(store), Some(Box::new(native))))
        }
        Err(e) if config.mode == crate::bootstrap::BootstrapMode::BuiltinDefaults => {
            eprintln!(
                "[simard] native cognitive memory unavailable ({e}) — using file backend for testing"
            );
            Ok((
                Arc::new(FileBackedMemoryStore::try_new(config.memory_store_path())?),
                None,
            ))
        }
        Err(e) => Err(SimardError::RuntimeInitFailed {
            component: "cognitive-memory".into(),
            reason: format!("native cognitive memory is required in production mode: {e}"),
        }),
    }
}

/// Resolved runtime pieces shared by fresh and handoff assembly paths.
struct AssembledParts {
    ports: RuntimePorts,
    request: RuntimeRequest,
    /// Cognitive memory backend for `RuntimeKernel::set_cognitive_bridge()`.
    consolidation_bridge: Option<Box<dyn CognitiveMemoryOps>>,
}

/// Build all runtime components from a bootstrap config.
fn assemble_parts(config: &BootstrapConfig) -> SimardResult<AssembledParts> {
    let prompt_store = Arc::new(FilePromptAssetStore::new(config.prompt_root.value.clone()));
    let (memory_store, consolidation_bridge) = build_memory_store(config)?;
    let evidence_store = Arc::new(FileBackedEvidenceStore::try_new(
        config.evidence_store_path(),
    )?);
    let goal_store = Arc::new(FileBackedGoalStore::try_new(config.goal_store_path())?);
    let handoff_store = Arc::new(FileBackedHandoffStore::try_new(
        config.handoff_store_path(),
    )?);

    let contract = ManifestContract::new(
        super::bootstrap_entrypoint(),
        "bootstrap-config -> identity-loader -> runtime-ports -> local-runtime",
        config.manifest_precedence(),
        Provenance::new(
            "bootstrap",
            format!("{}:{}", super::bootstrap_entrypoint(), config.identity),
        ),
        Freshness::now()?,
    )?;

    let manifest = BuiltinIdentityLoader.load(&IdentityLoadRequest::new(
        config.identity.clone(),
        env!("CARGO_PKG_VERSION"),
        contract,
    ))?;
    let base_types = builtin_base_type_registry_for_manifest(&manifest)?;
    let request = RuntimeRequest::new(
        manifest,
        config.selected_base_type.value.clone(),
        config.topology.value,
    );
    let agent_program = agent_program_for_manifest(&request.manifest)?;

    let ports = runtime_ports_for_topology(
        prompt_store,
        memory_store,
        evidence_store,
        goal_store,
        handoff_store,
        base_types,
        config.topology.value,
        agent_program,
    )?;

    Ok(AssembledParts {
        ports,
        request,
        consolidation_bridge,
    })
}

pub fn assemble_local_runtime(config: &BootstrapConfig) -> SimardResult<LocalRuntime> {
    let parts = assemble_parts(config)?;
    let mut runtime = LocalRuntime::compose(parts.ports, parts.request)?;
    if let Some(bridge) = parts.consolidation_bridge {
        runtime.set_cognitive_bridge(bridge);
    }
    Ok(runtime)
}

pub fn assemble_local_runtime_from_handoff(
    config: &BootstrapConfig,
    snapshot: RuntimeHandoffSnapshot,
) -> SimardResult<LocalRuntime> {
    let parts = assemble_parts(config)?;
    let mut runtime = LocalRuntime::compose_from_handoff(parts.ports, parts.request, snapshot)?;
    if let Some(bridge) = parts.consolidation_bridge {
        runtime.set_cognitive_bridge(bridge);
    }
    Ok(runtime)
}

pub fn run_local_session(config: &BootstrapConfig) -> SimardResult<LocalSessionExecution> {
    let mut runtime = assemble_local_runtime(config)?;
    runtime.start()?;

    let outcome = runtime.run(config.objective.value.clone())?;
    let _ = runtime.export_handoff()?;

    // Flush any pending bridge writes before shutdown.
    runtime.flush_pending_memory();

    let snapshot = runtime.snapshot()?;
    runtime.stop()?;
    let stopped_snapshot = runtime.snapshot()?;

    Ok(LocalSessionExecution {
        outcome,
        snapshot,
        stopped_snapshot,
    })
}

pub fn latest_local_handoff(
    config: &BootstrapConfig,
) -> SimardResult<Option<RuntimeHandoffSnapshot>> {
    FileBackedHandoffStore::try_new(config.handoff_store_path())?.latest()
}

pub fn builtin_base_type_registry_for_manifest(
    manifest: &IdentityManifest,
) -> SimardResult<BaseTypeRegistry> {
    let mut base_types = BaseTypeRegistry::default();
    for base_type in &manifest.supported_base_types {
        register_builtin_base_type(&mut base_types, base_type)?;
    }
    Ok(base_types)
}

#[expect(
    clippy::too_many_arguments,
    reason = "bootstrap wiring passes explicit stores and runtime services for topology-neutral assembly"
)]
fn runtime_ports_for_topology(
    prompt_store: Arc<dyn PromptAssetStore>,
    memory_store: Arc<dyn MemoryStore>,
    evidence_store: Arc<dyn EvidenceStore>,
    goal_store: Arc<dyn GoalStore>,
    handoff_store: Arc<dyn RuntimeHandoffStore>,
    base_types: BaseTypeRegistry,
    topology: RuntimeTopology,
    agent_program: Arc<dyn AgentProgram>,
) -> SimardResult<RuntimePorts> {
    match topology {
        RuntimeTopology::SingleProcess => Ok(RuntimePorts::with_runtime_services_and_program(
            prompt_store,
            memory_store,
            evidence_store,
            goal_store,
            base_types,
            Arc::new(crate::runtime::InProcessTopologyDriver::try_default()?),
            Arc::new(crate::runtime::InMemoryMailboxTransport::try_default()?),
            Arc::new(crate::runtime::InProcessSupervisor::try_default()?),
            Arc::clone(&agent_program),
            handoff_store,
            Arc::new(UuidSessionIdGenerator),
        )),
        RuntimeTopology::MultiProcess | RuntimeTopology::Distributed => {
            Ok(RuntimePorts::with_runtime_services_and_program(
                prompt_store,
                memory_store,
                evidence_store,
                goal_store,
                base_types,
                Arc::new(LoopbackMeshTopologyDriver::try_default()?),
                Arc::new(LoopbackMailboxTransport::try_default()?),
                Arc::new(CoordinatedSupervisor::try_default()?),
                agent_program,
                handoff_store,
                Arc::new(UuidSessionIdGenerator),
            ))
        }
    }
}

fn agent_program_for_manifest(manifest: &IdentityManifest) -> SimardResult<Arc<dyn AgentProgram>> {
    match manifest.default_mode {
        OperatingMode::Meeting => Ok(Arc::new(MeetingFacilitatorProgram::try_default()?)),
        OperatingMode::Curator => Ok(Arc::new(GoalCuratorProgram::try_default()?)),
        OperatingMode::Improvement => Ok(Arc::new(ImprovementCuratorProgram::try_default()?)),
        OperatingMode::Engineer | OperatingMode::Gym | OperatingMode::Orchestrator => {
            Ok(Arc::new(ObjectiveRelayProgram::try_default()?))
        }
    }
}

pub(super) fn register_builtin_base_type(
    base_types: &mut BaseTypeRegistry,
    base_type: &BaseTypeId,
) -> SimardResult<()> {
    match base_type.as_str() {
        LOCAL_BASE_TYPE => {
            base_types.register(TestAdapter::single_process_alias(
                base_type.as_str(),
                LOCAL_BASE_TYPE,
            )?);
            Ok(())
        }
        TERMINAL_SHELL_BASE_TYPE => {
            base_types.register(
                crate::base_type_harness::RealLocalHarnessAdapter::registered(base_type.as_str())?,
            );
            Ok(())
        }
        RUSTY_CLAWD_BASE_TYPE => {
            base_types.register(RustyClawdAdapter::registered(base_type.as_str())?);
            Ok(())
        }
        COPILOT_SDK_BASE_TYPE => {
            base_types.register(crate::base_type_copilot::CopilotSdkAdapter::registered(
                base_type.as_str(),
            )?);
            Ok(())
        }
        CLAUDE_AGENT_SDK_BASE_TYPE => {
            base_types.register(claude_agent_sdk_adapter(base_type.as_str())?);
            Ok(())
        }
        MS_AGENT_FRAMEWORK_BASE_TYPE => {
            base_types.register(ms_agent_framework_adapter(base_type.as_str())?);
            Ok(())
        }
        _ => Ok(()),
    }
}
