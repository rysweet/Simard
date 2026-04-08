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
    use super::*;
    fn builtin_defaults_inputs() -> BootstrapInputs {
        BootstrapInputs {
            mode: Some("builtin-defaults".to_string()),
            prompt_root: None,
            objective: None,
            state_root: None,
            identity: None,
            base_type: None,
            topology: None,
        }
    }

    #[test]
    fn test_resolve_builtin_defaults() {
        let config = BootstrapConfig::resolve(builtin_defaults_inputs()).unwrap();
        assert_eq!(config.mode, BootstrapMode::BuiltinDefaults);
        assert_eq!(config.identity, DEFAULT_IDENTITY);
        assert_eq!(config.objective.value, DEFAULT_OBJECTIVE);
        assert_eq!(
            config.selected_base_type.value,
            BaseTypeId::new(LOCAL_BASE_TYPE)
        );
        assert_eq!(config.topology.value, RuntimeTopology::SingleProcess);
    }

    #[test]
    fn test_resolve_explicit_config_missing_prompt_root() {
        let inputs = BootstrapInputs {
            mode: Some("explicit-config".to_string()),
            prompt_root: None,
            objective: Some("obj".to_string()),
            state_root: None,
            identity: Some("id".to_string()),
            base_type: Some("bt".to_string()),
            topology: Some("single-process".to_string()),
        };
        let result = BootstrapConfig::resolve(inputs);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_explicit_config_missing_objective() {
        let inputs = BootstrapInputs {
            mode: Some("explicit-config".to_string()),
            prompt_root: Some(PathBuf::from("/some/root")),
            objective: None,
            state_root: None,
            identity: Some("id".to_string()),
            base_type: Some("bt".to_string()),
            topology: Some("single-process".to_string()),
        };
        let result = BootstrapConfig::resolve(inputs);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_explicit_config_missing_identity() {
        let inputs = BootstrapInputs {
            mode: Some("explicit-config".to_string()),
            prompt_root: Some(PathBuf::from("/some/root")),
            objective: Some("obj".to_string()),
            state_root: None,
            identity: None,
            base_type: Some("bt".to_string()),
            topology: Some("single-process".to_string()),
        };
        let result = BootstrapConfig::resolve(inputs);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_invalid_mode() {
        let inputs = BootstrapInputs {
            mode: Some("invalid-mode".to_string()),
            prompt_root: None,
            objective: None,
            state_root: None,
            identity: None,
            base_type: None,
            topology: None,
        };
        let result = BootstrapConfig::resolve(inputs);
        assert!(result.is_err());
    }

    #[test]
    fn test_manifest_precedence_contains_expected_keys() {
        let config = BootstrapConfig::resolve(builtin_defaults_inputs()).unwrap();
        let precedence = config.manifest_precedence();
        assert!(precedence.iter().any(|s| s.starts_with("mode:")));
        assert!(precedence.iter().any(|s| s.starts_with("identity:")));
        assert!(precedence.iter().any(|s| s.starts_with("base-type:")));
        assert!(precedence.iter().any(|s| s.starts_with("topology:")));
        assert!(precedence.iter().any(|s| s.starts_with("prompt-root:")));
        assert!(precedence.iter().any(|s| s.starts_with("state-root:")));
        assert!(precedence.iter().any(|s| s.starts_with("objective:")));
    }

    #[test]
    fn test_memory_store_path() {
        let config = BootstrapConfig::resolve(builtin_defaults_inputs()).unwrap();
        let path = config.memory_store_path();
        assert!(path.ends_with("memory_records.json"));
    }

    #[test]
    fn test_evidence_store_path() {
        let config = BootstrapConfig::resolve(builtin_defaults_inputs()).unwrap();
        let path = config.evidence_store_path();
        assert!(path.ends_with("evidence_records.json"));
    }

    #[test]
    fn test_goal_store_path() {
        let config = BootstrapConfig::resolve(builtin_defaults_inputs()).unwrap();
        let path = config.goal_store_path();
        assert!(path.ends_with("goal_records.json"));
    }

    #[test]
    fn test_handoff_store_path() {
        let config = BootstrapConfig::resolve(builtin_defaults_inputs()).unwrap();
        let path = config.handoff_store_path();
        assert!(path.ends_with("latest_handoff.json"));
    }

    #[test]
    fn test_state_root_path_returns_ref() {
        let config = BootstrapConfig::resolve(builtin_defaults_inputs()).unwrap();
        let state_root = config.state_root_path();
        assert_eq!(state_root, config.state_root.value.as_path());
    }

    #[test]
    fn test_resolve_with_explicit_topology_single_process() {
        let inputs = BootstrapInputs {
            mode: Some("builtin-defaults".to_string()),
            topology: Some("single-process".to_string()),
            ..builtin_defaults_inputs()
        };
        let config = BootstrapConfig::resolve(inputs).unwrap();
        assert_eq!(config.topology.value, RuntimeTopology::SingleProcess);
        assert_eq!(
            config.topology.source,
            ConfigValueSource::Environment("SIMARD_RUNTIME_TOPOLOGY")
        );
    }

    #[test]
    fn test_resolve_with_invalid_topology() {
        let inputs = BootstrapInputs {
            mode: Some("builtin-defaults".to_string()),
            topology: Some("invalid-topology".to_string()),
            ..builtin_defaults_inputs()
        };
        let result = BootstrapConfig::resolve(inputs);
        assert!(result.is_err());
    }
}
