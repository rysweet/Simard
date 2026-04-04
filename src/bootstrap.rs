use std::ffi::OsString;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

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

const DEFAULT_IDENTITY: &str = "simard-engineer";
const DEFAULT_OBJECTIVE: &str = "bootstrap the Simard engineer loop";
const DEFAULT_STATE_ROOT: &str = "target/simard-state";
const LOCAL_BASE_TYPE: &str = "local-harness";
const TERMINAL_SHELL_BASE_TYPE: &str = "terminal-shell";
const RUSTY_CLAWD_BASE_TYPE: &str = "rusty-clawd";
const COPILOT_SDK_BASE_TYPE: &str = "copilot-sdk";
const CLAUDE_AGENT_SDK_BASE_TYPE: &str = "claude-agent-sdk";
const MS_AGENT_FRAMEWORK_BASE_TYPE: &str = "ms-agent-framework";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapMode {
    ExplicitConfig,
    BuiltinDefaults,
}

impl BootstrapMode {
    fn parse(raw: Option<String>) -> SimardResult<Self> {
        match raw.as_deref() {
            None => Ok(Self::ExplicitConfig),
            Some("explicit-config") => Ok(Self::ExplicitConfig),
            Some("builtin-defaults") => Ok(Self::BuiltinDefaults),
            Some(value) => Err(SimardError::InvalidConfigValue {
                key: "SIMARD_BOOTSTRAP_MODE".to_string(),
                value: value.to_string(),
                help: "expected 'explicit-config' or 'builtin-defaults'".to_string(),
            }),
        }
    }
}

impl Display for BootstrapMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::ExplicitConfig => "explicit-config",
            Self::BuiltinDefaults => "builtin-defaults",
        };
        f.write_str(label)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigValueSource {
    Environment(&'static str),
    ExplicitOptIn(&'static str),
}

impl Display for ConfigValueSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment(key) => write!(f, "env:{key}"),
            Self::ExplicitOptIn(key) => write!(f, "opt-in:{key}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigValue<T> {
    pub value: T,
    pub source: ConfigValueSource,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BootstrapInputs {
    pub prompt_root: Option<PathBuf>,
    pub objective: Option<String>,
    pub state_root: Option<PathBuf>,
    pub mode: Option<String>,
    pub identity: Option<String>,
    pub base_type: Option<String>,
    pub topology: Option<String>,
}

impl BootstrapInputs {
    pub fn from_env() -> SimardResult<Self> {
        Ok(Self {
            prompt_root: std::env::var_os("SIMARD_PROMPT_ROOT").map(PathBuf::from),
            objective: read_optional_utf8_env("SIMARD_OBJECTIVE")?,
            state_root: std::env::var_os("SIMARD_STATE_ROOT").map(PathBuf::from),
            mode: read_optional_utf8_env("SIMARD_BOOTSTRAP_MODE")?,
            identity: read_optional_utf8_env("SIMARD_IDENTITY")?,
            base_type: read_optional_utf8_env("SIMARD_BASE_TYPE")?,
            topology: read_optional_utf8_env("SIMARD_RUNTIME_TOPOLOGY")?,
        })
    }
}

pub(crate) fn validate_state_root(path: impl AsRef<Path>) -> SimardResult<PathBuf> {
    let raw_path = path.as_ref();
    if raw_path.as_os_str().is_empty() {
        return Err(SimardError::InvalidStateRoot {
            path: raw_path.to_path_buf(),
            reason: "state root must not be empty".to_string(),
        });
    }

    if raw_path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(SimardError::InvalidStateRoot {
            path: raw_path.to_path_buf(),
            reason: "state root must not contain '..' path segments".to_string(),
        });
    }

    let absolute_path = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| SimardError::InvalidStateRoot {
                path: raw_path.to_path_buf(),
                reason: format!("current working directory could not be resolved: {error}"),
            })?
            .join(raw_path)
    };

    let (existing_root, missing_segments) = split_existing_prefix(&absolute_path)?;
    let metadata =
        fs::symlink_metadata(&existing_root).map_err(|error| SimardError::InvalidStateRoot {
            path: raw_path.to_path_buf(),
            reason: format!(
                "existing state root ancestor '{}' could not be inspected: {error}",
                existing_root.display()
            ),
        })?;

    if metadata.file_type().is_symlink() {
        return Err(SimardError::InvalidStateRoot {
            path: raw_path.to_path_buf(),
            reason: "state root must not be a symlink".to_string(),
        });
    }
    if !metadata.is_dir() {
        return Err(SimardError::InvalidStateRoot {
            path: raw_path.to_path_buf(),
            reason: "state root must resolve to a directory".to_string(),
        });
    }

    let mut canonical =
        fs::canonicalize(&existing_root).map_err(|error| SimardError::InvalidStateRoot {
            path: raw_path.to_path_buf(),
            reason: format!(
                "state root ancestor '{}' could not be canonicalized: {error}",
                existing_root.display()
            ),
        })?;
    for segment in missing_segments {
        canonical.push(segment);
    }

    Ok(canonical)
}

