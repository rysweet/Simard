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
    /// When set, the effect could not run because the goal record was
    /// legitimately completed/removed (or otherwise concurrently mutated out
    /// from under the prepared effect) between prepare and dispatch. This is a
    /// benign race, not a failure: the dispatcher closes the outbox row as a
    /// counted, structured no-op instead of mapping it to
    /// `CycleErrorCode::DownstreamFailed`. A benign no-op is also `permanent`
    /// so it can never be mistaken for a retryable failure if this flag is ever
    /// dropped on the floor.
    no_op: bool,
}

impl EffectExecutionError {
    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            permanent: true,
            no_op: false,
        }
    }

    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            permanent: false,
            no_op: false,
        }
    }

    /// A benign goal-lifecycle race: the goal was legitimately
    /// completed/removed between preparing this effect and dispatching it, so
    /// there is nothing left to do. Structurally `permanent` (never retried)
    /// and flagged `no_op` so the dispatcher records a counted no-op outcome
    /// rather than a `DownstreamFailed` cycle error.
    pub fn benign_no_op(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            permanent: true,
            no_op: true,
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
                if error.no_op {
                    // Benign goal-lifecycle race (issue #4468): the goal was
                    // legitimately completed/removed between preparing this
                    // effect and dispatching it. Do NOT map this to
                    // DownstreamFailed. Close the outbox row as a succeeded
                    // no-op (empty evidence) so it is never redispatched, emit
                    // a structured tracing event, and increment a counter so
                    // the race stays observable rather than silently swallowed.
                    tracing::warn!(
                        target: "typed_ooda.effect_dispatch",
                        effect_id = %job.effect_id,
                        outcome_id = %job.outcome_id,
                        goal_id = %job.goal_id,
                        reason = %error,
                        "typed goal-session effect skipped as benign no-op: goal completed or removed between prepare and dispatch",
                    );
                    let _ = crate::self_metrics::record_metric(
                        "typed_ooda_effect_benign_no_op",
                        1.0,
                        &format!(
                            "goal={};effect={};outcome={};reason={}",
                            job.goal_id, job.effect_id, job.outcome_id, error
                        ),
                    );
                    let result = EffectResult::Succeeded { evidence: vec![] };
                    self.handler
                        .finish_effect(
                            &job,
                            &effect_mutation_request_id(&job, "noop"),
                            SystemTime::now(),
                            &result,
                        )
                        .map_err(|failure| {
                            CycleError::new(CycleErrorCode::PersistenceFailed, failure.to_string())
                        })?;
                    return Ok(());
                }
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
    use crate::typed_ooda::{
        Action, ActionKind, AdmissionSnapshot, AuthenticatedToolContext, CapabilityError,
        CapabilityErrorCode, CapabilityGrant, CapabilityHandler, CapabilityPolicy, FileIssueAction,
        OpaqueBytes, PullRequestRef, RepositoryRef, RequestMergeAction, TerminalKind,
    };
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // A deterministic effect executor whose behaviour is fixed per test and
    // which records how many times it was invoked. This lets tests assert that
    // the outbox worker only executes an effect exactly when it should.
    enum FakeMode {
        Succeed,
        Permanent,
        Retryable,
        // Models the systemic race this fix targets: between preparing a
        // goal-session effect and dispatching it, the goal record was
        // legitimately completed/removed, so the executor reports a benign,
        // structured no-op instead of a terminal failure.
        BenignNoOp,
    }

    struct FakeEffects {
        mode: FakeMode,
        calls: AtomicUsize,
    }

    impl FakeEffects {
        fn new(mode: FakeMode) -> Self {
            Self {
                mode,
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl EffectExecutor for FakeEffects {
        fn execute(&self, _job: &EffectJob) -> Result<EffectResult, EffectExecutionError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.mode {
                FakeMode::Succeed => Ok(EffectResult::Succeeded {
                    evidence: Vec::new(),
                }),
                FakeMode::Permanent => Err(EffectExecutionError::permanent("permanent boom")),
                FakeMode::Retryable => Err(EffectExecutionError::retryable("transient boom")),
                FakeMode::BenignNoOp => Err(EffectExecutionError::benign_no_op(
                    "goal disappeared before effect dispatch",
                )),
            }
        }
    }

    fn handler() -> CapabilityHandler {
        let dir = tempfile::tempdir().expect("tempdir");
        // Leak the tempdir so the sqlite file outlives the handler for the whole
        // test; the OS reclaims it when the test process exits.
        let path = dir.keep().join("outcomes.sqlite3");
        CapabilityHandler::open(path, CapabilityPolicy::new("policy-v1")).expect("open handler")
    }

    fn admission() -> AdmissionSnapshot {
        AdmissionSnapshot {
            concurrent_engineers: 0,
            disk_used_percent: 1,
            active_claims: BTreeSet::new(),
            policy_revision: "policy-v1".to_string(),
        }
    }

    fn invocation(session: &str, cycle: &str, goal: &str) -> GoalSessionInvocation {
        GoalSessionInvocation {
            session_id: session.to_string(),
            cycle_id: cycle.to_string(),
            goal_id: goal.to_string(),
            task: OpaqueBytes::from(b"task".to_vec()),
            reason: OpaqueBytes::from(b"reason".to_vec()),
            observe_output: OpaqueBytes::from(b"observe".to_vec()),
            orient_output: OpaqueBytes::from(b"orient".to_vec()),
            decide_output: OpaqueBytes::from(b"decide".to_vec()),
        }
    }

    fn no_action_actor(session: &str) -> AuthenticatedToolContext {
        AuthenticatedToolContext::new(
            "goal-session-actor",
            session,
            [CapabilityGrant::RecordNoAction],
        )
    }

    fn file_issue_action() -> Action {
        Action::FileIssue(FileIssueAction {
            repository: RepositoryRef::new("rysweet", "Simard"),
            title: OpaqueBytes::from(b"a real issue title".to_vec()),
            body: OpaqueBytes::from(b"body".to_vec()),
            labels: Vec::new(),
        })
    }

    fn merge_action() -> Action {
        Action::RequestMerge(RequestMergeAction {
            pull_request: PullRequestRef {
                repository: RepositoryRef::new("rysweet", "Simard"),
                number: 7,
            },
            expected_head_sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
            strategy: "squash".to_string(),
        })
    }

    #[test]
    fn error_types_expose_code_message_and_conversions() {
        let cycle = CycleError::new(CycleErrorCode::ToolFailed, "boom");
        assert_eq!(cycle.code(), CycleErrorCode::ToolFailed);
        assert_eq!(cycle.to_string(), "boom");

        let nonzero = RecipeProcessError::nonzero_exit(3);
        assert_eq!(nonzero.to_string(), "recipe process exited with status 3");
        assert_eq!(RecipeProcessError::failed("nope").to_string(), "nope");

        let from_capability: RecipeProcessError =
            CapabilityError::new(CapabilityErrorCode::PermissionDenied, "denied").into();
        assert_eq!(from_capability.to_string(), "denied");

        assert_eq!(EffectExecutionError::permanent("dead").to_string(), "dead");
        assert_eq!(
            EffectExecutionError::retryable("later").to_string(),
            "later"
        );
    }

    #[test]
    fn execute_actor_step_records_a_no_action_terminal() {
        let executor = GoalSessionExecutor::new(
            handler(),
            no_action_actor("session-1"),
            admission(),
            Box::new(FakeEffects::new(FakeMode::Succeed)),
        );
        assert_eq!(
            executor
                .handler()
                .terminal_for_cycle("session-1", "cycle-1")
                .unwrap(),
            None
        );

        let inv = invocation("session-1", "cycle-1", "goal-1");
        let execution = executor
            .execute(&inv, |inv, tools| {
                tools
                    .record_no_action(
                        "req-noaction",
                        OpaqueBytes::from(b"nothing to do".to_vec()),
                        inv.reason.clone(),
                        Vec::new(),
                    )
                    .map(|_| ())
                    .map_err(RecipeProcessError::from)
            })
            .expect("no-action execution succeeds");
        assert_eq!(execution.outcome.kind, TerminalKind::NoAction);
        // A no-action terminal has no downstream effect, so complete_outcome_effect
        // must be a no-op and never touch the effect executor.
        let durable = executor
            .handler()
            .terminal_for_cycle("session-1", "cycle-1")
            .unwrap()
            .expect("durable terminal persisted");
        assert_eq!(durable.outcome_id, execution.outcome.outcome_id);
    }

    #[test]
    fn execute_actor_step_rejects_session_mismatch() {
        let executor = GoalSessionExecutor::new(
            handler(),
            no_action_actor("session-real"),
            admission(),
            Box::new(FakeEffects::new(FakeMode::Succeed)),
        );
        let inv = invocation("session-other", "cycle-1", "goal-1");
        let error = executor
            .execute_actor_step(&inv, |_, _| Ok(()))
            .expect_err("session mismatch must fail");
        assert_eq!(error.code(), CycleErrorCode::ToolFailed);
    }

    #[test]
    fn execute_actor_step_rejects_bound_cycle_goal_mismatch() {
        let actor = no_action_actor("session-1").bound_to_cycle_goal("other-cycle", "other-goal");
        let executor = GoalSessionExecutor::new(
            handler(),
            actor,
            admission(),
            Box::new(FakeEffects::new(FakeMode::Succeed)),
        );
        let inv = invocation("session-1", "cycle-1", "goal-1");
        let error = executor
            .execute_actor_step(&inv, |_, _| Ok(()))
            .expect_err("bound cycle/goal mismatch must fail");
        assert_eq!(error.code(), CycleErrorCode::ToolFailed);
    }

    #[test]
    fn execute_actor_step_rejects_multiple_terminal_attempts() {
        let executor = GoalSessionExecutor::new(
            handler(),
            no_action_actor("session-1"),
            admission(),
            Box::new(FakeEffects::new(FakeMode::Succeed)),
        );
        let inv = invocation("session-1", "cycle-1", "goal-1");
        let error = executor
            .execute_actor_step(&inv, |inv, tools| {
                let _ = tools.record_no_action(
                    "req-1",
                    OpaqueBytes::from(b"first".to_vec()),
                    inv.reason.clone(),
                    Vec::new(),
                );
                let _ = tools.record_no_action(
                    "req-2",
                    OpaqueBytes::from(b"second".to_vec()),
                    inv.reason.clone(),
                    Vec::new(),
                );
                Ok(())
            })
            .expect_err("two terminal calls must fail");
        assert_eq!(error.code(), CycleErrorCode::MultipleTerminalAttempts);
    }

    #[test]
    fn execute_actor_step_surfaces_failed_capability_as_tool_failed() {
        let executor = GoalSessionExecutor::new(
            handler(),
            no_action_actor("session-1"),
            admission(),
            Box::new(FakeEffects::new(FakeMode::Succeed)),
        );
        let inv = invocation("session-1", "cycle-1", "goal-1");
        let error = executor
            .execute_actor_step(&inv, |inv, tools| {
                // Empty reason is rejected by the ledger; the tool records the
                // failure, and the executor must convert it to ToolFailed.
                let _ = tools.record_no_action(
                    "req-empty",
                    OpaqueBytes::from(Vec::new()),
                    inv.reason.clone(),
                    Vec::new(),
                );
                Ok(())
            })
            .expect_err("failed capability must fail");
        assert_eq!(error.code(), CycleErrorCode::ToolFailed);
    }

    #[test]
    fn execute_actor_step_maps_recipe_process_error() {
        let executor = GoalSessionExecutor::new(
            handler(),
            no_action_actor("session-1"),
            admission(),
            Box::new(FakeEffects::new(FakeMode::Succeed)),
        );
        let inv = invocation("session-1", "cycle-1", "goal-1");
        let error = executor
            .execute_actor_step(&inv, |_, _| {
                Err(RecipeProcessError::failed("actor crashed"))
            })
            .expect_err("recipe process failure must propagate");
        assert_eq!(error.code(), CycleErrorCode::RecipeFailed);
        assert!(error.to_string().contains("actor crashed"));
    }

    #[test]
    fn execute_actor_step_requires_a_durable_terminal() {
        let executor = GoalSessionExecutor::new(
            handler(),
            no_action_actor("session-1"),
            admission(),
            Box::new(FakeEffects::new(FakeMode::Succeed)),
        );
        let inv = invocation("session-1", "cycle-1", "goal-1");
        let error = executor
            .execute_actor_step(&inv, |_, _| Ok(()))
            .expect_err("no terminal recorded must fail");
        assert_eq!(error.code(), CycleErrorCode::MissingTerminal);
    }

    #[test]
    fn execute_with_effects_dispatches_a_successful_action_effect() {
        let actor = AuthenticatedToolContext::new(
            "goal-session-actor",
            "session-1",
            [CapabilityGrant::RecordAction(ActionKind::FileIssue)],
        )
        .scoped_to_repository(RepositoryRef::new("rysweet", "Simard"));
        let executor = GoalSessionExecutor::new(
            handler(),
            actor,
            admission(),
            Box::new(FakeEffects::new(FakeMode::Succeed)),
        );
        let effects = FakeEffects::new(FakeMode::Succeed);
        let inv = invocation("session-1", "cycle-1", "goal-1");
        let execution = executor
            .execute_with_effects(&inv, &effects, |_, tools| {
                tools
                    .record_action(
                        "req-file-issue",
                        file_issue_action(),
                        OpaqueBytes::from(b"raw".to_vec()),
                        Vec::new(),
                    )
                    .map(|_| ())
                    .map_err(RecipeProcessError::from)
            })
            .expect("action execution succeeds");
        assert_eq!(execution.outcome.kind, TerminalKind::Action);
        assert_eq!(
            effects.calls(),
            1,
            "the pending effect must be executed once"
        );

        // The effect is now succeeded; a second dispatch is a no-op and must not
        // execute the effect again.
        let worker = OutboxWorker::new(
            executor.handler(),
            &effects,
            "test-worker",
            Duration::from_secs(60),
        );
        worker
            .dispatch_outcome(&execution.outcome)
            .expect("idempotent redispatch");
        assert_eq!(effects.calls(), 1, "succeeded effect must not re-run");
    }

    fn record_pending_action(handler: &CapabilityHandler, action: Action) -> TerminalOutcome {
        let actor = AuthenticatedToolContext::new(
            "goal-session-actor",
            "session-1",
            [CapabilityGrant::RecordAction(action.kind())],
        )
        .scoped_to_repository(RepositoryRef::new("rysweet", "Simard"))
        .bound_to_cycle_goal("cycle-1", "goal-1");
        handler
            .record_action(
                &actor,
                crate::typed_ooda::RecordActionRequest {
                    identity: crate::typed_ooda::TerminalRequestIdentity::new(
                        "req-action",
                        "session-1",
                        "cycle-1",
                        "goal-1",
                    ),
                    action,
                    raw_semantic: OpaqueBytes::from(b"raw".to_vec()),
                    evidence: Vec::new(),
                },
                &AdmissionSnapshot {
                    concurrent_engineers: 0,
                    disk_used_percent: 1,
                    active_claims: BTreeSet::new(),
                    policy_revision: "policy-v1".to_string(),
                },
            )
            .expect("record action")
    }

    #[test]
    fn outbox_worker_retries_transient_effect_failures() {
        let handler = handler();
        let outcome = record_pending_action(&handler, file_issue_action());
        let effects = FakeEffects::new(FakeMode::Retryable);
        let worker = OutboxWorker::new(&handler, &effects, "test-worker", Duration::from_secs(60));
        let error = worker
            .dispatch_outcome(&outcome)
            .expect_err("retryable failure must surface");
        assert_eq!(error.code(), CycleErrorCode::DownstreamFailed);
        assert!(error.to_string().contains("retryable"));
        // The effect returns to pending, so a fresh worker can claim it again.
        let job = handler
            .effect_for_outcome(&outcome.outcome_id)
            .expect("query effect")
            .expect("effect");
        assert_eq!(job.state.as_str(), "pending");
    }

    #[test]
    fn outbox_worker_records_permanent_effect_failures() {
        let handler = handler();
        let outcome = record_pending_action(&handler, file_issue_action());
        let effects = FakeEffects::new(FakeMode::Permanent);
        let worker = OutboxWorker::new(&handler, &effects, "test-worker", Duration::from_secs(60));
        let error = worker
            .dispatch_outcome(&outcome)
            .expect_err("permanent failure must surface");
        assert_eq!(error.code(), CycleErrorCode::DownstreamFailed);
        let job = handler
            .effect_for_outcome(&outcome.outcome_id)
            .expect("query effect")
            .expect("effect");
        assert_eq!(job.state.as_str(), "failed");
    }

    // ---------------------------------------------------------------------
    // Regression: benign goal-removed race at effect dispatch.
    //
    // The systemic defect (issue #4468): a prepared goal-session effect is
    // dispatched after the goal record was legitimately completed/removed
    // between prepare and dispatch. Today the executor surfaces this as
    // EffectExecutionError::permanent("goal disappeared before effect
    // dispatch"), which maps to CycleErrorCode::DownstreamFailed and fails
    // the OODA cycle. It must instead be a benign, structured, counted no-op:
    // the outbox row is closed as succeeded (never redispatched) and the
    // cycle completes with Ok(()).
    // ---------------------------------------------------------------------

    #[test]
    fn benign_no_op_constructor_is_permanent_and_flagged() {
        // Defense-in-depth: a benign no-op is still `permanent` (so it can
        // never be mistaken for a retryable failure) AND carries the explicit
        // `no_op` discriminator that routes it to the counted-no-op arm.
        let error = EffectExecutionError::benign_no_op("goal disappeared before effect dispatch");
        assert!(
            error.permanent,
            "a benign no-op must remain permanent as a safety net"
        );
        assert!(
            error.no_op,
            "a benign no-op must set the no_op discriminator"
        );
        assert_eq!(error.to_string(), "goal disappeared before effect dispatch");

        // The existing constructors must NOT set the no_op flag, or a real
        // failure could be silently swallowed as a success.
        assert!(!EffectExecutionError::permanent("boom").no_op);
        assert!(!EffectExecutionError::retryable("later").no_op);
    }

    #[test]
    fn dispatch_after_goal_removed_is_benign_no_op() {
        let handler = handler();
        let outcome = record_pending_action(&handler, file_issue_action());
        let effects = FakeEffects::new(FakeMode::BenignNoOp);
        let worker = OutboxWorker::new(&handler, &effects, "test-worker", Duration::from_secs(60));

        // The goal was legitimately removed between prepare and dispatch. This
        // MUST complete the cycle successfully, not raise DownstreamFailed.
        worker
            .dispatch_outcome(&outcome)
            .expect("a benign goal-removed race must complete as a no-op, not DownstreamFailed");
        assert_eq!(
            effects.calls(),
            1,
            "the effect is attempted exactly once before the benign no-op"
        );

        // The outbox row is closed as succeeded so the effect is never
        // redispatched by a later cycle or startup recovery.
        let job = handler
            .effect_for_outcome(&outcome.outcome_id)
            .expect("query effect")
            .expect("effect");
        assert_eq!(
            job.state.as_str(),
            "succeeded",
            "a benign no-op must close the outbox row, not leave it pending/failed"
        );

        // Idempotent redispatch must observe the closed row and must not
        // re-run the effect executor.
        worker
            .dispatch_outcome(&outcome)
            .expect("idempotent redispatch of a closed benign no-op");
        assert_eq!(
            effects.calls(),
            1,
            "a benign no-op effect must never be re-run"
        );
    }

    #[test]
    fn permanent_effect_failure_is_not_treated_as_benign_no_op() {
        // Negative guard for the no_op reordering risk: a genuine permanent
        // failure (no no_op flag) must still fail the cycle with
        // DownstreamFailed and record the effect as `failed` — never swallowed
        // into a success by the benign-no-op arm.
        let handler = handler();
        let outcome = record_pending_action(&handler, file_issue_action());
        let effects = FakeEffects::new(FakeMode::Permanent);
        let worker = OutboxWorker::new(&handler, &effects, "test-worker", Duration::from_secs(60));
        let error = worker
            .dispatch_outcome(&outcome)
            .expect_err("a real permanent failure must surface as DownstreamFailed");
        assert_eq!(error.code(), CycleErrorCode::DownstreamFailed);
        let job = handler
            .effect_for_outcome(&outcome.outcome_id)
            .expect("query effect")
            .expect("effect");
        assert_ne!(
            job.state.as_str(),
            "succeeded",
            "a permanent failure must never be closed as succeeded"
        );
        assert_eq!(job.state.as_str(), "failed");
    }

    #[test]
    fn outbox_worker_blocks_privileged_effects_without_approval() {
        let handler = handler();
        let outcome = record_pending_action(&handler, merge_action());
        let effects = FakeEffects::new(FakeMode::Succeed);
        let worker = OutboxWorker::new(&handler, &effects, "test-worker", Duration::from_secs(60));
        let error = worker
            .dispatch_outcome(&outcome)
            .expect_err("privileged effect without approval must be blocked");
        assert_eq!(error.code(), CycleErrorCode::DownstreamFailed);
        assert_eq!(effects.calls(), 0, "blocked effect must not execute");
        assert!(error.to_string().contains("approval"));
    }

    #[test]
    fn outbox_worker_drains_pending_effects() {
        let handler = handler();
        let outcome = record_pending_action(&handler, file_issue_action());
        let effects = FakeEffects::new(FakeMode::Succeed);
        let worker = OutboxWorker::new(&handler, &effects, "test-worker", Duration::from_secs(60));
        let drained = worker.drain_pending(8).expect("drain succeeds");
        assert_eq!(drained, 1);
        assert_eq!(effects.calls(), 1);
        let job = handler
            .effect_for_outcome(&outcome.outcome_id)
            .expect("query effect")
            .expect("effect");
        assert_eq!(job.state.as_str(), "succeeded");
        // Nothing left to drain.
        assert_eq!(worker.drain_pending(8).expect("second drain"), 0);
    }
}
