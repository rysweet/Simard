use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::recipe_context_file::ContextFile;

use super::{
    AdmissionSnapshot, AuthenticatedToolContext, CapabilityHandler, CapabilityPolicy, CycleError,
    CycleErrorCode, GoalSessionExecution, GoalSessionInvocation,
};

const RECIPE_FILENAME: &str = "goal-session-actor.yaml";
const POLICY_FILENAME: &str = "goal-session-capabilities.toml";
const ADAPTER_TAG: &str = "typed-ooda";
const ACTOR_SESSION_LEASE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const TRUSTED_RECIPE: &str =
    include_str!("../../prompt_assets/simard/recipes/goal-session-actor.yaml");
const TRUSTED_POLICY: &str =
    include_str!("../../prompt_assets/simard/policies/goal-session-capabilities.toml");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedGoalSessionRoute {
    recipe_path: PathBuf,
    policy_path: PathBuf,
}

impl TypedGoalSessionRoute {
    pub fn production(repo_root: &Path) -> Result<Self, CycleError> {
        let mut roots = Vec::with_capacity(2);
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join(".simard/prompt_assets/simard"));
        }
        roots.push(repo_root.join("prompt_assets/simard"));

        for root in roots {
            let recipe_path = root.join("recipes").join(RECIPE_FILENAME);
            let policy_path = root.join("policies").join(POLICY_FILENAME);
            if recipe_path.is_file() && policy_path.is_file() {
                let route = Self {
                    recipe_path,
                    policy_path,
                };
                route.validate_assets()?;
                return Ok(route);
            }
        }
        Err(CycleError::new(
            CycleErrorCode::RecipeFailed,
            "typed goal-session recipe and capability policy are not installed",
        ))
    }

    pub fn recipe_path(&self) -> &Path {
        &self.recipe_path
    }

    pub fn policy_path(&self) -> &Path {
        &self.policy_path
    }

    pub fn load_policy(&self) -> Result<CapabilityPolicy, CycleError> {
        CapabilityPolicy::from_toml_file(&self.policy_path)
            .map_err(|error| CycleError::new(CycleErrorCode::ToolFailed, error.to_string()))
    }

    pub fn execute(
        &self,
        repo_root: &Path,
        ledger_path: &Path,
        handler: &CapabilityHandler,
        actor: &AuthenticatedToolContext,
        admission: &AdmissionSnapshot,
        invocation: &GoalSessionInvocation,
    ) -> Result<GoalSessionExecution, CycleError> {
        let lease = handler
            .register_actor_session(
                actor,
                &format!("actor-session:{}", uuid::Uuid::now_v7()),
                &invocation.cycle_id,
                &invocation.goal_id,
                ACTOR_SESSION_LEASE,
            )
            .map_err(|error| CycleError::new(CycleErrorCode::ToolFailed, error.to_string()))?;

        let task = ContextFile::write_bytes(ADAPTER_TAG, "task", invocation.task.as_bytes())
            .map_err(context_error)?;
        let reason = ContextFile::write_bytes(ADAPTER_TAG, "reason", invocation.reason.as_bytes())
            .map_err(context_error)?;
        let observe = ContextFile::write_bytes(
            ADAPTER_TAG,
            "observe_output",
            invocation.observe_output.as_bytes(),
        )
        .map_err(context_error)?;
        let orient = ContextFile::write_bytes(
            ADAPTER_TAG,
            "orient_output",
            invocation.orient_output.as_bytes(),
        )
        .map_err(context_error)?;
        let decide = ContextFile::write_bytes(
            ADAPTER_TAG,
            "decide_output",
            invocation.decide_output.as_bytes(),
        )
        .map_err(context_error)?;
        let token = ContextFile::write_bytes(ADAPTER_TAG, "auth_token", lease.token.as_bytes())
            .map_err(context_error)?;
        let admission_json = serde_json::to_vec(admission).map_err(|error| {
            CycleError::new(
                CycleErrorCode::PersistenceFailed,
                format!("admission snapshot serialization failed: {error}"),
            )
        })?;
        let admission = ContextFile::write_bytes(ADAPTER_TAG, "admission", &admission_json)
            .map_err(context_error)?;
        let binary = std::env::current_exe().map_err(|error| {
            CycleError::new(
                CycleErrorCode::RecipeFailed,
                format!("current Simard binary could not be resolved: {error}"),
            )
        })?;

        let runner = resolve_recipe_runner()?;
        // Resolve the agent binary from the canonical provider config BEFORE
        // creating the diagnostic file or spawning. recipe-runner-rs otherwise
        // falls back to a hardcoded "claude", which is unauthenticated on the
        // Copilot host and silently fails every OODA goal. No silent fallback:
        // if the provider config is unavailable, surface a visible RecipeFailed
        // rather than spawning with the wrong (unauthenticated) default.
        // Resolving here (before the temp file) also avoids leaking an orphaned
        // diagnostic file on the error path.
        let agent_binary =
            crate::session_builder::LlmProvider::resolve_agent_binary().ok_or_else(|| {
                CycleError::new(
                    CycleErrorCode::RecipeFailed,
                    "goal-session agent binary could not be resolved from provider config; \
                     refusing to spawn recipe-runner-rs with an unauthenticated default",
                )
            })?;
        let diagnostic_path = std::env::temp_dir().join(format!(
            "simard-goal-session-{}.stderr",
            uuid::Uuid::now_v7()
        ));
        let diagnostic_file = std::fs::File::create(&diagnostic_path).map_err(|error| {
            CycleError::new(
                CycleErrorCode::RecipeFailed,
                format!("goal-session diagnostic file could not be created: {error}"),
            )
        })?;
        let context = vec![
            format!("simard_binary={}", binary.display()),
            format!("recipe_path={}", self.recipe_path.display()),
            format!("ledger_path={}", ledger_path.display()),
            format!("policy_path={}", self.policy_path.display()),
            format!("session_id={}", invocation.session_id),
            format!("cycle_id={}", invocation.cycle_id),
            format!("goal_id={}", invocation.goal_id),
            task.arg_value(),
            reason.arg_value(),
            observe.arg_value(),
            orient.arg_value(),
            decide.arg_value(),
            token.arg_value(),
            admission.arg_value(),
        ];
        let mut command = build_goal_session_command(
            &runner,
            &self.recipe_path,
            repo_root,
            &context,
            agent_binary,
        );
        command.stdout(Stdio::null()).stderr(diagnostic_file);
        let status = command.status().map_err(|error| {
            CycleError::new(
                CycleErrorCode::RecipeFailed,
                format!("goal-session recipe runner failed to start: {error}"),
            )
        })?;
        if !status.success() {
            let diagnostic = bounded_diagnostic_file(&diagnostic_path);
            let _ = std::fs::remove_file(&diagnostic_path);
            return Err(CycleError::new(
                CycleErrorCode::RecipeFailed,
                format!("goal-session recipe exited with {status}: {diagnostic}"),
            ));
        }
        let _ = std::fs::remove_file(&diagnostic_path);

        let outcome = handler
            .terminal_for_cycle(&invocation.session_id, &invocation.cycle_id)
            .map_err(|error| CycleError::new(CycleErrorCode::PersistenceFailed, error.to_string()))?
            .ok_or_else(|| {
                CycleError::new(
                    CycleErrorCode::MissingTerminal,
                    "goal-session recipe completed without a durable terminal",
                )
            })?;
        Ok(GoalSessionExecution { outcome })
    }

    fn validate_assets(&self) -> Result<(), CycleError> {
        let recipe = std::fs::read_to_string(&self.recipe_path).map_err(|error| {
            CycleError::new(
                CycleErrorCode::RecipeFailed,
                format!(
                    "goal-session recipe {} could not be read: {error}",
                    self.recipe_path.display()
                ),
            )
        })?;
        if recipe != TRUSTED_RECIPE {
            return Err(CycleError::new(
                CycleErrorCode::RecipeFailed,
                "goal-session recipe does not match the trusted compiled asset",
            ));
        }
        let policy = std::fs::read_to_string(&self.policy_path).map_err(|error| {
            CycleError::new(
                CycleErrorCode::RecipeFailed,
                format!(
                    "goal-session policy {} could not be read: {error}",
                    self.policy_path.display()
                ),
            )
        })?;
        if policy != TRUSTED_POLICY {
            return Err(CycleError::new(
                CycleErrorCode::RecipeFailed,
                "goal-session policy does not match the trusted compiled asset",
            ));
        }
        self.load_policy().map(|_| ())
    }
}