fn split_existing_prefix(path: &Path) -> SimardResult<(PathBuf, Vec<OsString>)> {
    let mut existing = path.to_path_buf();
    let mut missing_segments = Vec::new();

    loop {
        match fs::symlink_metadata(&existing) {
            Ok(_) => {
                missing_segments.reverse();
                return Ok((existing, missing_segments));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let segment =
                    existing
                        .file_name()
                        .ok_or_else(|| SimardError::InvalidStateRoot {
                            path: path.to_path_buf(),
                            reason: "state root must stay under an existing directory".to_string(),
                        })?;
                missing_segments.push(segment.to_os_string());
                existing = existing
                    .parent()
                    .ok_or_else(|| SimardError::InvalidStateRoot {
                        path: path.to_path_buf(),
                        reason: "state root must stay under an existing directory".to_string(),
                    })?
                    .to_path_buf();
            }
            Err(error) => {
                return Err(SimardError::InvalidStateRoot {
                    path: path.to_path_buf(),
                    reason: format!(
                        "state root '{}' could not be inspected: {error}",
                        existing.display()
                    ),
                });
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapConfig {
    pub mode: BootstrapMode,
    pub identity: String,
    pub prompt_root: ConfigValue<PathBuf>,
    pub objective: ConfigValue<String>,
    pub state_root: ConfigValue<PathBuf>,
    pub selected_base_type: ConfigValue<BaseTypeId>,
    pub topology: ConfigValue<RuntimeTopology>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalSessionExecution {
    pub outcome: SessionOutcome,
    pub snapshot: ReflectionSnapshot,
    pub stopped_snapshot: ReflectionSnapshot,
}

impl BootstrapConfig {
    pub fn from_env() -> SimardResult<Self> {
        Self::resolve(BootstrapInputs::from_env()?)
    }

    pub fn resolve(inputs: BootstrapInputs) -> SimardResult<Self> {
        let mode = BootstrapMode::parse(inputs.mode)?;
        let prompt_root = match inputs.prompt_root {
            Some(path) => ConfigValue {
                value: path,
                source: ConfigValueSource::Environment("SIMARD_PROMPT_ROOT"),
            },
            None if mode == BootstrapMode::BuiltinDefaults => ConfigValue {
                value: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets"),
                source: ConfigValueSource::ExplicitOptIn("SIMARD_BOOTSTRAP_MODE"),
            },
            None => {
                return Err(SimardError::MissingRequiredConfig {
                    key: "SIMARD_PROMPT_ROOT".to_string(),
                    help: "set SIMARD_PROMPT_ROOT or opt in with SIMARD_BOOTSTRAP_MODE=builtin-defaults"
                        .to_string(),
                });
            }
        };

        let objective = match inputs.objective {
            Some(value) => ConfigValue {
                value,
                source: ConfigValueSource::Environment("SIMARD_OBJECTIVE"),
            },
            None if mode == BootstrapMode::BuiltinDefaults => ConfigValue {
                value: DEFAULT_OBJECTIVE.to_string(),
                source: ConfigValueSource::ExplicitOptIn("SIMARD_BOOTSTRAP_MODE"),
            },
            None => {
                return Err(SimardError::MissingRequiredConfig {
                    key: "SIMARD_OBJECTIVE".to_string(),
                    help:
                        "set SIMARD_OBJECTIVE or opt in with SIMARD_BOOTSTRAP_MODE=builtin-defaults"
                            .to_string(),
                });
            }
        };
        let selected_base_type = match inputs.base_type {
            Some(value) => ConfigValue {
                value: BaseTypeId::new(value),
                source: ConfigValueSource::Environment("SIMARD_BASE_TYPE"),
            },
            None if mode == BootstrapMode::BuiltinDefaults => ConfigValue {
                value: BaseTypeId::new(LOCAL_BASE_TYPE),
                source: ConfigValueSource::ExplicitOptIn("SIMARD_BOOTSTRAP_MODE"),
            },
            None => {
                return Err(SimardError::MissingRequiredConfig {
                    key: "SIMARD_BASE_TYPE".to_string(),
                    help:
                        "set SIMARD_BASE_TYPE or opt in with SIMARD_BOOTSTRAP_MODE=builtin-defaults"
                            .to_string(),
                });
            }
        };

        let topology = match inputs.topology {
            Some(value) => ConfigValue {
                value: parse_runtime_topology(value)?,
                source: ConfigValueSource::Environment("SIMARD_RUNTIME_TOPOLOGY"),
            },
            None if mode == BootstrapMode::BuiltinDefaults => ConfigValue {
                value: RuntimeTopology::SingleProcess,
                source: ConfigValueSource::ExplicitOptIn("SIMARD_BOOTSTRAP_MODE"),
            },
            None => {
                return Err(SimardError::MissingRequiredConfig {
                    key: "SIMARD_RUNTIME_TOPOLOGY".to_string(),
                    help: "set SIMARD_RUNTIME_TOPOLOGY or opt in with SIMARD_BOOTSTRAP_MODE=builtin-defaults"
                        .to_string(),
                });
            }
        };
        let state_root = match inputs.state_root {
            Some(path) => ConfigValue {
                value: validate_state_root(path)?,
                source: ConfigValueSource::Environment("SIMARD_STATE_ROOT"),
            },
            None if mode == BootstrapMode::BuiltinDefaults => ConfigValue {
                value: validate_state_root(
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_STATE_ROOT),
                )?,
                source: ConfigValueSource::ExplicitOptIn("SIMARD_BOOTSTRAP_MODE"),
            },
            None => {
                return Err(SimardError::MissingRequiredConfig {
                    key: "SIMARD_STATE_ROOT".to_string(),
                    help: "set SIMARD_STATE_ROOT or opt in with SIMARD_BOOTSTRAP_MODE=builtin-defaults"
                        .to_string(),
                });
            }
        };

        Ok(Self {
            mode,
            identity: match inputs.identity {
                Some(value) => value,
                None if mode == BootstrapMode::BuiltinDefaults => DEFAULT_IDENTITY.to_string(),
                None => {
                    return Err(SimardError::MissingRequiredConfig {
                        key: "SIMARD_IDENTITY".to_string(),
                        help:
                            "set SIMARD_IDENTITY or opt in with SIMARD_BOOTSTRAP_MODE=builtin-defaults"
                                .to_string(),
                    });
                }
            },
            prompt_root,
            objective,
            state_root,
            selected_base_type,
            topology,
        })
    }

    pub fn manifest_precedence(&self) -> Vec<String> {
        vec![
            format!("mode:{}", self.mode),
            format!("identity:{}", self.identity),
            format!("base-type:{}", self.selected_base_type.value),
            format!("topology:{}", self.topology.value),
            format!("prompt-root:{}", self.prompt_root.source),
            format!("state-root:{}", self.state_root.source),
            format!("objective:{}", self.objective.source),
        ]
    }
}

/// Build the memory store, attempting cognitive bridge first with file fallback.
///
/// Only launches the memory bridge — knowledge and gym bridges are launched
/// on-demand by subsystems that need them, avoiding unnecessary subprocess spawns.
fn build_memory_store(config: &BootstrapConfig) -> SimardResult<Arc<dyn MemoryStore>> {
    let bridge = find_python_dir().ok().and_then(|python_dir| {
        let db_path = cognitive_memory_db_path(&config.state_root.value);
        launch_memory_bridge(&config.identity, &db_path, &python_dir).ok()
    });

    if let Some(bridge) = bridge {
        eprintln!("[simard] cognitive memory bridge active — using Kuzu backend");
        let store = CognitiveBridgeMemoryStore::new(bridge, config.memory_store_path())?;
        // Pull cross-session records from the cognitive bridge to supplement
        // local fallback hydration — recovers data written by other processes.
        store.hydrate_from_bridge();
        Ok(Arc::new(store))
    } else {
        eprintln!("[simard] cognitive memory bridge unavailable — using JSON file backend");
        Ok(Arc::new(FileBackedMemoryStore::try_new(
            config.memory_store_path(),
        )?))
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
        bootstrap_entrypoint(),
        "bootstrap-config -> identity-loader -> runtime-ports -> local-runtime",
        config.manifest_precedence(),
        Provenance::new(
            "bootstrap",
            format!("{}:{}", bootstrap_entrypoint(), config.identity),
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

pub fn bootstrap_entrypoint() -> &'static str {
    concat!(module_path!(), "::assemble_local_runtime")
}

fn read_optional_utf8_env(key: &'static str) -> SimardResult<Option<String>> {
    match std::env::var_os(key) {
        None => Ok(None),
        Some(value) => decode_utf8_env_value(key, value),
    }
}

fn decode_utf8_env_value(key: &'static str, value: OsString) -> SimardResult<Option<String>> {
    value
        .into_string()
        .map(Some)
        .map_err(|_| SimardError::NonUnicodeConfigValue {
            key: key.to_string(),
        })
}

fn parse_runtime_topology(value: String) -> SimardResult<RuntimeTopology> {
    match value.as_str() {
        "single-process" => Ok(RuntimeTopology::SingleProcess),
        "multi-process" => Ok(RuntimeTopology::MultiProcess),
        "distributed" => Ok(RuntimeTopology::Distributed),
        _ => Err(SimardError::InvalidConfigValue {
            key: "SIMARD_RUNTIME_TOPOLOGY".to_string(),
            value,
            help: "expected 'single-process', 'multi-process', or 'distributed'".to_string(),
        }),
    }
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

impl BootstrapConfig {
    pub fn memory_store_path(&self) -> PathBuf {
        self.state_root.value.join("memory_records.json")
    }

    pub fn evidence_store_path(&self) -> PathBuf {
        self.state_root.value.join("evidence_records.json")
    }

    pub fn goal_store_path(&self) -> PathBuf {
        self.state_root.value.join("goal_records.json")
    }

    pub fn handoff_store_path(&self) -> PathBuf {
        self.state_root.value.join("latest_handoff.json")
    }

    pub fn state_root_path(&self) -> &Path {
        &self.state_root.value
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
    #[cfg(unix)]
    use std::ffi::OsString;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        LOCAL_BASE_TYPE, builtin_base_type_registry_for_manifest, decode_utf8_env_value,
        validate_state_root,
    };
    use crate::base_type_rustyclawd::RustyClawdAdapter;
    use crate::base_types::{BaseTypeFactory, BaseTypeId};
    use crate::error::SimardError;
    use crate::identity::{
        BuiltinIdentityLoader, IdentityLoadRequest, IdentityLoader, ManifestContract,
    };
    use crate::metadata::{Freshness, Provenance};

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(label: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("{label}-{unique}"));
            fs::create_dir_all(&path).expect("test directory should be created");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[cfg(unix)]
    #[test]
    fn invalid_unicode_env_value_is_reported_explicitly() {
        let error = decode_utf8_env_value("SIMARD_IDENTITY", OsString::from_vec(vec![0x66, 0x80]))
            .expect_err("invalid unicode config should fail");

        assert_eq!(
            error,
            SimardError::NonUnicodeConfigValue {
                key: "SIMARD_IDENTITY".to_string(),
            }
        );
    }

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

    #[test]
    fn validate_state_root_rejects_parent_directory_segments() {
        let error = validate_state_root(PathBuf::from("../outside-state"))
            .expect_err("state root traversal should fail");

        assert_eq!(
            error,
            SimardError::InvalidStateRoot {
                path: PathBuf::from("../outside-state"),
                reason: "state root must not contain '..' path segments".to_string(),
            }
        );
    }

    #[test]
    fn validate_state_root_canonicalizes_safe_existing_directories() {
        let temp_dir = TestDir::new("simard-state-root");
        let nested = temp_dir.path().join("nested");
        fs::create_dir_all(&nested).expect("nested directory should exist");

        let resolved =
            validate_state_root(nested.clone()).expect("existing state root should pass");
        let expected = fs::canonicalize(&nested).expect("existing state root should canonicalize");

        assert_eq!(resolved, expected);
    }

    #[test]
    fn validate_state_root_preserves_missing_segment_order() {
        let temp_dir = TestDir::new("simard-state-root-order");
        let requested = temp_dir.path().join("level1").join("level2").join("level3");

        let resolved =
            validate_state_root(requested).expect("missing state root path should resolve safely");
        let expected = fs::canonicalize(temp_dir.path())
            .expect("existing ancestor should canonicalize")
            .join("level1")
            .join("level2")
            .join("level3");

        assert_eq!(resolved, expected);
    }

    #[test]
    fn validate_state_root_rejects_existing_files() {
        let temp_dir = TestDir::new("simard-state-root-file");
        let file_path = temp_dir.path().join("state-root.txt");
        fs::write(&file_path, "not a directory").expect("file should be written");

        let error =
            validate_state_root(file_path.clone()).expect_err("state root file should fail");

        assert_eq!(
            error,
            SimardError::InvalidStateRoot {
                path: file_path,
                reason: "state root must resolve to a directory".to_string(),
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn validate_state_root_rejects_symlink_roots() {
        use std::os::unix::fs::symlink;

        let temp_dir = TestDir::new("simard-state-root-symlink");
        let real_dir = temp_dir.path().join("real");
        let link_dir = temp_dir.path().join("link");
        fs::create_dir_all(&real_dir).expect("real directory should exist");
        symlink(&real_dir, &link_dir).expect("symlink should be created");

        let error =
            validate_state_root(link_dir.clone()).expect_err("symlink state root should fail");

        assert_eq!(
            error,
            SimardError::InvalidStateRoot {
                path: link_dir,
                reason: "state root must not be a symlink".to_string(),
            }
        );
    }

    // ── BootstrapMode::parse ──

    #[test]
    fn bootstrap_mode_parse_none_defaults_to_explicit_config() {
        use super::BootstrapMode;
        let mode = BootstrapMode::parse(None).unwrap();
        assert_eq!(mode, BootstrapMode::ExplicitConfig);
    }

    #[test]
    fn bootstrap_mode_parse_explicit_config() {
        use super::BootstrapMode;
        let mode = BootstrapMode::parse(Some("explicit-config".to_string())).unwrap();
        assert_eq!(mode, BootstrapMode::ExplicitConfig);
    }

    #[test]
    fn bootstrap_mode_parse_builtin_defaults() {
        use super::BootstrapMode;
        let mode = BootstrapMode::parse(Some("builtin-defaults".to_string())).unwrap();
        assert_eq!(mode, BootstrapMode::BuiltinDefaults);
    }

    #[test]
    fn bootstrap_mode_parse_invalid_value() {
        use super::BootstrapMode;
        let err = BootstrapMode::parse(Some("invalid".to_string())).unwrap_err();
        match err {
            SimardError::InvalidConfigValue { key, value, .. } => {
                assert_eq!(key, "SIMARD_BOOTSTRAP_MODE");
                assert_eq!(value, "invalid");
            }
            other => panic!("expected InvalidConfigValue, got {other:?}"),
        }
    }

    // ── BootstrapMode Display ──

    #[test]
    fn bootstrap_mode_display_explicit_config() {
        use super::BootstrapMode;
        assert_eq!(BootstrapMode::ExplicitConfig.to_string(), "explicit-config");
    }

    #[test]
    fn bootstrap_mode_display_builtin_defaults() {
        use super::BootstrapMode;
        assert_eq!(
            BootstrapMode::BuiltinDefaults.to_string(),
            "builtin-defaults"
        );
    }

    // ── ConfigValueSource Display ──

    #[test]
    fn config_value_source_display_environment() {
        use super::ConfigValueSource;
        let source = ConfigValueSource::Environment("MY_VAR");
        assert_eq!(source.to_string(), "env:MY_VAR");
    }

    #[test]
    fn config_value_source_display_explicit_opt_in() {
        use super::ConfigValueSource;
        let source = ConfigValueSource::ExplicitOptIn("OPT_KEY");
        assert_eq!(source.to_string(), "opt-in:OPT_KEY");
    }

    // ── parse_runtime_topology ──

    #[test]
    fn parse_runtime_topology_single_process() {
        use super::parse_runtime_topology;
        use crate::runtime::RuntimeTopology;
        let topo = parse_runtime_topology("single-process".to_string()).unwrap();
        assert_eq!(topo, RuntimeTopology::SingleProcess);
    }

    #[test]
    fn parse_runtime_topology_multi_process() {
        use super::parse_runtime_topology;
        use crate::runtime::RuntimeTopology;
        let topo = parse_runtime_topology("multi-process".to_string()).unwrap();
        assert_eq!(topo, RuntimeTopology::MultiProcess);
    }

    #[test]
    fn parse_runtime_topology_distributed() {
        use super::parse_runtime_topology;
        use crate::runtime::RuntimeTopology;
        let topo = parse_runtime_topology("distributed".to_string()).unwrap();
        assert_eq!(topo, RuntimeTopology::Distributed);
    }

    #[test]
    fn parse_runtime_topology_invalid() {
        use super::parse_runtime_topology;
        let err = parse_runtime_topology("mesh".to_string()).unwrap_err();
        match err {
            SimardError::InvalidConfigValue { key, value, .. } => {
                assert_eq!(key, "SIMARD_RUNTIME_TOPOLOGY");
                assert_eq!(value, "mesh");
            }
            other => panic!("expected InvalidConfigValue, got {other:?}"),
        }
    }

    // ── BootstrapInputs default ──

    #[test]
    fn bootstrap_inputs_default_all_none() {
        use super::BootstrapInputs;
        let inputs = BootstrapInputs::default();
        assert!(inputs.prompt_root.is_none());
        assert!(inputs.objective.is_none());
        assert!(inputs.state_root.is_none());
        assert!(inputs.mode.is_none());
        assert!(inputs.identity.is_none());
        assert!(inputs.base_type.is_none());
        assert!(inputs.topology.is_none());
    }

    // ── BootstrapConfig::resolve ──

    #[test]
    fn resolve_builtin_defaults_produces_valid_config() {
        use super::{BootstrapConfig, BootstrapInputs, BootstrapMode};
        let inputs = BootstrapInputs {
            mode: Some("builtin-defaults".to_string()),
            ..Default::default()
        };
        let config = BootstrapConfig::resolve(inputs).unwrap();
        assert_eq!(config.mode, BootstrapMode::BuiltinDefaults);
        assert_eq!(config.identity, "simard-engineer");
        assert_eq!(config.selected_base_type.value.as_str(), "local-harness");
    }

    #[test]
    fn resolve_explicit_config_without_prompt_root_fails() {
        use super::{BootstrapConfig, BootstrapInputs};
        let inputs = BootstrapInputs {
            mode: Some("explicit-config".to_string()),
            ..Default::default()
        };
        let err = BootstrapConfig::resolve(inputs).unwrap_err();
        match err {
            SimardError::MissingRequiredConfig { key, .. } => {
                assert_eq!(key, "SIMARD_PROMPT_ROOT");
            }
            other => panic!("expected MissingRequiredConfig, got {other:?}"),
        }
    }

    #[test]
    fn resolve_explicit_config_without_objective_fails() {
        use super::{BootstrapConfig, BootstrapInputs};
        let inputs = BootstrapInputs {
            mode: Some("explicit-config".to_string()),
            prompt_root: Some(PathBuf::from("/some/path")),
            ..Default::default()
        };
        let err = BootstrapConfig::resolve(inputs).unwrap_err();
        match err {
            SimardError::MissingRequiredConfig { key, .. } => {
                assert_eq!(key, "SIMARD_OBJECTIVE");
            }
            other => panic!("expected MissingRequiredConfig, got {other:?}"),
        }
    }

    #[test]
    fn resolve_explicit_config_without_base_type_fails() {
        use super::{BootstrapConfig, BootstrapInputs};
        let inputs = BootstrapInputs {
            mode: Some("explicit-config".to_string()),
            prompt_root: Some(PathBuf::from("/some/path")),
            objective: Some("test".to_string()),
            ..Default::default()
        };
        let err = BootstrapConfig::resolve(inputs).unwrap_err();
        match err {
            SimardError::MissingRequiredConfig { key, .. } => {
                assert_eq!(key, "SIMARD_BASE_TYPE");
            }
            other => panic!("expected MissingRequiredConfig, got {other:?}"),
        }
    }

    #[test]
    fn resolve_explicit_config_without_topology_fails() {
        use super::{BootstrapConfig, BootstrapInputs};
        let inputs = BootstrapInputs {
            mode: Some("explicit-config".to_string()),
            prompt_root: Some(PathBuf::from("/some/path")),
            objective: Some("test".to_string()),
            base_type: Some("local-harness".to_string()),
            ..Default::default()
        };
        let err = BootstrapConfig::resolve(inputs).unwrap_err();
        match err {
            SimardError::MissingRequiredConfig { key, .. } => {
                assert_eq!(key, "SIMARD_RUNTIME_TOPOLOGY");
            }
            other => panic!("expected MissingRequiredConfig, got {other:?}"),
        }
    }

    #[test]
    fn resolve_explicit_config_without_state_root_fails() {
        use super::{BootstrapConfig, BootstrapInputs};
        let inputs = BootstrapInputs {
            mode: Some("explicit-config".to_string()),
            prompt_root: Some(PathBuf::from("/some/path")),
            objective: Some("test".to_string()),
            base_type: Some("local-harness".to_string()),
            topology: Some("single-process".to_string()),
            ..Default::default()
        };
        let err = BootstrapConfig::resolve(inputs).unwrap_err();
        match err {
            SimardError::MissingRequiredConfig { key, .. } => {
                assert_eq!(key, "SIMARD_STATE_ROOT");
            }
            other => panic!("expected MissingRequiredConfig, got {other:?}"),
        }
    }

    #[test]
    fn resolve_explicit_config_without_identity_fails() {
        use super::{BootstrapConfig, BootstrapInputs};
        let temp_dir = TestDir::new("simard-resolve-test");
        let inputs = BootstrapInputs {
            mode: Some("explicit-config".to_string()),
            prompt_root: Some(PathBuf::from("/some/path")),
            objective: Some("test".to_string()),
            base_type: Some("local-harness".to_string()),
            topology: Some("single-process".to_string()),
            state_root: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };
        let err = BootstrapConfig::resolve(inputs).unwrap_err();
        match err {
            SimardError::MissingRequiredConfig { key, .. } => {
                assert_eq!(key, "SIMARD_IDENTITY");
            }
            other => panic!("expected MissingRequiredConfig, got {other:?}"),
        }
    }

    #[test]
    fn resolve_full_explicit_config_succeeds() {
        use super::{BootstrapConfig, BootstrapInputs, BootstrapMode, ConfigValueSource};
        use crate::runtime::RuntimeTopology;
        let temp_dir = TestDir::new("simard-resolve-full");
        let inputs = BootstrapInputs {
            mode: Some("explicit-config".to_string()),
            prompt_root: Some(PathBuf::from("/some/path")),
            objective: Some("my-objective".to_string()),
            base_type: Some("local-harness".to_string()),
            topology: Some("single-process".to_string()),
            state_root: Some(temp_dir.path().to_path_buf()),
            identity: Some("my-identity".to_string()),
        };
        let config = BootstrapConfig::resolve(inputs).unwrap();
        assert_eq!(config.mode, BootstrapMode::ExplicitConfig);
        assert_eq!(config.identity, "my-identity");
        assert_eq!(config.objective.value, "my-objective");
        assert_eq!(config.selected_base_type.value.as_str(), "local-harness");
        assert_eq!(config.topology.value, RuntimeTopology::SingleProcess);
        assert_eq!(config.prompt_root.value, PathBuf::from("/some/path"));
        assert!(matches!(
            config.prompt_root.source,
            ConfigValueSource::Environment(_)
        ));
        assert!(matches!(
            config.objective.source,
            ConfigValueSource::Environment(_)
        ));
    }

    // ── BootstrapConfig path methods ──

    #[test]
    fn config_path_methods_use_state_root() {
        use super::{BootstrapConfig, BootstrapInputs};
        let temp_dir = TestDir::new("simard-paths-test");
        let inputs = BootstrapInputs {
            mode: Some("builtin-defaults".to_string()),
            state_root: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };
        let config = BootstrapConfig::resolve(inputs).unwrap();
        assert!(
            config
                .memory_store_path()
                .to_string_lossy()
                .contains("memory_records.json")
        );
        assert!(
            config
                .evidence_store_path()
                .to_string_lossy()
                .contains("evidence_records.json")
        );
        assert!(
            config
                .goal_store_path()
                .to_string_lossy()
                .contains("goal_records.json")
        );
        assert!(
            config
                .handoff_store_path()
                .to_string_lossy()
                .contains("latest_handoff.json")
        );
    }

    #[test]
    fn state_root_path_returns_state_root_ref() {
        use super::{BootstrapConfig, BootstrapInputs};
        let inputs = BootstrapInputs {
            mode: Some("builtin-defaults".to_string()),
            ..Default::default()
        };
        let config = BootstrapConfig::resolve(inputs).unwrap();
        assert!(!config.state_root_path().as_os_str().is_empty());
    }

    // ── manifest_precedence ──

    #[test]
    fn manifest_precedence_returns_expected_entries() {
        use super::{BootstrapConfig, BootstrapInputs};
        let inputs = BootstrapInputs {
            mode: Some("builtin-defaults".to_string()),
            ..Default::default()
        };
        let config = BootstrapConfig::resolve(inputs).unwrap();
        let prec = config.manifest_precedence();
        assert!(prec.len() >= 7);
        assert!(prec.iter().any(|s| s.starts_with("mode:")));
        assert!(prec.iter().any(|s| s.starts_with("identity:")));
        assert!(prec.iter().any(|s| s.starts_with("base-type:")));
        assert!(prec.iter().any(|s| s.starts_with("topology:")));
        assert!(prec.iter().any(|s| s.starts_with("prompt-root:")));
        assert!(prec.iter().any(|s| s.starts_with("state-root:")));
        assert!(prec.iter().any(|s| s.starts_with("objective:")));
    }

    // ── bootstrap_entrypoint ──

    #[test]
    fn bootstrap_entrypoint_contains_module_path() {
        use super::bootstrap_entrypoint;
        let entry = bootstrap_entrypoint();
        assert!(entry.contains("bootstrap"));
        assert!(entry.contains("assemble_local_runtime"));
    }

    // ── decode_utf8_env_value ──

    #[test]
    fn decode_utf8_env_value_valid_string() {
        use std::ffi::OsString;
        let result = decode_utf8_env_value("KEY", OsString::from("hello")).unwrap();
        assert_eq!(result, Some("hello".to_string()));
    }

    // ── validate_state_root ──

    #[test]
    fn validate_state_root_rejects_empty_path() {
        let err = validate_state_root("").unwrap_err();
        assert_eq!(
            err,
            SimardError::InvalidStateRoot {
                path: PathBuf::from(""),
                reason: "state root must not be empty".to_string(),
            }
        );
    }

    #[test]
    fn validate_state_root_rejects_double_parent_traversal() {
        let err = validate_state_root("a/../../outside").unwrap_err();
        match err {
            SimardError::InvalidStateRoot { reason, .. } => {
                assert!(reason.contains(".."));
            }
            other => panic!("expected InvalidStateRoot, got {other:?}"),
        }
    }

    // ── register_builtin_base_type ──

    #[test]
    fn register_unknown_base_type_does_not_error() {
        use super::register_builtin_base_type;
        use crate::base_types::BaseTypeId;
        use crate::runtime::BaseTypeRegistry;
        let mut registry = BaseTypeRegistry::default();
        let result = register_builtin_base_type(&mut registry, &BaseTypeId::new("nonexistent"));
        assert!(
            result.is_ok(),
            "unknown base type should be silently ignored"
        );
    }

    #[test]
    fn register_local_harness_base_type_succeeds() {
        use super::register_builtin_base_type;
        use crate::base_types::BaseTypeId;
        use crate::runtime::BaseTypeRegistry;
        let mut registry = BaseTypeRegistry::default();
        let result = register_builtin_base_type(&mut registry, &BaseTypeId::new("local-harness"));
        assert!(result.is_ok());
        assert!(registry.get(&BaseTypeId::new("local-harness")).is_some());
    }

    #[test]
    fn register_rusty_clawd_base_type_succeeds() {
        use super::register_builtin_base_type;
        use crate::base_types::BaseTypeId;
        use crate::runtime::BaseTypeRegistry;
        let mut registry = BaseTypeRegistry::default();
        let result = register_builtin_base_type(&mut registry, &BaseTypeId::new("rusty-clawd"));
        assert!(result.is_ok());
        assert!(registry.get(&BaseTypeId::new("rusty-clawd")).is_some());
    }

    #[test]
    fn register_terminal_shell_base_type_succeeds() {
        use super::register_builtin_base_type;
        use crate::base_types::BaseTypeId;
        use crate::runtime::BaseTypeRegistry;
        let mut registry = BaseTypeRegistry::default();
        let result = register_builtin_base_type(&mut registry, &BaseTypeId::new("terminal-shell"));
        assert!(result.is_ok());
        assert!(registry.get(&BaseTypeId::new("terminal-shell")).is_some());
    }
}
