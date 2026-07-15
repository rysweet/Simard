use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use super::CapabilityHandler;
use super::ledger::{EffectJob, EffectResult};
use super::types::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalSessionInvocation {
    pub session_id: String,
    pub cycle_id: String,
    pub goal_id: String,
    pub task: OpaqueBytes,
    pub reason: OpaqueBytes,
    pub observe_output: OpaqueBytes,
    pub orient_output: OpaqueBytes,
    pub decide_output: OpaqueBytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CycleErrorCode {
    MissingTerminal,
    MultipleTerminalAttempts,
    ToolFailed,
    RecipeFailed,
    DownstreamFailed,
    PersistenceFailed,
}

#[derive(Debug)]
pub struct CycleError {
    code: CycleErrorCode,
    message: String,
}

impl CycleError {
    pub(crate) fn new(code: CycleErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> CycleErrorCode {
        self.code
    }
}

impl std::fmt::Display for CycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CycleError {}

#[derive(Debug)]
pub struct RecipeProcessError {
    message: String,
}

impl RecipeProcessError {
    pub fn nonzero_exit(code: i32) -> Self {
        Self {
            message: format!("recipe process exited with status {code}"),
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RecipeProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RecipeProcessError {}

impl From<CapabilityError> for RecipeProcessError {
    fn from(value: CapabilityError) -> Self {
        Self {
            message: value.to_string(),
        }
    }
}

#[derive(Debug)]
pub struct EffectExecutionError {
    message: String,
    permanent: bool,
}

impl EffectExecutionError {
    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            permanent: true,
        }
    }

    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            permanent: false,
        }
    }
}

impl std::fmt::Display for EffectExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EffectExecutionError {}

pub trait EffectExecutor: Send + Sync {
    fn execute(&self, job: &EffectJob) -> Result<EffectResult, EffectExecutionError>;
}

#[derive(Debug)]
struct ToolCallState {
    terminal_attempts: usize,
    failed: Option<String>,
}

pub struct GoalSessionTools<'a> {
    handler: &'a CapabilityHandler,
    actor: &'a AuthenticatedToolContext,
    admission: &'a AdmissionSnapshot,
    invocation: &'a GoalSessionInvocation,
    state: Mutex<ToolCallState>,
}

