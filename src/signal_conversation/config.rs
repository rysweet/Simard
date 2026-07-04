//! Signal channel configuration (the `[signal]` table of the runtime config).
//!
//! Resolution follows the existing runtime-config rule: environment wins, then
//! the config file, then a clear error — never a silent default. When the
//! `signal` feature is off, this module is not compiled and the `[signal]` table
//! is ignored.

use std::path::Path;

use serde::Deserialize;

use crate::error::{SimardError, SimardResult};

/// Typed view of the `[signal]` configuration table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalConfig {
    /// signal-cli JSON-RPC daemon endpoint (`host:port`).
    pub endpoint: String,
    /// The Signal account signal-cli owns — a linked device or dedicated number.
    pub account: String,
    /// E.164 operator numbers permitted to COMMAND Simard. Fail-closed: empty
    /// means nobody may command.
    pub allowlist: Vec<String>,
    /// If true, non-allowlisted senders may receive READ-ONLY results (e.g.
    /// `status`) but can never trigger a mutation. Default false.
    pub read_only_unknown: bool,
}

pub const ENV_ENDPOINT: &str = "SIMARD_SIGNAL_ENDPOINT";
pub const ENV_ACCOUNT: &str = "SIMARD_SIGNAL_ACCOUNT";
pub const ENV_ALLOWLIST: &str = "SIMARD_SIGNAL_ALLOWLIST";
pub const ENV_READ_ONLY_UNKNOWN: &str = "SIMARD_SIGNAL_READ_ONLY_UNKNOWN";

/// The `[signal]` table as read from `config.toml` (every field optional so env
/// can supply or override it).
#[derive(Debug, Default, Deserialize)]
struct SignalTable {
    endpoint: Option<String>,
    account: Option<String>,
    allowlist: Option<Vec<String>>,
    read_only_unknown: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct RootConfig {
    signal: Option<SignalTable>,
}

impl SignalConfig {
    /// Load from the default state root (`$SIMARD_STATE_ROOT` or `~/.simard`).
    pub fn load() -> SimardResult<Self> {
        Self::load_from(&crate::state_root::simard_state_root())
    }

    /// Load the `[signal]` config, resolving each field env-first, then the
    /// `config.toml` `[signal]` table, then failing for a missing required key.
    /// `endpoint` and `account` are required; `allowlist` defaults to empty
    /// (fail-closed) and `read_only_unknown` to false.
    pub fn load_from(state_root: &Path) -> SimardResult<Self> {
        let table = read_signal_table(state_root)?;

        let endpoint = resolve_string(ENV_ENDPOINT, table.endpoint, "endpoint")?;
        let account = resolve_string(ENV_ACCOUNT, table.account, "account")?;

        let allowlist = match std::env::var(ENV_ALLOWLIST) {
            Ok(v) => v
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            Err(_) => table.allowlist.unwrap_or_default(),
        };

        let read_only_unknown = match std::env::var(ENV_READ_ONLY_UNKNOWN) {
            Ok(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"),
            Err(_) => table.read_only_unknown.unwrap_or(false),
        };

        Ok(Self {
            endpoint,
            account,
            allowlist,
            read_only_unknown,
        })
    }
}

/// Read + parse the `[signal]` table from `<state_root>/config.toml`, tolerating
/// a missing file (returns an empty table so env-only configuration works).
fn read_signal_table(state_root: &Path) -> SimardResult<SignalTable> {
    let path = state_root.join(crate::runtime_config::CONFIG_FILE_NAME);
    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SignalTable::default()),
        Err(e) => {
            return Err(SimardError::ActionExecutionFailed {
                action: "read config.toml".to_string(),
                reason: format!("{}: {e}", path.display()),
            });
        }
    };
    let root: RootConfig =
        toml::from_str(&contents).map_err(|e| SimardError::InvalidConfigValue {
            key: "signal".to_string(),
            value: path.display().to_string(),
            help: format!("could not parse the [signal] table: {e}"),
        })?;
    Ok(root.signal.unwrap_or_default())
}

/// Resolve a required string field env-first, then file, else a clear error.
fn resolve_string(env_key: &str, file_value: Option<String>, field: &str) -> SimardResult<String> {
    if let Ok(v) = std::env::var(env_key) {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Ok(v);
        }
    }
    match file_value {
        Some(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(SimardError::MissingRequiredConfig {
            key: format!("signal.{field}"),
            help: format!("set `{env_key}` or add `{field}` to the [signal] table of config.toml"),
        }),
    }
}
