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
                Duration::from_secs(1800),
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

        let mut command = Command::new("recipe-runner-rs");
        command
            .arg(&self.recipe_path)
            .arg("--no-auto-stage")
            .arg("-C")
            .arg(repo_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for value in [
            format!("simard_binary={}", binary.display()),
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
        let output = command.output().map_err(|error| {
            CycleError::new(
                CycleErrorCode::RecipeFailed,
                format!("goal-session recipe runner failed to start: {error}"),
            )
        })?;
        if !output.status.success() {
            return Err(CycleError::new(
                CycleErrorCode::RecipeFailed,
                format!(
                    "goal-session recipe exited with {}: {}",
                    output.status,
                    bounded_diagnostic(&output.stderr)
                ),
            ));
        }

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
        if !recipe.contains("type: \"bash\"")
            || !recipe.contains("ooda actor-run")
            || recipe.contains("type: \"agent\"")
        {
            return Err(CycleError::new(
                CycleErrorCode::RecipeFailed,
                "goal-session recipe must launch the authenticated typed actor runtime and may not directly invoke an unscoped agent",
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

fn bounded_diagnostic(bytes: &[u8]) -> String {
    let end = bytes.len().min(1000);
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}