impl GoalSessionTools<'_> {
    pub fn record_action(
        &self,
        request_id: &str,
        action: Action,
        raw_semantic: OpaqueBytes,
        evidence: Vec<EvidenceRef>,
    ) -> CapabilityResult<TerminalOutcome> {
        self.terminal_call(|| {
            self.handler.record_action(
                self.actor,
                RecordActionRequest {
                    identity: self.identity(request_id),
                    action,
                    raw_semantic,
                    evidence,
                },
                self.admission,
            )
        })
    }

    pub fn record_no_action(
        &self,
        request_id: &str,
        reason: OpaqueBytes,
        raw_semantic: OpaqueBytes,
        evidence: Vec<EvidenceRef>,
    ) -> CapabilityResult<TerminalOutcome> {
        self.terminal_call(|| {
            self.handler.record_no_action(
                self.actor,
                RecordNoActionRequest {
                    identity: self.identity(request_id),
                    reason,
                    raw_semantic,
                    evidence,
                },
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_blocked(
        &self,
        request_id: &str,
        reason: OpaqueBytes,
        blocker: BlockerRef,
        retry: RetryPolicy,
        raw_semantic: OpaqueBytes,
        evidence: Vec<EvidenceRef>,
    ) -> CapabilityResult<TerminalOutcome> {
        self.terminal_call(|| {
            self.handler.record_blocked(
                self.actor,
                RecordBlockedRequest {
                    identity: self.identity(request_id),
                    reason,
                    blocker,
                    retry,
                    raw_semantic,
                    evidence,
                },
            )
        })
    }

    pub fn record_completed(
        &self,
        request_id: &str,
        summary: OpaqueBytes,
        completion: CompletionRef,
        raw_semantic: OpaqueBytes,
        evidence: Vec<EvidenceRef>,
    ) -> CapabilityResult<TerminalOutcome> {
        self.terminal_call(|| {
            self.handler.record_completed(
                self.actor,
                RecordCompletedRequest {
                    identity: self.identity(request_id),
                    summary,
                    completion,
                    raw_semantic,
                    evidence,
                },
            )
        })
    }

    fn identity(&self, request_id: &str) -> TerminalRequestIdentity {
        TerminalRequestIdentity::new(
            request_id,
            &self.invocation.session_id,
            &self.invocation.cycle_id,
            &self.invocation.goal_id,
        )
    }

    fn terminal_call<T>(&self, call: impl FnOnce() -> CapabilityResult<T>) -> CapabilityResult<T> {
        {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.terminal_attempts += 1;
        }
        let result = call();
        if let Err(error) = &result {
            let mut state = self.state.lock().unwrap_or_else(|value| value.into_inner());
            state.failed = Some(error.to_string());
        }
        result
    }
}

pub struct GoalSessionExecutor {
    handler: CapabilityHandler,
    actor: AuthenticatedToolContext,
    admission: AdmissionSnapshot,
    effects: Box<dyn EffectExecutor>,
}

impl GoalSessionExecutor {
    pub fn new(
        handler: CapabilityHandler,
        actor: AuthenticatedToolContext,
        admission: AdmissionSnapshot,
        effects: Box<dyn EffectExecutor>,
    ) -> Self {
        Self {
            handler,
            actor,
            admission,
            effects,
        }
    }

    pub fn handler(&self) -> &CapabilityHandler {
        &self.handler
    }

    pub fn execute<F>(
        &self,
        invocation: &GoalSessionInvocation,
        actor_step: F,
    ) -> Result<GoalSessionExecution, CycleError>
    where
        F: FnOnce(&GoalSessionInvocation, &GoalSessionTools<'_>) -> Result<(), RecipeProcessError>,
    {
        self.execute_with_effects(invocation, self.effects.as_ref(), actor_step)
    }

    pub fn execute_with_effects<F>(
        &self,
        invocation: &GoalSessionInvocation,
        effects: &dyn EffectExecutor,
        actor_step: F,
    ) -> Result<GoalSessionExecution, CycleError>
    where
        F: FnOnce(&GoalSessionInvocation, &GoalSessionTools<'_>) -> Result<(), RecipeProcessError>,
    {
        let execution = self.execute_actor_step(invocation, actor_step)?;
        self.complete_outcome_effect(&execution.outcome, effects)?;
        Ok(execution)
    }

    pub fn execute_actor_step<F>(
        &self,
        invocation: &GoalSessionInvocation,
        actor_step: F,
    ) -> Result<GoalSessionExecution, CycleError>
    where
        F: FnOnce(&GoalSessionInvocation, &GoalSessionTools<'_>) -> Result<(), RecipeProcessError>,
    {
        if invocation.session_id != self.actor.session_id {
            return Err(CycleError::new(
                CycleErrorCode::ToolFailed,
                "goal-session invocation does not match authenticated actor session",
            ));
        }
        let actor = match (self.actor.bound_cycle_id(), self.actor.bound_goal_id()) {
            (Some(cycle_id), Some(goal_id))
                if cycle_id == invocation.cycle_id && goal_id == invocation.goal_id =>
            {
                self.actor.clone()
            }
            (None, None) => self
                .actor
                .clone()
                .bound_to_cycle_goal(&invocation.cycle_id, &invocation.goal_id),
            _ => {
                return Err(CycleError::new(
                    CycleErrorCode::ToolFailed,
                    "goal-session invocation does not match the actor's server-bound cycle and goal",
                ));
            }
        };
        let tools = GoalSessionTools {
            handler: &self.handler,
            actor: &actor,
            admission: &self.admission,
            invocation,
            state: Mutex::new(ToolCallState {
                terminal_attempts: 0,
                failed: None,
            }),
        };
        let process_result = actor_step(invocation, &tools);
        let state = tools
            .state
            .into_inner()
            .unwrap_or_else(|error| error.into_inner());
        if state.terminal_attempts > 1 {
            return Err(CycleError::new(
                CycleErrorCode::MultipleTerminalAttempts,
                "goal-session actor attempted more than one terminal capability",
            ));
        }
        if let Some(error) = state.failed {
            return Err(CycleError::new(
                CycleErrorCode::ToolFailed,
                format!("goal-session capability failed: {error}"),
            ));
        }
        if let Err(error) = process_result {
            return Err(CycleError::new(
                CycleErrorCode::RecipeFailed,
                error.to_string(),
            ));
        }
        let outcome = self
            .handler
            .terminal_for_cycle(&invocation.session_id, &invocation.cycle_id)
            .map_err(|error| CycleError::new(CycleErrorCode::PersistenceFailed, error.to_string()))?
            .ok_or_else(|| {
                CycleError::new(
                    CycleErrorCode::MissingTerminal,
                    "recipe completed without a durable terminal outcome",
                )
            })?;
        Ok(GoalSessionExecution { outcome })
    }

    pub fn complete_outcome_effect(
        &self,
        outcome: &TerminalOutcome,
        effects: &dyn EffectExecutor,
    ) -> Result<(), CycleError> {
        if outcome.kind != TerminalKind::Action {
            return Ok(());
        }
        let worker = OutboxWorker::new(
            &self.handler,
            effects,
            "goal-session-effect-dispatcher",
            Duration::from_secs(300),
        );
        worker.dispatch_outcome(outcome)
    }
}

pub struct OutboxWorker<'a> {
    handler: &'a CapabilityHandler,
    effects: &'a dyn EffectExecutor,
    worker_id: &'a str,
    lease: Duration,
}

impl<'a> OutboxWorker<'a> {
    pub fn new(
        handler: &'a CapabilityHandler,
        effects: &'a dyn EffectExecutor,
        worker_id: &'a str,
        lease: Duration,
    ) -> Self {
        Self {
            handler,
            effects,
            worker_id,
            lease,
        }
    }

    pub fn recover_startup(&self) -> Result<usize, CycleError> {
        self.handler
            .recover_expired_effects(
                &format!("{}:recover:{}", self.worker_id, uuid::Uuid::now_v7()),
                SystemTime::now(),
            )
            .map_err(|error| CycleError::new(CycleErrorCode::PersistenceFailed, error.to_string()))
    }

    pub fn drain_pending(&self, limit: usize) -> Result<usize, CycleError> {
        self.recover_startup()?;
        let mut completed = 0;
        for _ in 0..limit {
            let Some(job) = self
                .handler
                .claim_next_effect(
                    self.worker_id,
                    &format!("{}:claim:{}", self.worker_id, uuid::Uuid::now_v7()),
                    SystemTime::now(),
                    self.lease,
                )
                .map_err(|error| {
                    CycleError::new(CycleErrorCode::PersistenceFailed, error.to_string())
                })?
            else {
                break;
            };
            self.execute_claimed(job)?;
            completed += 1;
        }
        Ok(completed)
    }

    pub fn dispatch_outcome(&self, outcome: &TerminalOutcome) -> Result<(), CycleError> {
        self.recover_startup()?;
        let current = self
            .handler
            .effect_for_outcome(&outcome.outcome_id)
            .map_err(|error| CycleError::new(CycleErrorCode::PersistenceFailed, error.to_string()))?
            .ok_or_else(|| {
                CycleError::new(
                    CycleErrorCode::DownstreamFailed,
                    "action terminal has no durable effect",
                )
            })?;
        match current.state.as_str() {
            "succeeded" => return Ok(()),
            "failed" => {
                return Err(CycleError::new(
                    CycleErrorCode::DownstreamFailed,
                    current
                        .error
                        .unwrap_or_else(|| "downstream effect permanently failed".to_string()),
                ));
            }
            "blocked" => {
                return Err(CycleError::new(
                    CycleErrorCode::DownstreamFailed,
                    current
                        .error
                        .unwrap_or_else(|| "downstream effect is blocked".to_string()),
                ));
            }
            "running" => {
                return Err(CycleError::new(
                    CycleErrorCode::DownstreamFailed,
                    "downstream effect is leased by another worker",
                ));
            }
            "indeterminate" => {
                return Err(CycleError::new(
                    CycleErrorCode::DownstreamFailed,
                    "downstream effect execution is indeterminate and will not be repeated",
                ));
            }
            "pending" => {}
            other => {
                return Err(CycleError::new(
                    CycleErrorCode::PersistenceFailed,
                    format!("unknown durable effect state {other:?}"),
                ));
            }
        }
        if crate::read_only_guard::observe_only_enabled() {
            self.handler
                .block_effect_authorization(
                    &current,
                    &effect_mutation_request_id(&current, "observe-only-block"),
                    "SIMARD_OBSERVE_ONLY denied mutation at effect dispatch",
                )
                .map_err(|error| {
                    CycleError::new(CycleErrorCode::PersistenceFailed, error.to_string())
                })?;
            return Err(CycleError::new(
                CycleErrorCode::DownstreamFailed,
                "effect is blocked by SIMARD_OBSERVE_ONLY",
            ));
        }
        if matches!(
            current.action,
            Action::RequestMerge(_) | Action::RequestDeploy(_)
        ) && current.approval.is_none()
        {
            self.handler
                .block_effect_authorization(
                    &current,
                    &effect_mutation_request_id(&current, "approval-block"),
                    "server-issued privileged approval is required",
                )
                .map_err(|error| {
                    CycleError::new(CycleErrorCode::PersistenceFailed, error.to_string())
                })?;
            return Err(CycleError::new(
                CycleErrorCode::DownstreamFailed,
                "privileged effect is blocked pending a server-issued approval",
            ));
        }
        let job = self
            .handler
            .claim_effect_for_outcome(
                &outcome.outcome_id,
                self.worker_id,
                &format!("{}:claim:{}", self.worker_id, uuid::Uuid::now_v7()),
                SystemTime::now(),
                self.lease,
            )
            .map_err(|error| CycleError::new(CycleErrorCode::PersistenceFailed, error.to_string()))?
            .ok_or_else(|| {
                CycleError::new(
                    CycleErrorCode::DownstreamFailed,
                    "action terminal has no pending effect",
                )
            })?;
        self.execute_claimed(job)
    }

    fn execute_claimed(&self, job: EffectJob) -> Result<(), CycleError> {
        if crate::read_only_guard::observe_only_enabled() {
            self.handler
                .block_effect_authorization(
                    &job,
                    &effect_mutation_request_id(&job, "observe-only-block"),
                    "SIMARD_OBSERVE_ONLY denied mutation at effect dispatch",
                )
                .map_err(|error| {
                    CycleError::new(CycleErrorCode::PersistenceFailed, error.to_string())
                })?;
            return Err(CycleError::new(
                CycleErrorCode::DownstreamFailed,
                "effect is blocked by SIMARD_OBSERVE_ONLY",
            ));
        }
        if matches!(
            job.action,
            Action::RequestMerge(_) | Action::RequestDeploy(_)
        ) && job.approval.is_none()
        {
            self.handler
                .block_effect_authorization(
                    &job,
                    &effect_mutation_request_id(&job, "approval-block"),
                    "server-issued privileged approval is required",
                )
                .map_err(|error| {
                    CycleError::new(CycleErrorCode::PersistenceFailed, error.to_string())
                })?;
            return Err(CycleError::new(
                CycleErrorCode::DownstreamFailed,
                "privileged effect is blocked pending a server-issued approval",
            ));
        }
        let result = match self.effects.execute(&job) {
            Ok(result) => result,
            Err(error) => {
                if !error.permanent {
                    self.handler
                        .release_effect_for_retry(
                            &job,
                            &effect_mutation_request_id(&job, "retry"),
                            SystemTime::now(),
                            &error.to_string(),
                        )
                        .map_err(|failure| {
                            CycleError::new(CycleErrorCode::PersistenceFailed, failure.to_string())
                        })?;
                    return Err(CycleError::new(
                        CycleErrorCode::DownstreamFailed,
                        format!("retryable downstream effect failure: {error}"),
                    ));
                }
                let result = EffectResult::Failed {
                    error: error.to_string(),
                };
                self.handler
                    .finish_effect(
                        &job,
                        &effect_mutation_request_id(&job, "failed"),
                        SystemTime::now(),
                        &result,
                    )
                    .map_err(|failure| {
                        CycleError::new(CycleErrorCode::PersistenceFailed, failure.to_string())
                    })?;
                return Err(CycleError::new(
                    CycleErrorCode::DownstreamFailed,
                    format!("permanent downstream effect failure: {error}"),
                ));
            }
        };
        self.handler
            .finish_effect(
                &job,
                &effect_mutation_request_id(&job, "complete"),
                SystemTime::now(),
                &result,
            )
            .map_err(|error| {
                CycleError::new(CycleErrorCode::PersistenceFailed, error.to_string())
            })?;
        match result {
            EffectResult::Succeeded { .. } => Ok(()),
            EffectResult::Failed { error } => {
                Err(CycleError::new(CycleErrorCode::DownstreamFailed, error))
            }
        }
    }
}

fn effect_mutation_request_id(job: &EffectJob, operation: &str) -> String {
    format!("{}:{}:{}", job.effect_id, job.lease_generation, operation)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalSessionExecution {
    pub outcome: TerminalOutcome,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_error_exposes_code_and_message() {
        let error = CycleError::new(CycleErrorCode::MissingTerminal, "no durable terminal");
        assert_eq!(error.code(), CycleErrorCode::MissingTerminal);
        assert_eq!(error.to_string(), "no durable terminal");
    }

    #[test]
    fn recipe_process_error_nonzero_exit_names_the_status() {
        let error = RecipeProcessError::nonzero_exit(17);
        assert_eq!(error.to_string(), "recipe process exited with status 17");
    }

    #[test]
    fn recipe_process_error_failed_preserves_message() {
        let error = RecipeProcessError::failed("spawn refused");
        assert_eq!(error.to_string(), "spawn refused");
    }

    #[test]
    fn recipe_process_error_carries_capability_error_message() {
        let capability =
            CapabilityError::new(CapabilityErrorCode::PermissionDenied, "grant missing");
        let error: RecipeProcessError = capability.into();
        assert_eq!(error.to_string(), "grant missing");
    }

    #[test]
    fn effect_execution_error_permanent_is_not_retryable() {
        let error = EffectExecutionError::permanent("disk is gone");
        assert!(
            error.permanent,
            "permanent errors must set the permanent flag"
        );
        assert_eq!(error.to_string(), "disk is gone");
    }

    #[test]
    fn effect_execution_error_retryable_is_not_permanent() {
        let error = EffectExecutionError::retryable("transient network blip");
        assert!(
            !error.permanent,
            "retryable errors must leave the permanent flag clear"
        );
        assert_eq!(error.to_string(), "transient network blip");
    }
}
