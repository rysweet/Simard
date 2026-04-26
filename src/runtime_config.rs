//! Persistent runtime configuration loaded from `~/.simard/config.toml`.
//!
//! Replaces the historical `SIMARD_LLM_PROVIDER`-env-var-only approach,
//! which was unreliable across subprocess wrappers (notably `tmux
//! new-session` does not propagate env to its server-attached panes).
//!
//! ## Resolution order
//!
//! [`RuntimeConfig::load`] consults sources in this order:
//!
//! 1. **Environment variable** (e.g. `SIMARD_LLM_PROVIDER`) — wins when set.
//! 2. **Config file** at `<state_root>/config.toml` — used when env unset.
//! 3. **Error** — there is no silent default. The caller must surface a
//!    clear configuration failure rather than guess.
//!
//! ## Bootstrapping
//!
//! [`RuntimeConfig::bootstrap_from_env`] is called once at daemon startup.
//! If the operator launched the daemon with `SIMARD_LLM_PROVIDER=copilot`
//! in the environment but no `config.toml` exists, this helper writes the
//! current settings to disk so child processes (engineer subprocesses
//! spawned via tmux, meeting REPLs invoked from the dashboard, etc.) read
//! the same configuration without needing env propagation.
//!
//! ## Anti-pattern guard
//!
//! This module deliberately rejects any "if neither env nor file is set,
//! pick a default" path. The project's design rule — see the fallback
//! audit, `~/.copilot/session-state/.../files/fallback-audit.md` — is
//! that silent defaults mask configuration failures. A missing provider
//! is an operator error and must surface as such.

use crate::error::{SimardError, SimardResult};
use crate::session_builder::LlmProvider;
use std::path::{Path, PathBuf};

/// Filename inside the state root where persistent runtime config lives.
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// Environment variable selecting the LLM provider. When set, wins over
/// the on-disk config.
pub const ENV_LLM_PROVIDER: &str = "SIMARD_LLM_PROVIDER";

/// Persistent runtime configuration shared by the daemon and every
/// subprocess it spawns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub llm_provider: LlmProvider,
}

impl RuntimeConfig {
    /// Load the runtime config from environment + config file.
    ///
    /// Returns `Err(MissingRequiredConfig)` if neither source supplies
    /// the LLM provider — there is **no silent default**.
    pub fn load() -> SimardResult<Self> {
        Self::load_from(&default_state_root())
    }

    /// Load from a specific state root (test seam).
    pub fn load_from(state_root: &Path) -> SimardResult<Self> {
        // 1. Env wins when set.
        if let Some(provider) = read_env_provider()? {
            return Ok(Self {
                llm_provider: provider,
            });
        }

        // 2. Config file when present.
        let path = state_root.join(CONFIG_FILE_NAME);
        if path.exists() {
            let body =
                std::fs::read_to_string(&path).map_err(|e| SimardError::PersistentStoreIo {
                    store: "runtime_config".to_string(),
                    action: "read config.toml".to_string(),
                    path: path.clone(),
                    reason: e.to_string(),
                })?;
            return Self::from_toml_str(&body);
        }

        // 3. No silent default — fail loud.
        Err(SimardError::MissingRequiredConfig {
            key: ENV_LLM_PROVIDER.to_string(),
            help: format!(
                "neither {ENV_LLM_PROVIDER} env var nor {} provides llm_provider. \
                 Set the env var (e.g. SIMARD_LLM_PROVIDER=copilot) or write \
                 a config file like:\n\nllm_provider = \"copilot\"\n",
                path.display(),
            ),
        })
    }