fn context_error(error: std::io::Error) -> CycleError {
    CycleError::new(
        CycleErrorCode::RecipeFailed,
        format!("goal-session private context transport failed: {error}"),
    )
}

/// Build the `recipe-runner-rs` `Command` for the typed goal-session actor.
///
/// Thin construction seam: assembles the runner argv (recipe path,
/// `--no-auto-stage`, `-C <repo>`, then each context value as `-c <value>`)
/// and — critically — exports `AMPLIHACK_AGENT_BINARY` so the nested agent
/// uses the resolved provider binary instead of recipe-runner-rs's hardcoded
/// `"claude"` default. Stdio is applied by the caller. No behavior beyond
/// carrying the env; kept minimal so the env invariant is unit-testable.
fn build_goal_session_command(
    runner: &Path,
    recipe_path: &Path,
    repo_root: &Path,
    context: &[String],
    agent_binary: &str,
) -> Command {
    let mut command = Command::new(runner);
    command
        .arg(recipe_path)
        .arg("--no-auto-stage")
        .arg("-C")
        .arg(repo_root)
        .env("AMPLIHACK_AGENT_BINARY", agent_binary);
    for value in context {
        command.arg("-c").arg(value);
    }
    command
}

fn resolve_recipe_runner() -> Result<PathBuf, CycleError> {
    if let Some(configured) = std::env::var_os("SIMARD_RECIPE_RUNNER_BIN") {
        let path = PathBuf::from(configured);
        if path.is_absolute() && path.is_file() {
            return Ok(path);
        }
        return Err(CycleError::new(
            CycleErrorCode::RecipeFailed,
            "SIMARD_RECIPE_RUNNER_BIN must name an existing absolute file",
        ));
    }
    let path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".cargo/bin/recipe-runner-rs");
    if path.is_file() {
        return Ok(path);
    }
    Err(CycleError::new(
        CycleErrorCode::RecipeFailed,
        "recipe-runner-rs was not found at ~/.cargo/bin; configure SIMARD_RECIPE_RUNNER_BIN",
    ))
}

