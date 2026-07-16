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
    //! Hermetic unit coverage for the goal-session executor rails.
    //!
    //! These exercise the executor's decision logic directly against a real
    //! SQLite capability ledger in a tempdir plus in-process fake effect
    //! executors — no network, no subprocess, no shared fixtures. They cover
    //! branches the integration suite does not reach at the unit level
    //! (actor/session binding mismatches, retryable effect release, the
    //! standalone outbox worker) alongside the core terminal-rail contract.

    use super::*;
    use crate::read_only_guard::OBSERVE_ONLY_ENV;
    use crate::typed_ooda::{
        Action, ActionKind, AdmissionSnapshot, AuthenticatedToolContext, BaseType, CapabilityError,
        CapabilityErrorCode, CapabilityGrant, CapabilityHandler, CapabilityPolicy, OpaqueBytes,
        RepositoryRef, SpawnEngineerAction, TerminalKind,
    };
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const POLICY: &str = "goal-session-policy-v1";
    const SESSION: &str = "session-exec";
    const GOAL: &str = "goal-4052";

    struct SucceedingEffects;
    impl EffectExecutor for SucceedingEffects {
        fn execute(&self, _job: &EffectJob) -> Result<EffectResult, EffectExecutionError> {
            Ok(EffectResult::Succeeded {
                evidence: Vec::new(),
            })
        }
    }

    struct PermanentlyFailingEffects;
    impl EffectExecutor for PermanentlyFailingEffects {
        fn execute(&self, _job: &EffectJob) -> Result<EffectResult, EffectExecutionError> {
            Err(EffectExecutionError::permanent("permanent downstream boom"))
        }
    }

    struct RetryableEffects;
    impl EffectExecutor for RetryableEffects {
        fn execute(&self, _job: &EffectJob) -> Result<EffectResult, EffectExecutionError> {
            Err(EffectExecutionError::retryable(
                "transient downstream hiccup",
            ))
        }
    }

    /// Records how many times a job was handed to the effect layer so tests can
    /// prove the outbox actually executed the claimed work (not just that no
    /// error surfaced).
    struct CountingEffects {
        calls: Arc<AtomicUsize>,
    }
    impl EffectExecutor for CountingEffects {
        fn execute(&self, _job: &EffectJob) -> Result<EffectResult, EffectExecutionError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(EffectResult::Succeeded {
                evidence: Vec::new(),
            })
        }
    }

    /// Removes `SIMARD_OBSERVE_ONLY` for the lifetime of an effect-dispatching
    /// test and restores the prior value on drop. Effect dispatch consults the
    /// process-global observe-only flag; pairing this with the
    /// `cognitive_memory` serial group keeps the assertions hermetic even
    /// though other tests mutate the same variable.
    struct ObserveOnlyCleared {
        prev: Option<std::ffi::OsString>,
    }
    impl ObserveOnlyCleared {
        fn new() -> Self {
            let prev = std::env::var_os(OBSERVE_ONLY_ENV);
            unsafe {
                std::env::remove_var(OBSERVE_ONLY_ENV);
            }
            Self { prev }
        }
    }
    impl Drop for ObserveOnlyCleared {
        fn drop(&mut self) {
            unsafe {
                match self.prev.take() {
                    Some(value) => std::env::set_var(OBSERVE_ONLY_ENV, value),
                    None => std::env::remove_var(OBSERVE_ONLY_ENV),
                }
            }
        }
    }

    fn admission() -> AdmissionSnapshot {
        AdmissionSnapshot {
            concurrent_engineers: 0,
            disk_used_percent: 5,
            active_claims: BTreeSet::new(),
            policy_revision: POLICY.to_string(),
        }
    }

    fn spawn_grants() -> [CapabilityGrant; 4] {
        [
            CapabilityGrant::RecordAction(ActionKind::SpawnEngineer),
            CapabilityGrant::RecordNoAction,
            CapabilityGrant::RecordBlocked,
            CapabilityGrant::RecordCompleted,
        ]
    }

    fn actor(session: &str) -> AuthenticatedToolContext {
        AuthenticatedToolContext::new("goal-session-actor", session, spawn_grants())
            .scoped_to_repository(RepositoryRef::new("rysweet", "Simard"))
            .with_engineer_permissions(["repo_read", "repo_write"])
    }

    fn invocation(cycle: &str) -> GoalSessionInvocation {
        GoalSessionInvocation {
            session_id: SESSION.to_string(),
            cycle_id: cycle.to_string(),
            goal_id: GOAL.to_string(),
            task: OpaqueBytes::from(b"\nTASK:\0\xffraw task bytes\n".to_vec()),
            reason: OpaqueBytes::from(b"{\"reason\":\"raw\"}\n".to_vec()),
            observe_output: OpaqueBytes::from(b"observe\nNO ACTION\n".to_vec()),
            orient_output: OpaqueBytes::from(vec![0xff, 0xfe, b'O']),
            decide_output: OpaqueBytes::from(b"ACTION: SPAWN_ENGINEER is just prose".to_vec()),
        }
    }

    fn make_executor_with(
        session: &str,
        effects: Box<dyn EffectExecutor>,
    ) -> (tempfile::TempDir, GoalSessionExecutor) {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = CapabilityHandler::open(
            dir.path().join("outcomes.sqlite3"),
            CapabilityPolicy::goal_session_default(POLICY),
        )
        .expect("handler");
        (
            dir,
            GoalSessionExecutor::new(handler, actor(session), admission(), effects),
        )
    }

    fn make_executor(effects: Box<dyn EffectExecutor>) -> (tempfile::TempDir, GoalSessionExecutor) {
        make_executor_with(SESSION, effects)
    }

    fn spawn_action(task: OpaqueBytes) -> Action {
        Action::SpawnEngineer(SpawnEngineerAction {
            task,
            repository: RepositoryRef::new("rysweet", "Simard"),
            base_type: BaseType::Copilot,
            requested_permissions: BTreeSet::from(["repo_read".to_string()]),
            claim_key: format!("rysweet/Simard:{GOAL}"),
        })
    }

    // ---- Error value types ------------------------------------------------

    #[test]
    fn cycle_error_carries_code_and_display_message() {
        let err = CycleError::new(CycleErrorCode::ToolFailed, "boom");
        assert_eq!(err.code(), CycleErrorCode::ToolFailed);
        assert_eq!(err.to_string(), "boom");
    }

    #[test]
    fn recipe_process_error_variants_render_distinct_messages() {
        assert_eq!(
            RecipeProcessError::nonzero_exit(17).to_string(),
            "recipe process exited with status 17"
        );
        assert_eq!(
            RecipeProcessError::failed("no terminal").to_string(),
            "no terminal"
        );
    }

    #[test]
    fn recipe_process_error_preserves_capability_error_message() {
        let capability = CapabilityError::new(
            CapabilityErrorCode::PermissionDenied,
            "actor lacks the grant",
        );
        let recipe: RecipeProcessError = capability.into();
        assert_eq!(recipe.to_string(), "actor lacks the grant");
    }

    #[test]
    fn effect_execution_error_tracks_permanence_via_message_and_display() {
        assert_eq!(EffectExecutionError::permanent("gone").to_string(), "gone");
        assert_eq!(
            EffectExecutionError::retryable("later").to_string(),
            "later"
        );
    }

    // ---- Actor / session binding rails ------------------------------------

    #[test]
    fn session_mismatch_between_invocation_and_actor_is_a_tool_failure() {
        let (_dir, executor) = make_executor(Box::new(SucceedingEffects));
        let mut inv = invocation("cycle-session-mismatch");
        inv.session_id = "some-other-session".to_string();

        let err = executor
            .execute(&inv, |_received, _tools| Ok(()))
            .expect_err("invocation session must match the authenticated actor");
        assert_eq!(err.code(), CycleErrorCode::ToolFailed);
        assert!(
            err.to_string().contains("authenticated actor session"),
            "message should name the mismatch: {err}"
        );
    }

    #[test]
    fn actor_bound_to_a_different_cycle_and_goal_is_a_tool_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = CapabilityHandler::open(
            dir.path().join("outcomes.sqlite3"),
            CapabilityPolicy::goal_session_default(POLICY),
        )
        .expect("handler");
        let bound_actor = actor(SESSION).bound_to_cycle_goal("some-other-cycle", "some-other-goal");
        let executor = GoalSessionExecutor::new(
            handler,
            bound_actor,
            admission(),
            Box::new(SucceedingEffects),
        );

        let err = executor
            .execute(
                &invocation("cycle-binding-mismatch"),
                |_received, _tools| Ok(()),
            )
            .expect_err("a server-bound actor may only serve its bound cycle/goal");
        assert_eq!(err.code(), CycleErrorCode::ToolFailed);
        assert!(
            err.to_string().contains("server-bound cycle and goal"),
            "message should explain the binding mismatch: {err}"
        );
    }

    // ---- Terminal rails ---------------------------------------------------

    #[test]
    fn recipe_success_without_a_terminal_is_a_missing_terminal_failure() {
        let (_dir, executor) = make_executor(Box::new(SucceedingEffects));
        let err = executor
            .execute(
                &invocation("cycle-missing-terminal"),
                |_received, _tools| Ok(()),
            )
            .expect_err("process exit 0 is not a durable terminal");
        assert_eq!(err.code(), CycleErrorCode::MissingTerminal);
        assert_eq!(
            executor
                .handler()
                .terminal_count(SESSION, "cycle-missing-terminal")
                .expect("terminal count"),
            0
        );
    }

    #[test]
    fn recipe_process_error_surfaces_as_recipe_failed_without_a_terminal() {
        let (_dir, executor) = make_executor(Box::new(SucceedingEffects));
        let err = executor
            .execute(&invocation("cycle-recipe-failed"), |_received, _tools| {
                Err(RecipeProcessError::nonzero_exit(3))
            })
            .expect_err("a non-zero recipe exit fails the cycle");
        assert_eq!(err.code(), CycleErrorCode::RecipeFailed);
        assert!(err.to_string().contains("status 3"), "message: {err}");
        assert_eq!(
            executor
                .handler()
                .terminal_count(SESSION, "cycle-recipe-failed")
                .expect("terminal count"),
            0
        );
    }

    #[test]
    fn a_second_terminal_attempt_fails_the_cycle_and_keeps_only_the_first() {
        let (_dir, executor) = make_executor(Box::new(SucceedingEffects));
        let err = executor
            .execute(&invocation("cycle-double-terminal"), |received, tools| {
                tools.record_no_action(
                    "req-first",
                    OpaqueBytes::from(b"first".to_vec()),
                    received.decide_output.clone(),
                    Vec::new(),
                )?;
                let _ = tools.record_no_action(
                    "req-second",
                    OpaqueBytes::from(b"second".to_vec()),
                    received.decide_output.clone(),
                    Vec::new(),
                );
                Ok(())
            })
            .expect_err("exactly one terminal is permitted per cycle");
        assert_eq!(err.code(), CycleErrorCode::MultipleTerminalAttempts);
        assert_eq!(
            executor
                .handler()
                .terminal_count(SESSION, "cycle-double-terminal")
                .expect("terminal count"),
            1,
            "the first terminal is authoritative and is not replaced"
        );
    }

    #[test]
    fn a_failed_capability_call_fails_the_cycle_even_if_the_actor_swallows_it() {
        // An empty task with no requested permissions is rejected by the
        // capability handler; the actor ignores the error and returns Ok, but
        // the executor must still fail the cycle from the recorded failure.
        let (_dir, executor) = make_executor(Box::new(SucceedingEffects));
        let err = executor
            .execute(&invocation("cycle-tool-failed"), |_received, tools| {
                let _ = tools.record_action(
                    "req-bad-action",
                    Action::SpawnEngineer(SpawnEngineerAction {
                        task: OpaqueBytes::from(Vec::new()),
                        repository: RepositoryRef::new("rysweet", "Simard"),
                        base_type: BaseType::Copilot,
                        requested_permissions: BTreeSet::new(),
                        claim_key: format!("rysweet/Simard:{GOAL}"),
                    }),
                    OpaqueBytes::from(Vec::new()),
                    Vec::new(),
                );
                Ok(())
            })
            .expect_err("a swallowed capability error still fails the cycle");
        assert_eq!(err.code(), CycleErrorCode::ToolFailed);
        assert_eq!(
            executor
                .handler()
                .terminal_count(SESSION, "cycle-tool-failed")
                .expect("terminal count"),
            0
        );
    }

    #[test]
    fn no_action_terminal_succeeds_from_its_durable_record_and_skips_effects() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (_dir, executor) = make_executor(Box::new(CountingEffects {
            calls: calls.clone(),
        }));
        let inv = invocation("cycle-no-action");
        let execution = executor
            .execute(&inv, |received, tools| {
                tools.record_no_action(
                    "req-no-action",
                    received.reason.clone(),
                    received.decide_output.clone(),
                    Vec::new(),
                )?;
                Ok(())
            })
            .expect("durable no-action terminal");
        assert_eq!(execution.outcome.kind, TerminalKind::NoAction);
        assert_eq!(
            execution
                .outcome
                .payload
                .no_action()
                .expect("no-action payload")
                .reason
                .as_bytes(),
            inv.reason.as_bytes(),
            "the recorded reason is the raw bytes the actor supplied"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "a no-action terminal never dispatches a downstream effect"
        );
    }

    // ---- Effect dispatch rails (observe-only sensitive) -------------------

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn action_terminal_completes_only_after_its_effect_succeeds() {
        let _observe = ObserveOnlyCleared::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let (_dir, executor) = make_executor(Box::new(CountingEffects {
            calls: calls.clone(),
        }));
        let inv = invocation("cycle-action-success");
        let expected_task = inv.task.clone();

        let execution = executor
            .execute(&inv, |received, tools| {
                // The actor receives the raw bytes byte-for-byte.
                assert_eq!(received.task.as_bytes(), expected_task.as_bytes());
                tools.record_action(
                    "req-action-success",
                    spawn_action(received.task.clone()),
                    received.decide_output.clone(),
                    Vec::new(),
                )?;
                Ok(())
            })
            .expect("action terminal plus successful effect");

        assert_eq!(execution.outcome.kind, TerminalKind::Action);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the effect ran exactly once"
        );
        assert_eq!(
            executor
                .handler()
                .effect_for_outcome(&execution.outcome.outcome_id)
                .expect("effect query")
                .expect("action effect exists")
                .state
                .as_str(),
            "succeeded"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn permanent_effect_failure_fails_the_cycle_but_keeps_the_action_terminal() {
        let _observe = ObserveOnlyCleared::new();
        let (_dir, executor) = make_executor(Box::new(PermanentlyFailingEffects));
        let inv = invocation("cycle-perm-effect-fail");

        let err = executor
            .execute(&inv, |received, tools| {
                tools.record_action(
                    "req-perm-fail",
                    spawn_action(received.task.clone()),
                    received.decide_output.clone(),
                    Vec::new(),
                )?;
                Ok(())
            })
            .expect_err("a permanent downstream failure fails the cycle");
        assert_eq!(err.code(), CycleErrorCode::DownstreamFailed);

        let terminal = executor
            .handler()
            .terminal_for_cycle(SESSION, "cycle-perm-effect-fail")
            .expect("terminal query")
            .expect("the semantic action terminal remains authoritative");
        assert_eq!(terminal.kind, TerminalKind::Action);
        assert_eq!(
            executor
                .handler()
                .effect_for_outcome(&terminal.outcome_id)
                .expect("effect query")
                .expect("failed effect")
                .state
                .as_str(),
            "failed"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn retryable_effect_failure_releases_the_effect_back_to_pending() {
        let _observe = ObserveOnlyCleared::new();
        let (_dir, executor) = make_executor(Box::new(RetryableEffects));
        let inv = invocation("cycle-retryable-effect");

        let err = executor
            .execute(&inv, |received, tools| {
                tools.record_action(
                    "req-retryable",
                    spawn_action(received.task.clone()),
                    received.decide_output.clone(),
                    Vec::new(),
                )?;
                Ok(())
            })
            .expect_err("a retryable downstream failure fails this attempt");
        assert_eq!(err.code(), CycleErrorCode::DownstreamFailed);

        let terminal = executor
            .handler()
            .terminal_for_cycle(SESSION, "cycle-retryable-effect")
            .expect("terminal query")
            .expect("action terminal");
        let effect = executor
            .handler()
            .effect_for_outcome(&terminal.outcome_id)
            .expect("effect query")
            .expect("effect exists");
        assert_eq!(
            effect.state.as_str(),
            "pending",
            "a retryable failure returns the effect to the outbox for another worker"
        );
        assert_eq!(
            effect.error.as_deref(),
            Some("transient downstream hiccup"),
            "the retry reason is preserved on the released effect"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn outbox_worker_drains_a_pending_effect_left_by_the_actor_step() {
        let _observe = ObserveOnlyCleared::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let (_dir, executor) = make_executor(Box::new(SucceedingEffects));
        let inv = invocation("cycle-drain-pending");

        // Only run the actor step: this records the action terminal and leaves
        // a *pending* effect in the outbox without dispatching it.
        let execution = executor
            .execute_actor_step(&inv, |received, tools| {
                tools.record_action(
                    "req-drain",
                    spawn_action(received.task.clone()),
                    received.decide_output.clone(),
                    Vec::new(),
                )?;
                Ok(())
            })
            .expect("action terminal recorded");
        assert_eq!(execution.outcome.kind, TerminalKind::Action);
        assert_eq!(
            executor
                .handler()
                .effect_for_outcome(&execution.outcome.outcome_id)
                .expect("effect query")
                .expect("pending effect")
                .state
                .as_str(),
            "pending",
            "the actor step leaves the effect undispatched"
        );

        let counting = CountingEffects {
            calls: calls.clone(),
        };
        let worker = OutboxWorker::new(
            executor.handler(),
            &counting,
            "test-outbox-worker",
            Duration::from_secs(300),
        );
        let drained = worker.drain_pending(8).expect("drain the outbox");
        assert_eq!(drained, 1, "exactly one pending effect was claimed and run");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            executor
                .handler()
                .effect_for_outcome(&execution.outcome.outcome_id)
                .expect("effect query")
                .expect("effect")
                .state
                .as_str(),
            "succeeded",
            "draining transitions the effect to its terminal success state"
        );

        // A second drain has no pending work and completes zero jobs.
        assert_eq!(worker.drain_pending(8).expect("second drain"), 0);
    }
}
