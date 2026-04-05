use std::path::{Path, PathBuf};

use super::types::{BootstrapMode, ConfigValue, ConfigValueSource, parse_runtime_topology};
use super::validation::validate_state_root;
use super::{DEFAULT_IDENTITY, DEFAULT_OBJECTIVE, DEFAULT_STATE_ROOT, LOCAL_BASE_TYPE};
use crate::base_types::BaseTypeId;
use crate::error::{SimardError, SimardResult};
use crate::runtime::RuntimeTopology;

use super::types::BootstrapInputs;

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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::BootstrapConfig;
    use crate::bootstrap::test_support::TestDir;
    use crate::bootstrap::{BootstrapInputs, BootstrapMode, ConfigValueSource};
    use crate::error::SimardError;
    use crate::runtime::RuntimeTopology;

    // ── BootstrapConfig::resolve ──

    #[test]
    fn resolve_builtin_defaults_produces_valid_config() {
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
}