fn bounded_diagnostic_file(path: &Path) -> String {
    const LIMIT: u64 = 4096;
    let mut bytes = Vec::new();
    let Ok(file) = std::fs::File::open(path) else {
        return "diagnostic output unavailable".to_string();
    };
    let mut reader = file.take(LIMIT + 1);
    if reader.read_to_end(&mut bytes).is_err() {
        return "diagnostic output unreadable".to_string();
    }
    let truncated = bytes.len() as u64 > LIMIT;
    bytes.truncate(LIMIT as usize);
    let mut diagnostic = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        diagnostic.push_str("\n[diagnostic truncated after 4096 bytes]");
    }
    diagnostic
}

#[cfg(test)]
mod agent_binary_rail_tests {
    //! BUG 1 regression (the multi-day outage cause): the goal-session
    //! `recipe-runner-rs` subprocess must carry `AMPLIHACK_AGENT_BINARY` set to
    //! the resolved provider binary. Without it, `recipe-runner-rs` falls back
    //! to the hardcoded `"claude"`, which is unauthenticated on the Copilot
    //! host — the agent step exits 1, no durable terminal is recorded, and every
    //! OODA goal logs "consecutive failures" with an empty `terminal_outcomes`.
    //!
    //! Every other `recipe-runner-rs` spawn site in the repo already sets this
    //! (journal, stewardship, disk_health, disk_reclaim). These tests pin the
    //! same invariant for the typed goal-session route by inspecting the built
    //! `Command`'s env — mirroring the `command.get_envs()` style — via the
    //! `build_goal_session_command` construction seam the fix extracts.
    use super::*;
    use std::ffi::{OsStr, OsString};
    use std::path::Path;

