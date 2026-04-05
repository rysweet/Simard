use std::ffi::OsString;
use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;

use crate::error::{SimardError, SimardResult};
use crate::runtime::RuntimeTopology;

/// How the bootstrap system resolves configuration values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapMode {
    /// Read all config from env vars / CLI flags (production default).
    ExplicitConfig,
    /// Use hardcoded defaults for testing and development.
    BuiltinDefaults,
}

impl BootstrapMode {
    pub(super) fn parse(raw: Option<String>) -> SimardResult<Self> {
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

/// Where a configuration value was resolved from.
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

/// A resolved config value paired with its provenance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigValue<T> {
    pub value: T,
    pub source: ConfigValueSource,
}

/// Raw inputs collected from CLI args or env vars before validation.
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

fn read_optional_utf8_env(key: &'static str) -> SimardResult<Option<String>> {
    match std::env::var_os(key) {
        None => Ok(None),
        Some(value) => decode_utf8_env_value(key, value),
    }
}

pub(super) fn decode_utf8_env_value(
    key: &'static str,
    value: OsString,
) -> SimardResult<Option<String>> {
    value
        .into_string()
        .map(Some)
        .map_err(|_| SimardError::NonUnicodeConfigValue {
            key: key.to_string(),
        })
}

pub(super) fn parse_runtime_topology(value: String) -> SimardResult<RuntimeTopology> {
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

#[cfg(test)]
mod tests {
    use crate::error::SimardError;

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

    // ── decode_utf8_env_value ──

    #[test]
    fn decode_utf8_env_value_valid_string() {
        use std::ffi::OsString;
        let result = super::decode_utf8_env_value("KEY", OsString::from("hello")).unwrap();
        assert_eq!(result, Some("hello".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn invalid_unicode_env_value_is_reported_explicitly() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let error =
            super::decode_utf8_env_value("SIMARD_IDENTITY", OsString::from_vec(vec![0x66, 0x80]))
                .expect_err("invalid unicode config should fail");

        assert_eq!(
            error,
            SimardError::NonUnicodeConfigValue {
                key: "SIMARD_IDENTITY".to_string(),
            }
        );
    }
}