    /// Parse a TOML config body. Public for tests.
    pub fn from_toml_str(body: &str) -> SimardResult<Self> {
        // Minimal hand parser: look for `llm_provider = "..."`. Avoids
        // pulling in serde wiring for a one-key file. Replaces if/when we
        // grow more keys.
        let mut provider: Option<LlmProvider> = None;
        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("llm_provider") {
                let value = rest
                    .trim_start()
                    .strip_prefix('=')
                    .ok_or_else(|| SimardError::InvalidConfigValue {
                        key: "llm_provider".to_string(),
                        value: line.to_string(),
                        help: "expected `llm_provider = \"copilot\"` or `\"rustyclawd\"`"
                            .to_string(),
                    })?
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'');
                provider = Some(parse_provider("config.toml", value)?);
            }
        }
        let llm_provider = provider.ok_or_else(|| SimardError::MissingRequiredConfig {
            key: "llm_provider".to_string(),
            help: "config.toml present but missing `llm_provider` key".to_string(),
        })?;
        Ok(Self { llm_provider })
    }

    /// Serialize to TOML for writing to disk.
    pub fn to_toml_string(&self) -> String {
        let provider_str = match self.llm_provider {
            LlmProvider::Copilot => "copilot",
            LlmProvider::RustyClawd => "rustyclawd",
        };
        format!(
            "# Simard runtime configuration\n\
             # Loaded by every Simard process at startup. Subprocesses\n\
             # (engineer, meeting REPL, etc.) read this file so the\n\
             # operator does not have to plumb env vars through tmux,\n\
             # systemd, or ssh wrappers.\n\
             llm_provider = \"{provider_str}\"\n"
        )
    }

    /// One-time bootstrap: if the daemon was launched with
    /// `SIMARD_LLM_PROVIDER` set in env but no config.toml exists yet,
    /// snapshot the env-derived config to disk so subprocesses can read
    /// it without env propagation. No-op if the file already exists.
    ///
    /// Returns `Ok(true)` if a file was written, `Ok(false)` otherwise.
    pub fn bootstrap_from_env(state_root: &Path) -> SimardResult<bool> {
        let path = state_root.join(CONFIG_FILE_NAME);
        if path.exists() {
            return Ok(false);
        }
        let Some(provider) = read_env_provider()? else {
            return Ok(false);
        };
        std::fs::create_dir_all(state_root).map_err(|e| SimardError::PersistentStoreIo {
            store: "runtime_config".to_string(),
            action: "create state root".to_string(),
            path: state_root.to_path_buf(),
            reason: e.to_string(),
        })?;
        let cfg = Self {
            llm_provider: provider,
        };
        std::fs::write(&path, cfg.to_toml_string()).map_err(|e| {
            SimardError::PersistentStoreIo {
                store: "runtime_config".to_string(),
                action: "write config.toml".to_string(),
                path: path.clone(),
                reason: e.to_string(),
            }
        })?;
        Ok(true)
    }
}

