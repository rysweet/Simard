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
    fn new(code: CycleErrorCode, message: impl Into<String>) -> Self {
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

    pub(crate) fn note_tool_failure(&self, error: impl Into<String>) {
        let mut state = self.state.lock().unwrap_or_else(|value| value.into_inner());
        state.failed = Some(error.into());
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
        if invocation.session_id != self.actor.session_id {
            return Err(CycleError::new(
                CycleErrorCode::ToolFailed,
                "goal-session invocation does not match authenticated actor session",
            ));
        }
        let tools = GoalSessionTools {
            handler: &self.handler,
            actor: &self.actor,
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
        if outcome.kind == TerminalKind::Action {
            self.execute_effect(&outcome, effects)?;
        }
        Ok(GoalSessionExecution { outcome })
    }

    fn execute_effect(
        &self,
        outcome: &TerminalOutcome,
        effects: &dyn EffectExecutor,
    ) -> Result<(), CycleError> {
        let job = self
            .handler
            .claim_effect_for_outcome(
                &outcome.outcome_id,
                "goal-session-effect-dispatcher",
                SystemTime::now(),
                Duration::from_secs(300),
            )
            .map_err(|error| CycleError::new(CycleErrorCode::PersistenceFailed, error.to_string()))?
            .ok_or_else(|| {
                CycleError::new(
                    CycleErrorCode::DownstreamFailed,
                    "action terminal has no pending effect",
                )
            })?;
        let result = match effects.execute(&job) {
            Ok(result) => result,
            Err(error) => {
                let result = EffectResult::Failed {
                    error: error.to_string(),
                };
                self.handler
                    .finish_effect(&job.effect_id, &result)
                    .map_err(|failure| {
                        CycleError::new(CycleErrorCode::PersistenceFailed, failure.to_string())
                    })?;
                let qualifier = if error.permanent {
                    "permanent"
                } else {
                    "retryable"
                };
                return Err(CycleError::new(
                    CycleErrorCode::DownstreamFailed,
                    format!("{qualifier} downstream effect failure: {error}"),
                ));
            }
        };
        self.handler
            .finish_effect(&job.effect_id, &result)
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalSessionExecution {
    pub outcome: TerminalOutcome,
}
