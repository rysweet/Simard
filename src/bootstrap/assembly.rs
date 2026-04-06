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
use crate::bridge_launcher::{cognitive_memory_db_path, find_python_dir, launch_memory_bridge};
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

/// Build the memory store — cognitive bridge is mandatory in production.
///
/// In `BuiltinDefaults` mode (tests/dev), falls back to file-backed store
/// when the Python bridge is unavailable. In `ExplicitConfig` mode (production),
/// the cognitive bridge MUST succeed or startup fails.
fn build_memory_store(config: &BootstrapConfig) -> SimardResult<Arc<dyn MemoryStore>> {
    let bridge = find_python_dir().ok().and_then(|python_dir| {
        let db_path = cognitive_memory_db_path(&config.state_root.value);
        launch_memory_bridge(&config.identity, &db_path, &python_dir).ok()
    });

    if let Some(bridge) = bridge {
        eprintln!("[simard] cognitive memory bridge active — using LadybugDB backend");
        let store = CognitiveBridgeMemoryStore::new(bridge, config.memory_store_path())?;
        store.hydrate_from_bridge();
        Ok(Arc::new(store))
    } else if config.mode == crate::bootstrap::BootstrapMode::BuiltinDefaults {
        eprintln!(
            "[simard] cognitive memory bridge unavailable (builtin-defaults mode) — using file backend for testing"
        );
        Ok(Arc::new(FileBackedMemoryStore::try_new(
            config.memory_store_path(),
        )?))
    } else {
        Err(SimardError::BridgeSpawnFailed {
            bridge: "cognitive-memory".into(),
            reason: "cognitive memory bridge is required in production mode. \
                     Ensure Python and the bridge server are available, or set \
                     SIMARD_BOOTSTRAP_MODE=builtin-defaults for testing."
                .into(),
        })
    }
}

/// Resolved runtime pieces shared by fresh and handoff assembly paths.
struct AssembledParts {
    ports: RuntimePorts,
    request: RuntimeRequest,
}

/// Build all runtime components from a bootstrap config.
fn assemble_parts(config: &BootstrapConfig) -> SimardResult<AssembledParts> {
    let prompt_store = Arc::new(FilePromptAssetStore::new(config.prompt_root.value.clone()));
    let memory_store = build_memory_store(config)?;
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

    Ok(AssembledParts { ports, request })
}

pub fn assemble_local_runtime(config: &BootstrapConfig) -> SimardResult<LocalRuntime> {
    let parts = assemble_parts(config)?;
    LocalRuntime::compose(parts.ports, parts.request)
}

pub fn assemble_local_runtime_from_handoff(
    config: &BootstrapConfig,
    snapshot: RuntimeHandoffSnapshot,
) -> SimardResult<LocalRuntime> {
    let parts = assemble_parts(config)?;
    LocalRuntime::compose_from_handoff(parts.ports, parts.request, snapshot)
}

pub fn run_local_session(config: &BootstrapConfig) -> SimardResult<LocalSessionExecution> {
    let mut runtime = assemble_local_runtime(config)?;
    runtime.start()?;

    let outcome = runtime.run(config.objective.value.clone())?;
    let _ = runtime.export_handoff()?;
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

fn register_builtin_base_type(
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

#[cfg(test)]
mod tests {
    use super::{
        LOCAL_BASE_TYPE, builtin_base_type_registry_for_manifest, register_builtin_base_type,
    };
    use crate::base_type_rustyclawd::RustyClawdAdapter;
    use crate::base_types::{BaseTypeFactory, BaseTypeId};
    use crate::identity::{
        BuiltinIdentityLoader, IdentityLoadRequest, IdentityLoader, ManifestContract,
    };
    use crate::metadata::{Freshness, Provenance};
    use crate::runtime::BaseTypeRegistry;

    #[test]
    fn builtin_adapter_catalog_covers_manifest_advertised_base_types() {
        let manifest = BuiltinIdentityLoader
            .load(&IdentityLoadRequest::new(
                "simard-engineer",
                env!("CARGO_PKG_VERSION"),
                ManifestContract::new(
                    crate::bootstrap_entrypoint(),
                    "bootstrap-config -> identity-loader -> runtime-ports -> local-runtime",
                    vec!["tests:bootstrap-catalog".to_string()],
                    Provenance::new("test", "bootstrap::catalog"),
                    Freshness::now().expect("freshness should be observable"),
                )
                .expect("contract should be valid"),
            ))
            .expect("builtin identity should load");

        let registry =
            builtin_base_type_registry_for_manifest(&manifest).expect("registry should build");
        let local = registry
            .get(&BaseTypeId::new("local-harness"))
            .expect("local harness should be registered");
        let rusty = registry
            .get(&BaseTypeId::new("rusty-clawd"))
            .expect("rusty-clawd should be registered");
        let copilot = registry
            .get(&BaseTypeId::new("copilot-sdk"))
            .expect("copilot-sdk should be registered");
        let claude_sdk = registry
            .get(&BaseTypeId::new("claude-agent-sdk"))
            .expect("claude-agent-sdk should be registered");
        let ms_agent = registry
            .get(&BaseTypeId::new("ms-agent-framework"))
            .expect("ms-agent-framework should be registered");

        assert_eq!(local.descriptor().backend.identity, LOCAL_BASE_TYPE);
        assert_eq!(
            copilot.descriptor().backend.identity,
            "copilot-sdk::pty-session"
        );
        assert_eq!(
            rusty.descriptor().backend.identity,
            RustyClawdAdapter::registered("rusty-clawd")
                .expect("rusty-clawd adapter should initialize")
                .descriptor()
                .backend
                .identity
        );
        assert_eq!(
            claude_sdk.descriptor().backend.identity,
            "claude-agent-sdk::session-backend"
        );
        assert_eq!(
            ms_agent.descriptor().backend.identity,
            "ms-agent-framework::session-backend"
        );
    }

    // ── register_builtin_base_type ──

    #[test]
    fn register_unknown_base_type_does_not_error() {
        let mut registry = BaseTypeRegistry::default();
        let result = register_builtin_base_type(&mut registry, &BaseTypeId::new("nonexistent"));
        assert!(
            result.is_ok(),
            "unknown base type should be silently ignored"
        );
    }

    #[test]
    fn register_local_harness_base_type_succeeds() {
        let mut registry = BaseTypeRegistry::default();
        let result = register_builtin_base_type(&mut registry, &BaseTypeId::new("local-harness"));
        assert!(result.is_ok());
        assert!(registry.get(&BaseTypeId::new("local-harness")).is_some());
    }

    #[test]
    fn register_rusty_clawd_base_type_succeeds() {
        let mut registry = BaseTypeRegistry::default();
        let result = register_builtin_base_type(&mut registry, &BaseTypeId::new("rusty-clawd"));
        assert!(result.is_ok());
        assert!(registry.get(&BaseTypeId::new("rusty-clawd")).is_some());
    }

    #[test]
    fn register_terminal_shell_base_type_succeeds() {
        let mut registry = BaseTypeRegistry::default();
        let result = register_builtin_base_type(&mut registry, &BaseTypeId::new("terminal-shell"));
        assert!(result.is_ok());
        assert!(registry.get(&BaseTypeId::new("terminal-shell")).is_some());
    }
}