fn read_env_provider() -> SimardResult<Option<LlmProvider>> {
    match std::env::var(ENV_LLM_PROVIDER) {
        Ok(s) if s.is_empty() => Ok(None),
        Ok(s) => Ok(Some(parse_provider(ENV_LLM_PROVIDER, &s)?)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(SimardError::NonUnicodeConfigValue {
            key: ENV_LLM_PROVIDER.to_string(),
        }),
    }
}

fn parse_provider(source: &str, value: &str) -> SimardResult<LlmProvider> {
    match value.trim().to_ascii_lowercase().as_str() {
        "copilot" => Ok(LlmProvider::Copilot),
        "rustyclawd" => Ok(LlmProvider::RustyClawd),
        other => Err(SimardError::InvalidConfigValue {
            key: source.to_string(),
            value: other.to_string(),
            help: "expected \"copilot\" or \"rustyclawd\"".to_string(),
        }),
    }
}

/// `<state_root>` resolution: `SIMARD_STATE_ROOT` if set, else
/// `$HOME/.simard`. This is *not* a fallback — both branches are valid
/// canonical locations, and the env var is the documented override knob.
fn default_state_root() -> PathBuf {
    if let Ok(v) = std::env::var("SIMARD_STATE_ROOT") {
        return PathBuf::from(v);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
    PathBuf::from(home).join(".simard")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn with_clean_env<F: FnOnce()>(f: F) {
        // The tests in this module all read/write `SIMARD_LLM_PROVIDER`.
        // cargo runs tests multi-threaded by default, so they MUST be
        // serialized to avoid env-var races where one test's `set_var`
        // is observed by another test's `read_env_provider()`.
        //
        // CI flake observed at runtime_config.rs:286 in run 24947780630:
        // `bootstrap_from_env_writes_when_missing` saw `wrote == false`
        // because a concurrent test had cleared the env var between
        // set_var and bootstrap_from_env.
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: lock above guarantees no other test in this module
        // reads/writes SIMARD_LLM_PROVIDER while the closure runs.
        unsafe { std::env::remove_var(ENV_LLM_PROVIDER) };
        f();
        // Always clear on exit so the next test starts clean even if the
        // closure panicked before its own remove_var ran.
        unsafe { std::env::remove_var(ENV_LLM_PROVIDER) };
    }

    #[test]
    fn load_from_env_wins_when_set() {
        let tmp = TempDir::new().unwrap();
        with_clean_env(|| {
            unsafe { std::env::set_var(ENV_LLM_PROVIDER, "copilot") };
            let cfg = RuntimeConfig::load_from(tmp.path()).unwrap();
            assert_eq!(cfg.llm_provider, LlmProvider::Copilot);
            unsafe { std::env::remove_var(ENV_LLM_PROVIDER) };
        });
    }

    #[test]
    fn load_from_file_when_env_unset() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(CONFIG_FILE_NAME),
            "llm_provider = \"copilot\"\n",
        )
        .unwrap();
        with_clean_env(|| {
            let cfg = RuntimeConfig::load_from(tmp.path()).unwrap();
            assert_eq!(cfg.llm_provider, LlmProvider::Copilot);
        });
    }

    #[test]
    fn load_errors_when_neither_source_present() {
        let tmp = TempDir::new().unwrap();
        with_clean_env(|| {
            let err = RuntimeConfig::load_from(tmp.path()).unwrap_err();
            assert!(
                matches!(err, SimardError::MissingRequiredConfig { .. }),
                "expected MissingRequiredConfig, got {err:?}"
            );
        });
    }

    #[test]
    fn load_rejects_unknown_provider_value() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(CONFIG_FILE_NAME),
            "llm_provider = \"bogus\"\n",
        )
        .unwrap();
        with_clean_env(|| {
            let err = RuntimeConfig::load_from(tmp.path()).unwrap_err();
            assert!(
                matches!(err, SimardError::InvalidConfigValue { .. }),
                "expected InvalidConfigValue, got {err:?}"
            );
        });
    }

    #[test]
    fn bootstrap_from_env_writes_when_missing() {
        let tmp = TempDir::new().unwrap();
        with_clean_env(|| {
            unsafe { std::env::set_var(ENV_LLM_PROVIDER, "copilot") };
            let wrote = RuntimeConfig::bootstrap_from_env(tmp.path()).unwrap();
            assert!(wrote, "should have written config.toml");
            assert!(tmp.path().join(CONFIG_FILE_NAME).exists());
            let body = std::fs::read_to_string(tmp.path().join(CONFIG_FILE_NAME)).unwrap();
            assert!(body.contains("llm_provider = \"copilot\""));
            unsafe { std::env::remove_var(ENV_LLM_PROVIDER) };
        });
    }

    #[test]
    fn bootstrap_from_env_skips_when_file_exists() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(CONFIG_FILE_NAME),
            "llm_provider = \"rustyclawd\"\n",
        )
        .unwrap();
        with_clean_env(|| {
            unsafe { std::env::set_var(ENV_LLM_PROVIDER, "copilot") };
            let wrote = RuntimeConfig::bootstrap_from_env(tmp.path()).unwrap();
            assert!(!wrote, "should not have overwritten existing file");
            let body = std::fs::read_to_string(tmp.path().join(CONFIG_FILE_NAME)).unwrap();
            assert!(body.contains("rustyclawd"));
            unsafe { std::env::remove_var(ENV_LLM_PROVIDER) };
        });
    }

    #[test]
    fn bootstrap_from_env_noop_when_env_unset_and_no_file() {
        let tmp = TempDir::new().unwrap();
        with_clean_env(|| {
            let wrote = RuntimeConfig::bootstrap_from_env(tmp.path()).unwrap();
            assert!(!wrote);
            assert!(!tmp.path().join(CONFIG_FILE_NAME).exists());
        });
    }

    #[test]
    fn toml_roundtrip_preserves_provider() {
        for provider in [LlmProvider::Copilot, LlmProvider::RustyClawd] {
            let cfg = RuntimeConfig {
                llm_provider: provider,
            };
            let body = cfg.to_toml_string();
            let parsed = RuntimeConfig::from_toml_str(&body).unwrap();
            assert_eq!(parsed.llm_provider, provider);
        }
    }

    #[test]
    fn toml_parser_skips_comments_and_blank_lines() {
        let body = "# top comment\n\n# llm_provider = \"rustyclawd\"\nllm_provider = \"copilot\"\n";
        let cfg = RuntimeConfig::from_toml_str(body).unwrap();
        assert_eq!(cfg.llm_provider, LlmProvider::Copilot);
    }
}
