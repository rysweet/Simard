use std::ffi::OsString;
use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;
use std::sync::Arc;

use crate::base_types::{BaseTypeId, LocalProcessHarnessAdapter, RustyClawdAdapter};
use crate::error::{SimardError, SimardResult};
use crate::evidence::InMemoryEvidenceStore;
use crate::identity::{
    BuiltinIdentityLoader, IdentityLoadRequest, IdentityLoader, IdentityManifest, ManifestContract,
};
use crate::memory::InMemoryMemoryStore;
use crate::metadata::{Freshness, Provenance};
use crate::prompt_assets::FilePromptAssetStore;
use crate::reflection::{ReflectionSnapshot, ReflectiveRuntime};
use crate::runtime::{
    BaseTypeRegistry, LocalRuntime, RuntimePorts, RuntimeRequest, RuntimeTopology, SessionOutcome,
};
use crate::session::UuidSessionIdGenerator;

const DEFAULT_IDENTITY: &str = "simard-engineer";
const DEFAULT_OBJECTIVE: &str = "bootstrap the Simard engineer loop";
const LOCAL_BASE_TYPE: &str = "local-harness";
const RUSTY_CLAWD_BASE_TYPE: &str = "rusty-clawd";
const COPILOT_SDK_BASE_TYPE: &str = "copilot-sdk";

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
            mode: read_optional_utf8_env("SIMARD_BOOTSTRAP_MODE")?,
            identity: read_optional_utf8_env("SIMARD_IDENTITY")?,
            base_type: read_optional_utf8_env("SIMARD_BASE_TYPE")?,
            topology: read_optional_utf8_env("SIMARD_RUNTIME_TOPOLOGY")?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapConfig {
    pub mode: BootstrapMode,
    pub identity: String,
    pub prompt_root: ConfigValue<PathBuf>,
    pub objective: ConfigValue<String>,
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
            format!("objective:{}", self.objective.source),
        ]
    }
}

pub fn assemble_local_runtime(config: &BootstrapConfig) -> SimardResult<LocalRuntime> {
    let prompt_store = Arc::new(FilePromptAssetStore::new(config.prompt_root.value.clone()));
    let memory_store = Arc::new(InMemoryMemoryStore::try_default()?);
    let evidence_store = Arc::new(InMemoryEvidenceStore::try_default()?);

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
    let base_types = base_type_registry_for_manifest(&manifest)?;

    let request = RuntimeRequest::new(
        manifest,
        config.selected_base_type.value.clone(),
        config.topology.value,
    );

    LocalRuntime::compose(
        RuntimePorts::new(
            prompt_store,
            memory_store,
            evidence_store,
            base_types,
            Arc::new(UuidSessionIdGenerator),
        ),
        request,
    )
}

pub fn run_local_session(config: &BootstrapConfig) -> SimardResult<LocalSessionExecution> {
    let mut runtime = assemble_local_runtime(config)?;
    runtime.start()?;

    let outcome = runtime.run(config.objective.value.clone())?;
    let snapshot = runtime.snapshot()?;
    runtime.stop()?;
    let stopped_snapshot = runtime.snapshot()?;

    Ok(LocalSessionExecution {
        outcome,
        snapshot,
        stopped_snapshot,
    })
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

fn base_type_registry_for_manifest(manifest: &IdentityManifest) -> SimardResult<BaseTypeRegistry> {
    let mut base_types = BaseTypeRegistry::default();
    for base_type in &manifest.supported_base_types {
        register_builtin_base_type(&mut base_types, base_type)?;
    }
    Ok(base_types)
}

fn register_builtin_base_type(
    base_types: &mut BaseTypeRegistry,
    base_type: &BaseTypeId,
) -> SimardResult<()> {
    match base_type.as_str() {
        LOCAL_BASE_TYPE => {
            base_types.register(LocalProcessHarnessAdapter::single_process_alias(
                base_type.as_str(),
                LOCAL_BASE_TYPE,
            )?);
            Ok(())
        }
        RUSTY_CLAWD_BASE_TYPE => {
            base_types.register(RustyClawdAdapter::registered(base_type.as_str())?);
            Ok(())
        }
        COPILOT_SDK_BASE_TYPE => {
            base_types.register(LocalProcessHarnessAdapter::single_process_alias(
                base_type.as_str(),
                LOCAL_BASE_TYPE,
            )?);
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

    use super::{LOCAL_BASE_TYPE, base_type_registry_for_manifest, decode_utf8_env_value};
    use crate::base_types::{BaseTypeFactory, BaseTypeId, RustyClawdAdapter};
    use crate::error::SimardError;
    use crate::identity::{
        BuiltinIdentityLoader, IdentityLoadRequest, IdentityLoader, ManifestContract,
    };
    use crate::metadata::{Freshness, Provenance};

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

        let registry = base_type_registry_for_manifest(&manifest).expect("registry should build");
        let local = registry
            .get(&BaseTypeId::new("local-harness"))
            .expect("local harness should be registered");
        let rusty = registry
            .get(&BaseTypeId::new("rusty-clawd"))
            .expect("rusty-clawd should be registered");
        let copilot = registry
            .get(&BaseTypeId::new("copilot-sdk"))
            .expect("copilot-sdk should be registered");

        assert_eq!(local.descriptor().backend.identity, LOCAL_BASE_TYPE);
        assert_eq!(copilot.descriptor().backend.identity, LOCAL_BASE_TYPE);
        assert_eq!(
            rusty.descriptor().backend.identity,
            RustyClawdAdapter::registered("rusty-clawd")
                .expect("rusty-clawd adapter should initialize")
                .descriptor()
                .backend
                .identity
        );
    }
}