    #[test]
    fn goal_session_command_exports_amplihack_agent_binary() {
        let context = vec![
            "session_id=cycle-session".to_string(),
            "goal_id=goal-1".to_string(),
        ];
        let command = build_goal_session_command(
            Path::new("/home/agent/.cargo/bin/recipe-runner-rs"),
            Path::new("/etc/simard/recipes/goal-session-actor.yaml"),
            Path::new("/srv/simard/repo"),
            &context,
            "copilot",
        );
        let carries_binary = command.get_envs().any(|(key, value)| {
            key == OsStr::new("AMPLIHACK_AGENT_BINARY") && value == Some(OsStr::new("copilot"))
        });
        assert!(
            carries_binary,
            "goal-session recipe Command must export AMPLIHACK_AGENT_BINARY=<resolved provider>",
        );
    }

    #[test]
    fn goal_session_command_binary_is_the_resolved_value_not_hardcoded() {
        // The env value is whatever the caller resolved from the canonical
        // provider resolver — never a hardcoded default. A different provider
        // binary must flow through verbatim.
        let command = build_goal_session_command(
            Path::new("/runner"),
            Path::new("/recipe.yaml"),
            Path::new("/repo"),
            &[],
            "rustyclawd",
        );
        let value = command
            .get_envs()
            .find(|(key, _)| *key == OsStr::new("AMPLIHACK_AGENT_BINARY"))
            .and_then(|(_, value)| value)
            .map(OsStr::to_owned);
        assert_eq!(value, Some(OsString::from("rustyclawd")));
    }

    #[test]
    fn goal_session_command_preserves_runner_argv() {
        // Adding the env must not disturb the existing recipe-runner argv:
        // recipe path first, then --no-auto-stage, -C <repo>, and each
        // context value passed as `-c <value>`.
        let context = vec!["session_id=abc".to_string(), "goal_id=xyz".to_string()];
        let command = build_goal_session_command(
            Path::new("/runner"),
            Path::new("/recipe.yaml"),
            Path::new("/repo"),
            &context,
            "copilot",
        );
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args.first().map(String::as_str),
            Some("/recipe.yaml"),
            "the recipe path must remain the first positional argument",
        );
        assert!(
            args.iter().any(|arg| arg == "--no-auto-stage"),
            "the --no-auto-stage flag must be preserved",
        );
        let dash_c = args
            .iter()
            .position(|arg| arg == "-C")
            .expect("the -C working-directory flag must be present");
        assert_eq!(
            args.get(dash_c + 1).map(String::as_str),
            Some("/repo"),
            "-C must be followed by the repo root",
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-c" && pair[1] == "session_id=abc"),
            "each context value must be carried as `-c <value>`",
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-c" && pair[1] == "goal_id=xyz"),
            "each context value must be carried as `-c <value>`",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// RAII guard that overrides an env var for the duration of a test and
    /// restores the previous value (or unset) on drop. Tests using it MUST be
    /// `#[serial_test::serial(cognitive_memory)]` because the process environment is global.
    struct EnvGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: env mutation is serialised via `#[serial_test::serial(cognitive_memory)]`.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    fn write_trusted_assets(root: &Path) {
        let recipes = root.join("prompt_assets/simard/recipes");
        let policies = root.join("prompt_assets/simard/policies");
        fs::create_dir_all(&recipes).expect("create recipes dir");
        fs::create_dir_all(&policies).expect("create policies dir");
        fs::write(recipes.join(RECIPE_FILENAME), TRUSTED_RECIPE).expect("write recipe");
        fs::write(policies.join(POLICY_FILENAME), TRUSTED_POLICY).expect("write policy");
    }

    #[test]
    fn ledger_path_appends_the_canonical_relative_path() {
        let path = crate::typed_ooda::ledger_path(Path::new("/state/root"));
        assert_eq!(path, Path::new("/state/root/typed-ooda/outcomes.sqlite3"));
    }

    #[test]
    fn bounded_diagnostic_file_reports_missing_file() {
        let missing = std::env::temp_dir().join(format!("simard-missing-{}", uuid::Uuid::now_v7()));
        assert_eq!(
            bounded_diagnostic_file(&missing),
            "diagnostic output unavailable"
        );
    }

    #[test]
    fn bounded_diagnostic_file_returns_short_content_verbatim() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("diag.txt");
        fs::write(&path, b"boom happened here").expect("write diag");
        assert_eq!(bounded_diagnostic_file(&path), "boom happened here");
    }

