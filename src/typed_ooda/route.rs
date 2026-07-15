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
        let mut command = Command::new(runner);
        command
            .arg(&self.recipe_path)
            .arg("--no-auto-stage")
            .arg("-C")
            .arg(repo_root)
            .stdout(Stdio::null())
            .stderr(diagnostic_file);
        for value in [
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
        ] {
            command.arg("-c").arg(value);
        }
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
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).expect("write test asset");
        path
    }

    /// Build a route whose on-disk assets are byte-identical to the trusted
    /// compiled-in copies, exercising the accept path without touching the
    /// process environment or the real install tree.
    fn trusted_route(dir: &Path) -> TypedGoalSessionRoute {
        TypedGoalSessionRoute {
            recipe_path: write(dir, "recipe.yaml", TRUSTED_RECIPE),
            policy_path: write(dir, "policy.toml", TRUSTED_POLICY),
        }
    }

    #[test]
    fn accessors_return_configured_paths() {
        let dir = tempfile::tempdir().unwrap();
        let route = trusted_route(dir.path());
        assert_eq!(route.recipe_path(), dir.path().join("recipe.yaml"));
        assert_eq!(route.policy_path(), dir.path().join("policy.toml"));
    }

    #[test]
    fn validate_assets_accepts_byte_identical_trusted_assets() {
        let dir = tempfile::tempdir().unwrap();
        trusted_route(dir.path())
            .validate_assets()
            .expect("trusted assets must validate");
    }

    #[test]
    fn validate_assets_rejects_recipe_that_diverges_from_trusted_copy() {
        let dir = tempfile::tempdir().unwrap();
        let route = TypedGoalSessionRoute {
            recipe_path: write(
                dir.path(),
                "recipe.yaml",
                &format!("{TRUSTED_RECIPE}\n# smuggled instruction\n"),
            ),
            policy_path: write(dir.path(), "policy.toml", TRUSTED_POLICY),
        };
        let error = route
            .validate_assets()
            .expect_err("tampered recipe rejected");
        assert_eq!(error.code(), CycleErrorCode::RecipeFailed);
        assert!(
            error
                .to_string()
                .contains("recipe does not match the trusted compiled asset"),
            "unexpected message: {error}"
        );
    }

    #[test]
    fn validate_assets_rejects_policy_that_diverges_from_trusted_copy() {
        let dir = tempfile::tempdir().unwrap();
        let route = TypedGoalSessionRoute {
            recipe_path: write(dir.path(), "recipe.yaml", TRUSTED_RECIPE),
            policy_path: write(
                dir.path(),
                "policy.toml",
                &format!("{TRUSTED_POLICY}\n# widened grant\n"),
            ),
        };
        let error = route
            .validate_assets()
            .expect_err("tampered policy rejected");
        assert_eq!(error.code(), CycleErrorCode::RecipeFailed);
        assert!(
            error
                .to_string()
                .contains("policy does not match the trusted compiled asset"),
            "unexpected message: {error}"
        );
    }

    #[test]
    fn validate_assets_reports_unreadable_recipe() {
        let dir = tempfile::tempdir().unwrap();
        let route = TypedGoalSessionRoute {
            recipe_path: dir.path().join("absent-recipe.yaml"),
            policy_path: dir.path().join("absent-policy.toml"),
        };
        let error = route
            .validate_assets()
            .expect_err("missing recipe rejected");
        assert_eq!(error.code(), CycleErrorCode::RecipeFailed);
        assert!(
            error.to_string().contains("could not be read"),
            "unexpected message: {error}"
        );
    }

    #[test]
    fn load_policy_parses_the_trusted_policy_document() {
        let dir = tempfile::tempdir().unwrap();
        let policy = trusted_route(dir.path())
            .load_policy()
            .expect("trusted policy parses");
        assert!(
            !policy.revision.is_empty(),
            "parsed policy must carry a revision id"
        );
    }

    #[test]
    fn load_policy_surfaces_malformed_toml_as_tool_failure() {
        let dir = tempfile::tempdir().unwrap();
        let route = TypedGoalSessionRoute {
            recipe_path: write(dir.path(), "recipe.yaml", TRUSTED_RECIPE),
            policy_path: write(dir.path(), "policy.toml", "this is = = not valid toml"),
        };
        let error = route.load_policy().expect_err("malformed policy rejected");
        assert_eq!(error.code(), CycleErrorCode::ToolFailed);
    }

    #[test]
    fn bounded_diagnostic_file_returns_full_short_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "diag", "recipe blew up here");
        assert_eq!(bounded_diagnostic_file(&path), "recipe blew up here");
    }

    #[test]
    fn bounded_diagnostic_file_truncates_and_marks_oversized_output() {
        let dir = tempfile::tempdir().unwrap();
        let payload = "x".repeat(5000);
        let path = write(dir.path(), "diag", &payload);
        let diagnostic = bounded_diagnostic_file(&path);
        assert!(
            diagnostic.starts_with(&"x".repeat(4096)),
            "the first 4096 bytes must be preserved verbatim"
        );
        assert!(
            diagnostic.ends_with("[diagnostic truncated after 4096 bytes]"),
            "oversized output must carry the truncation marker: {diagnostic}"
        );
        assert!(
            !diagnostic.contains(&"x".repeat(4097)),
            "no more than 4096 payload bytes may survive truncation"
        );
    }

    #[test]
    fn bounded_diagnostic_file_reports_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            bounded_diagnostic_file(&dir.path().join("no-such-diagnostic")),
            "diagnostic output unavailable"
        );
    }

    #[test]
    fn context_error_maps_io_failures_to_recipe_failed() {
        let error = context_error(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "transport closed",
        ));
        assert_eq!(error.code(), CycleErrorCode::RecipeFailed);
        assert!(
            error
                .to_string()
                .contains("private context transport failed"),
            "unexpected message: {error}"
        );
    }
}