    #[test]
    fn bounded_diagnostic_file_truncates_oversized_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("diag.txt");
        fs::write(&path, vec![b'x'; 8192]).expect("write diag");
        let diagnostic = bounded_diagnostic_file(&path);
        assert!(diagnostic.contains("[diagnostic truncated after 4096 bytes]"));
        // 4096 'x' bytes plus the truncation notice.
        assert!(diagnostic.starts_with(&"x".repeat(4096)));
        assert!(diagnostic.len() < 8192);
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn resolve_recipe_runner_rejects_relative_configuration() {
        let _guard = EnvGuard::set("SIMARD_RECIPE_RUNNER_BIN", Path::new("relative/runner"));
        let error = resolve_recipe_runner().expect_err("relative path must be rejected");
        assert_eq!(error.code(), CycleErrorCode::RecipeFailed);
        assert!(error.to_string().contains("absolute file"));
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn resolve_recipe_runner_rejects_absolute_but_missing_file() {
        let missing = std::env::temp_dir().join(format!("simard-runner-{}", uuid::Uuid::now_v7()));
        let _guard = EnvGuard::set("SIMARD_RECIPE_RUNNER_BIN", &missing);
        let error = resolve_recipe_runner().expect_err("missing file must be rejected");
        assert_eq!(error.code(), CycleErrorCode::RecipeFailed);
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn resolve_recipe_runner_accepts_absolute_existing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let runner = dir.path().join("recipe-runner-rs");
        fs::write(&runner, b"#!/bin/sh\n").expect("write runner");
        let _guard = EnvGuard::set("SIMARD_RECIPE_RUNNER_BIN", &runner);
        let resolved = resolve_recipe_runner().expect("existing absolute file accepted");
        assert_eq!(resolved, runner);
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn production_loads_and_validates_installed_assets() {
        let home = tempfile::tempdir().expect("home tempdir");
        let repo = tempfile::tempdir().expect("repo tempdir");
        let _home_guard = EnvGuard::set("HOME", home.path());
        write_trusted_assets(repo.path());

        let route = TypedGoalSessionRoute::production(repo.path()).expect("production route");
        assert_eq!(
            route.recipe_path(),
            repo.path()
                .join("prompt_assets/simard/recipes")
                .join(RECIPE_FILENAME)
        );
        assert_eq!(
            route.policy_path(),
            repo.path()
                .join("prompt_assets/simard/policies")
                .join(POLICY_FILENAME)
        );
        // load_policy must parse the trusted TOML into a usable policy.
        let policy = route.load_policy().expect("policy parses");
        assert!(!policy.revision.is_empty());
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn production_fails_when_assets_are_not_installed() {
        let home = tempfile::tempdir().expect("home tempdir");
        let repo = tempfile::tempdir().expect("repo tempdir");
        let _home_guard = EnvGuard::set("HOME", home.path());

        let error =
            TypedGoalSessionRoute::production(repo.path()).expect_err("missing assets must fail");
        assert_eq!(error.code(), CycleErrorCode::RecipeFailed);
        assert!(error.to_string().contains("not installed"));
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn production_rejects_a_tampered_recipe() {
        let home = tempfile::tempdir().expect("home tempdir");
        let repo = tempfile::tempdir().expect("repo tempdir");
        let _home_guard = EnvGuard::set("HOME", home.path());
        write_trusted_assets(repo.path());
        // Corrupt the recipe so it no longer matches the trusted compiled asset.
        let recipe_path = repo
            .path()
            .join("prompt_assets/simard/recipes")
            .join(RECIPE_FILENAME);
        fs::write(&recipe_path, "tampered: true\n").expect("overwrite recipe");

        let error = TypedGoalSessionRoute::production(repo.path())
            .expect_err("tampered recipe must be rejected");
        assert_eq!(error.code(), CycleErrorCode::RecipeFailed);
        assert!(error.to_string().contains("trusted compiled asset"));
    }

    #[test]
    fn context_error_wraps_io_errors_as_recipe_failures() {
        let io = std::io::Error::other("pipe broke");
        let error = context_error(io);
        assert_eq!(error.code(), CycleErrorCode::RecipeFailed);
        assert!(error.to_string().contains("pipe broke"));
    }
}
