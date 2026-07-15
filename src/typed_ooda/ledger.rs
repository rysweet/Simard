use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::types::*;

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
pub struct EffectKind(String);

impl EffectKind {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
pub struct EffectState(String);

impl EffectState {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
pub struct EffectJob {
    pub effect_id: String,
    pub outcome_id: String,
    pub request_id: String,
    pub goal_id: String,
    pub repository: Option<RepositoryRef>,
    pub kind: EffectKind,
    pub state: EffectState,
    pub action: Action,
    pub attempt: u32,
    pub lease_generation: u64,
    pub lease_owner: Option<String>,
    pub lease_expires_at_unix_millis: Option<i64>,
    pub error: Option<String>,
    pub result: Option<EffectResult>,
    pub approval: Option<PrivilegedApproval>,
}

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorSessionLease {
    pub token: String,
    pub expires_at_unix_millis: i64,
}

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
pub struct PrivilegedApproval {
    pub approval_id: String,
    pub principal: String,
    pub effect_id: String,
    pub outcome_id: String,
    pub session_id: String,
    pub cycle_id: String,
    pub goal_id: String,
    pub action_kind: ActionKind,
    pub canonical_payload_hash: String,
    pub repository: Option<RepositoryRef>,
    pub policy_revision: String,
    pub issued_at_unix_millis: i64,
    pub signature: String,
}

#[derive(Clone, Debug)]
pub struct ApprovalAuthority {
    principal: String,
    signing_key: Vec<u8>,
}

impl ApprovalAuthority {
    pub fn from_environment() -> CapabilityResult<Self> {
        let principal = std::env::var("SIMARD_PRIVILEGED_PRINCIPAL").map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::Unauthenticated,
                "SIMARD_PRIVILEGED_PRINCIPAL is required to issue an approval",
            )
        })?;
        validate_identifier("privileged principal", &principal)?;
        let signing_key = std::env::var("SIMARD_PRIVILEGED_APPROVAL_KEY")
            .map_err(|_| {
                CapabilityError::new(
                    CapabilityErrorCode::Unauthenticated,
                    "SIMARD_PRIVILEGED_APPROVAL_KEY is required to issue an approval",
                )
            })?
            .into_bytes();
        if signing_key.len() < 32 {
            return Err(CapabilityError::new(
                CapabilityErrorCode::Unauthenticated,
                "SIMARD_PRIVILEGED_APPROVAL_KEY must contain at least 32 bytes",
            ));
        }
        Ok(Self {
            principal,
            signing_key,
        })
    }

    #[cfg(test)]
    pub fn for_test(principal: &str) -> Self {
        Self {
            principal: principal.to_string(),
            signing_key: vec![0x5a; 32],
        }
    }

    pub fn verifies(&self, approval: &PrivilegedApproval) -> CapabilityResult<bool> {
        if approval.principal != self.principal {
            return Ok(false);
        }
        let expected = approval_signature(&self.signing_key, approval)?;
        Ok(constant_time_eq(
            expected.as_bytes(),
            approval.signature.as_bytes(),
        ))
    }
}

pub struct CapabilityHandler {
    connection: Mutex<Connection>,
    policy: CapabilityPolicy,
}

#[derive(Debug, Eq, PartialEq)]
struct ActorBinding {
    cycle_id: String,
    goal_id: String,
    actor_identity: String,
    repository_json: Vec<u8>,
    grants_json: Vec<u8>,
    engineer_permissions_json: Vec<u8>,
    working_directory_json: Option<Vec<u8>>,
    observe_only: bool,
}

struct StoredActorSession {
    binding: ActorBinding,
    token_hash: String,
    expires_at: i64,
}

impl ActorBinding {
    fn new(
        actor: &AuthenticatedToolContext,
        cycle_id: &str,
        goal_id: &str,
        repository: &RepositoryRef,
    ) -> CapabilityResult<Self> {
        Ok(Self {
            cycle_id: cycle_id.to_string(),
            goal_id: goal_id.to_string(),
            actor_identity: actor.actor_identity.clone(),
            repository_json: serde_json::to_vec(repository).map_err(serialization)?,
            grants_json: serde_json::to_vec(actor.grants()).map_err(serialization)?,
            engineer_permissions_json: serde_json::to_vec(actor.engineer_permissions())
                .map_err(serialization)?,
            working_directory_json: actor
                .bound_working_directory()
                .map(serde_json::to_vec)
                .transpose()
                .map_err(serialization)?,
            observe_only: actor.is_observe_only(),
        })
    }

    fn into_context(self, session_id: &str) -> CapabilityResult<AuthenticatedToolContext> {
        let repository: RepositoryRef =
            serde_json::from_slice(&self.repository_json).map_err(serialization)?;
        let grants: BTreeSet<CapabilityGrant> =
            serde_json::from_slice(&self.grants_json).map_err(serialization)?;
        let engineer_permissions: BTreeSet<String> =
            serde_json::from_slice(&self.engineer_permissions_json).map_err(serialization)?;
        let mut actor = AuthenticatedToolContext::new(self.actor_identity, session_id, grants)
            .scoped_to_repository(repository)
            .bound_to_cycle_goal(self.cycle_id, self.goal_id)
            .with_engineer_permissions(engineer_permissions)
            .with_observe_only(self.observe_only);
        if let Some(json) = self.working_directory_json {
            let working_directory: PathBuf =
                serde_json::from_slice(&json).map_err(serialization)?;
            actor = actor.scoped_to_working_directory(working_directory);
        }
        Ok(actor)
    }
}

impl std::fmt::Debug for CapabilityHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapabilityHandler")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl CapabilityHandler {
    pub fn open(path: impl AsRef<Path>, policy: CapabilityPolicy) -> CapabilityResult<Self> {
        let mut connection = Connection::open(path.as_ref()).map_err(persistence)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(persistence)?;
        super::schema::initialize(&mut connection, now_millis()).map_err(persistence)?;
        Ok(Self {
            connection: Mutex::new(connection),
            policy,
        })
    }

    pub fn register_actor_session(
        &self,
        actor: &AuthenticatedToolContext,
        request_id: &str,
        cycle_id: &str,
        goal_id: &str,
        ttl: Duration,
    ) -> CapabilityResult<ActorSessionLease> {
        validate_identifier("actor registration request id", request_id)?;
        validate_identifier("actor identity", &actor.actor_identity)?;
        validate_identifier("session id", &actor.session_id)?;
        validate_identifier("cycle id", cycle_id)?;
        validate_identifier("goal id", goal_id)?;
        let repository = actor.bound_repository().ok_or_else(|| {
            CapabilityError::new(
                CapabilityErrorCode::PermissionDenied,
                "actor session must be bound to one repository",
            )
        })?;
        match (actor.bound_cycle_id(), actor.bound_goal_id()) {
            (Some(bound_cycle), Some(bound_goal))
                if bound_cycle == cycle_id && bound_goal == goal_id => {}
            (None, None) => {}
            _ => {
                return Err(CapabilityError::new(
                    CapabilityErrorCode::AuthorizationScopeViolation,
                    "actor registration target does not match its existing cycle and goal binding",
                ));
            }
        }
        self.validate_governed_repository(repository)?;
        let ttl_millis = i64::try_from(ttl.as_millis()).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "actor session lease is too long",
            )
        })?;
        if ttl_millis <= 0 {
            return Err(CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "actor session lease must be positive",
            ));
        }

        let fingerprint = fingerprint(
            actor,
            &self.policy.revision,
            &("actor_session_v1", cycle_id, goal_id, ttl_millis),
        )?;
        let binding = ActorBinding::new(actor, cycle_id, goal_id, repository)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistence)?;
        if let Some(existing) =
            replay_request(&transaction, request_id, "actor_session", &fingerprint)?
        {
            return Ok(existing);
        }
        transaction
            .execute(
                "DELETE FROM actor_sessions WHERE expires_at < ?1",
                [now_millis()],
            )
            .map_err(persistence)?;
        let existing_binding = load_actor_binding(&transaction, &actor.session_id)?;
        if existing_binding
            .as_ref()
            .is_some_and(|existing| existing != &binding)
        {
            return Err(CapabilityError::new(
                CapabilityErrorCode::AuthorizationScopeViolation,
                "actor session is already bound to a different identity or authorization scope",
            ));
        }
        let token = Uuid::new_v4().simple().to_string();
        let expires_at_unix_millis = now_millis().saturating_add(ttl_millis);
        let token_hash = sha256_hex(token.as_bytes());
        transaction
            .execute(
                "INSERT INTO actor_sessions(
                    session_id, cycle_id, goal_id, actor_identity, repository_json,
                    grants_json, engineer_permissions_json, working_directory_json,
                    observe_only, token_hash, expires_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(session_id) DO UPDATE SET
                    cycle_id=excluded.cycle_id,
                    goal_id=excluded.goal_id,
                    actor_identity=excluded.actor_identity,
                    repository_json=excluded.repository_json,
                    grants_json=excluded.grants_json,
                    engineer_permissions_json=excluded.engineer_permissions_json,
                    working_directory_json=excluded.working_directory_json,
                    observe_only=excluded.observe_only,
                    token_hash=excluded.token_hash,
                    expires_at=excluded.expires_at",
                params![
                    actor.session_id,
                    binding.cycle_id,
                    binding.goal_id,
                    binding.actor_identity,
                    binding.repository_json,
                    binding.grants_json,
                    binding.engineer_permissions_json,
                    binding.working_directory_json,
                    binding.observe_only,
                    token_hash,
                    expires_at_unix_millis
                ],
            )
            .map_err(persistence)?;
        let lease = ActorSessionLease {
            token,
            expires_at_unix_millis,
        };
        record_request(
            &transaction,
            request_id,
            "actor_session",
            &fingerprint,
            &lease,
        )?;
        transaction.commit().map_err(persistence)?;
        Ok(lease)
    }

    pub fn authenticate_actor_session(
        &self,
        token: &str,
        session_id: &str,
        cycle_id: &str,
        goal_id: &str,
    ) -> CapabilityResult<AuthenticatedToolContext> {
        validate_identifier("session id", session_id)?;
        validate_identifier("cycle id", cycle_id)?;
        validate_identifier("goal id", goal_id)?;
        let connection = self.lock()?;
        let Some(stored) = load_actor_session(&connection, session_id)? else {
            return Err(CapabilityError::new(
                CapabilityErrorCode::Unauthenticated,
                "actor session lease was not found",
            ));
        };
        if stored.binding.cycle_id != cycle_id
            || stored.binding.goal_id != goal_id
            || stored.expires_at < now_millis()
            || stored.token_hash != sha256_hex(token.as_bytes())
        {
            return Err(CapabilityError::new(
                CapabilityErrorCode::Unauthenticated,
                "actor session lease is expired or does not match the invocation",
            ));
        }
        stored.binding.into_context(session_id)
    }

    pub fn issue_privileged_approval(
        &self,
        authority: &ApprovalAuthority,
        request_id: &str,
        effect_id: &str,
    ) -> CapabilityResult<PrivilegedApproval> {
        validate_identifier("approval request id", request_id)?;
        validate_identifier("effect id", effect_id)?;
        let fingerprint = fingerprint(
            &AuthenticatedToolContext::new(
                &authority.principal,
                "privileged-approval",
                std::iter::empty(),
            ),
            &self.policy.revision,
            &("privileged_approval_v1", effect_id),
        )?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistence)?;
        if let Some(existing) = replay_request(
            &transaction,
            request_id,
            "privileged_approval",
            &fingerprint,
        )? {
            return Ok(existing);
        }
        let job = query_effect_by_id(&transaction, effect_id)?.ok_or_else(|| {
            CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "effect job was not found",
            )
        })?;
        if !matches!(job.state.as_str(), "pending" | "blocked") {
            return Err(CapabilityError::new(
                CapabilityErrorCode::StateTransitionRejected,
                "privileged approval can be issued only for a pending or blocked effect",
            ));
        }
        if !matches!(
            job.action,
            Action::RequestMerge(_) | Action::RequestDeploy(_)
        ) {
            return Err(CapabilityError::new(
                CapabilityErrorCode::PermissionDenied,
                "only merge and deploy effects require privileged approval",
            ));
        }
        let outcome = terminal_for_outcome_id(&transaction, &job.outcome_id)?;
        let policy_revision = match &outcome.payload {
            TypedOutcomePayload::Action(payload) => payload.admission.policy_revision.clone(),
            _ => {
                return Err(CapabilityError::new(
                    CapabilityErrorCode::StateTransitionRejected,
                    "privileged effect is not linked to an action outcome",
                ));
            }
        };
        let mut approval = PrivilegedApproval {
            approval_id: Uuid::now_v7().to_string(),
            principal: authority.principal.clone(),
            effect_id: job.effect_id,
            outcome_id: outcome.outcome_id,
            session_id: outcome.session_id,
            cycle_id: outcome.cycle_id,
            goal_id: outcome.goal_id,
            action_kind: job.action.kind(),
            canonical_payload_hash: action_payload_hash(&job.action)?,
            repository: outcome.repository.clone(),
            policy_revision,
            issued_at_unix_millis: now_millis(),
            signature: String::new(),
        };
        approval.signature = approval_signature(&authority.signing_key, &approval)?;
        let json = serde_json::to_vec(&approval).map_err(serialization)?;
        transaction
            .execute(
                "INSERT INTO authorization_decisions(
                    decision_id, effect_id, decision, decision_json, recorded_at
                 ) VALUES (?1, ?2, 'approved', ?3, ?4)",
                params![
                    approval.approval_id,
                    approval.effect_id,
                    json,
                    approval.issued_at_unix_millis
                ],
            )
            .map_err(persistence)?;
        transaction
            .execute(
                "UPDATE effect_jobs
                 SET state='pending', error=NULL, lease_owner=NULL, lease_expires_at=NULL
                 WHERE effect_id=?1 AND state='blocked'",
                [&approval.effect_id],
            )
            .map_err(persistence)?;
        record_request(
            &transaction,
            request_id,
            "privileged_approval",
            &fingerprint,
            &approval,
        )?;
        transaction.commit().map_err(persistence)?;
        Ok(approval)
    }

    pub fn record_action(
        &self,
        actor: &AuthenticatedToolContext,
        request: RecordActionRequest,
        admission: &AdmissionSnapshot,
    ) -> CapabilityResult<TerminalOutcome> {
        self.validate_actor_target(actor, &request.identity)?;
        let grant = CapabilityGrant::RecordAction(request.action.kind());
        if actor.is_observe_only()
            || crate::read_only_guard::observe_only_enabled()
            || !actor.allows(grant)
            || !self.policy.allows(grant)
        {
            return self.record_action_denied(actor, request, grant);
        }
        self.authorize(actor, &request.identity, grant)?;
        if let Err(error) = self.authorize_action_scope(actor, &request.action) {
            if error.code() == CapabilityErrorCode::PermissionDenied {
                return self.record_action_denied(actor, request, grant);
            }
            return Err(error);
        }
        self.authorize_engineer_scope(actor, &request.action)?;
        self.validate_common(&request.raw_semantic, &request.evidence)?;
        if let Err(error) = self.validate_action(&request.action) {
            if error.code() == CapabilityErrorCode::PermissionDenied {
                return self.record_action_denied(actor, request, grant);
            }
            return Err(error);
        }

        let fingerprint = fingerprint(actor, &self.policy.revision, &request)?;

        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistence)?;
        if let Some(existing) = replay_request(
            &transaction,
            &request.identity.request_id,
            "terminal",
            &fingerprint,
        )? {
            return Ok(existing);
        }
        if let Action::SpawnEngineer(spawn) = &request.action {
            let expected_claim = format!(
                "{}/{}:{}",
                spawn.repository.owner, spawn.repository.name, request.identity.goal_id
            );
            if spawn.claim_key != expected_claim {
                return Err(CapabilityError::new(
                    CapabilityErrorCode::InvalidArgument,
                    format!("engineer claim key must be {expected_claim:?}"),
                ));
            }
        }
        self.admit(&request.action, admission)?;
        ensure_cycle_open(&transaction, &request.identity)?;
        let RecordActionRequest {
            identity,
            action,
            raw_semantic,
            evidence,
        } = request;
        let payload = TypedOutcomePayload::Action(ActionOutcomePayload {
            action,
            admission: AdmissionDecision {
                policy_revision: admission.policy_revision.clone(),
            },
        });
        let outcome = self.new_outcome(actor, &identity, payload, raw_semantic, evidence);
        let outcome_json = serde_json::to_vec(&outcome).map_err(serialization)?;
        insert_terminal(&transaction, &outcome, &fingerprint, &outcome_json)?;
        record_request_json(
            &transaction,
            &outcome.request_id,
            "terminal",
            &fingerprint,
            &outcome_json,
        )?;
        let TypedOutcomePayload::Action(action_payload) = &outcome.payload else {
            unreachable!("record_action always creates an action payload");
        };
        if let Action::SpawnEngineer(spawn) = &action_payload.action {
            transaction
                .execute(
                    "INSERT INTO engineer_claims(claim_key, outcome_id, request_id) VALUES (?1, ?2, ?3)",
                    params![spawn.claim_key, outcome.outcome_id, outcome.request_id],
                )
                .map_err(|error| {
                    if is_constraint(&error) {
                        CapabilityError::new(
                            CapabilityErrorCode::AdmissionRejected,
                            format!("engineer claim is already active: {}", spawn.claim_key),
                        )
                    } else {
                        persistence(error)
                    }
                })?;
        }
        insert_effect(&transaction, &outcome, &action_payload.action)?;
        transaction.commit().map_err(persistence)?;
        Ok(outcome)
    }

    pub fn record_no_action(
        &self,
        actor: &AuthenticatedToolContext,
        request: RecordNoActionRequest,
    ) -> CapabilityResult<TerminalOutcome> {
        self.authorize(actor, &request.identity, CapabilityGrant::RecordNoAction)?;
        self.validate_common(&request.raw_semantic, &request.evidence)?;
        self.validate_opaque("reason", &request.reason, true)?;
        let fingerprint = fingerprint(actor, &self.policy.revision, &request)?;
        self.commit_terminal(
            actor,
            &request.identity,
            TypedOutcomePayload::NoAction(NoActionOutcomePayload {
                reason: request.reason,
            }),
            request.raw_semantic,
            request.evidence,
            fingerprint,
        )
    }

    pub fn record_blocked(
        &self,
        actor: &AuthenticatedToolContext,
        request: RecordBlockedRequest,
    ) -> CapabilityResult<TerminalOutcome> {
        self.authorize(actor, &request.identity, CapabilityGrant::RecordBlocked)?;
        self.validate_common(&request.raw_semantic, &request.evidence)?;
        self.validate_opaque("reason", &request.reason, true)?;
        let fingerprint = fingerprint(actor, &self.policy.revision, &request)?;
        self.commit_terminal(
            actor,
            &request.identity,
            TypedOutcomePayload::Blocked(BlockedOutcomePayload {
                reason: request.reason,
                blocker: request.blocker,
                retry: request.retry,
            }),
            request.raw_semantic,
            request.evidence,
            fingerprint,
        )
    }

    pub fn record_completed(
        &self,
        actor: &AuthenticatedToolContext,
        request: RecordCompletedRequest,
    ) -> CapabilityResult<TerminalOutcome> {
        self.authorize(actor, &request.identity, CapabilityGrant::RecordCompleted)?;
        self.validate_common(&request.raw_semantic, &request.evidence)?;
        self.validate_opaque("completion summary", &request.summary, true)?;
        validate_identifier("completion criterion", &request.completion.criterion_id)?;
        validate_evidence(&request.completion.verification_evidence)?;
        if request.completion.verification_evidence.is_empty() {
            return Err(CapabilityError::new(
                CapabilityErrorCode::StateTransitionRejected,
                "completion requires typed verification evidence",
            ));
        }
        if request
            .completion
            .verification_evidence
            .iter()
            .any(|required| !request.evidence.contains(required))
        {
            return Err(CapabilityError::new(
                CapabilityErrorCode::StateTransitionRejected,
                "completion verification references must also be attached as outcome evidence",
            ));
        }
        let fingerprint = fingerprint(actor, &self.policy.revision, &request)?;
        self.commit_terminal(
            actor,
            &request.identity,
            TypedOutcomePayload::Completed(CompletedOutcomePayload {
                summary: request.summary,
                completion: request.completion,
            }),
            request.raw_semantic,
            request.evidence,
            fingerprint,
        )
    }

    pub fn record_progress(
        &self,
        actor: &AuthenticatedToolContext,
        request: RecordProgressRequest,
    ) -> CapabilityResult<ProgressRecord> {
        self.authorize(actor, &request.identity, CapabilityGrant::RecordProgress)?;
        self.validate_opaque("progress summary", &request.summary, true)?;
        validate_evidence(&request.evidence)?;
        if request.percent > 100 {
            return Err(CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "progress percent must be in 0..=100",
            ));
        }
        let fingerprint = fingerprint(actor, &self.policy.revision, &request)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistence)?;
        if let Some(existing) = replay_request(
            &transaction,
            &request.identity.request_id,
            "progress",
            &fingerprint,
        )? {
            return Ok(existing);
        }
        ensure_cycle_open(&transaction, &request.identity)?;
        let record = ProgressRecord {
            progress_id: Uuid::now_v7().to_string(),
            request_id: request.identity.request_id,
            session_id: request.identity.session_id,
            actor_identity: actor.actor_identity.clone(),
            goal_id: request.identity.goal_id,
            cycle_id: request.identity.cycle_id,
            percent: request.percent,
            summary: request.summary,
            evidence: request.evidence,
            recorded_at_unix_millis: now_millis(),
        };
        let json = serde_json::to_vec(&record).map_err(serialization)?;
        transaction
            .execute(
                "INSERT INTO progress_records(request_id, request_hash, session_id, cycle_id, progress_json) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    record.request_id,
                    fingerprint,
                    record.session_id,
                    record.cycle_id,
                    &json
                ],
            )
            .map_err(persistence)?;
        record_request_json(
            &transaction,
            &record.request_id,
            "progress",
            &fingerprint,
            &json,
        )?;
        transaction.commit().map_err(persistence)?;
        Ok(record)
    }

    pub fn execute_process(
        &self,
        actor: &AuthenticatedToolContext,
        request: ProcessExecRequest,
    ) -> CapabilityResult<ProcessExecutionRecord> {
        let (mut record, should_execute) = self.reserve_process_execution(actor, &request)?;
        if !should_execute {
            return Ok(record);
        }
        record.status = ProcessExecutionStatus::Running;
        self.update_process_execution(&record, ProcessExecutionStatus::Reserved)?;

        let output = Command::new(&request.program)
            .args(&request.args)
            .current_dir(&request.working_directory)
            .output();
        match output {
            Ok(output) => {
                record.status = if output.status.success() {
                    ProcessExecutionStatus::Completed
                } else {
                    ProcessExecutionStatus::Failed
                };
                record.exit_code = output.status.code();
                record.stdout = output.stdout;
                record.stderr = output.stderr;
            }
            Err(error) => {
                record.status = ProcessExecutionStatus::Failed;
                record.stderr = error.to_string().into_bytes();
            }
        }
        self.update_process_execution(&record, ProcessExecutionStatus::Running)?;
        Ok(record)
    }

    fn reserve_process_execution(
        &self,
        actor: &AuthenticatedToolContext,
        request: &ProcessExecRequest,
    ) -> CapabilityResult<(ProcessExecutionRecord, bool)> {
        self.validate_process_execution(actor, request)?;
        let fingerprint = fingerprint(actor, &self.policy.revision, &("process_exec_v1", request))?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistence)?;
        if let Some(existing) = replay_request(
            &transaction,
            &request.identity.request_id,
            "process_exec",
            &fingerprint,
        )? {
            return Ok((existing, false));
        }
        reserve_process_slot(
            &transaction,
            &request.identity,
            self.policy.process_exec_mutations_per_cycle,
        )?;
        let record = new_process_execution(request);
        let result_json = serde_json::to_vec(&record).map_err(serialization)?;
        insert_process_execution(&transaction, request, &record, &fingerprint, &result_json)?;
        record_request_json(
            &transaction,
            &record.request_id,
            "process_exec",
            &fingerprint,
            &result_json,
        )?;
        transaction.commit().map_err(persistence)?;
        Ok((record, true))
    }

    fn validate_process_execution(
        &self,
        actor: &AuthenticatedToolContext,
        request: &ProcessExecRequest,
    ) -> CapabilityResult<()> {
        self.authorize(actor, &request.identity, CapabilityGrant::ProcessExec)?;
        if actor.is_observe_only()
            || !actor.engineer_permissions().contains("process_exec")
            || !self
                .policy
                .allowed_engineer_permissions
                .contains("process_exec")
        {
            return Err(CapabilityError::new(
                CapabilityErrorCode::AuthorizationScopeViolation,
                "process_exec is outside the actor, action, or base-type scope",
            ));
        }
        if !request.program.is_absolute() || !request.program.is_file() {
            return Err(CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "process executable must be an existing absolute file",
            ));
        }
        if actor.bound_working_directory() != Some(request.working_directory.as_path())
            || !request.working_directory.is_dir()
        {
            return Err(CapabilityError::new(
                CapabilityErrorCode::AuthorizationScopeViolation,
                "process working directory does not match authenticated action scope",
            ));
        }
        if request.args.len() > 256
            || request
                .args
                .iter()
                .any(|argument| argument.as_bytes().contains(&0))
        {
            return Err(CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "process arguments exceed the typed execution boundary",
            ));
        }
        Ok(())
    }

    fn update_process_execution(
        &self,
        record: &ProcessExecutionRecord,
        expected: ProcessExecutionStatus,
    ) -> CapabilityResult<()> {
        let result_json = serde_json::to_vec(record).map_err(serialization)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistence)?;
        let changed = transaction
            .execute(
                "UPDATE process_executions SET status=?2, result_json=?3
                 WHERE execution_id=?1 AND status=?4",
                params![
                    record.execution_id,
                    record.status.as_str(),
                    &result_json,
                    expected.as_str()
                ],
            )
            .map_err(persistence)?;
        if changed != 1 {
            return Err(CapabilityError::new(
                CapabilityErrorCode::IndeterminateExecution,
                "process execution state changed concurrently",
            ));
        }
        transaction
            .execute(
                "UPDATE mutation_requests SET result_json=?2 WHERE request_id=?1",
                params![record.request_id, result_json],
            )
            .map_err(persistence)?;
        transaction.commit().map_err(persistence)
    }

    pub fn terminal_for_cycle(
        &self,
        session_id: &str,
        cycle_id: &str,
    ) -> CapabilityResult<Option<TerminalOutcome>> {
        let connection = self.lock()?;
        let json: Option<Vec<u8>> = connection
            .query_row(
                "SELECT outcome_json FROM terminal_outcomes WHERE session_id=?1 AND cycle_id=?2",
                params![session_id, cycle_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(persistence)?;
        json.map(|value| serde_json::from_slice(&value).map_err(serialization))
            .transpose()
    }

    pub fn terminal_for_request(
        &self,
        request_id: &str,
    ) -> CapabilityResult<Option<TerminalOutcome>> {
        validate_identifier("request id", request_id)?;
        let connection = self.lock()?;
        let json: Option<Vec<u8>> = connection
            .query_row(
                "SELECT outcome_json FROM terminal_outcomes WHERE request_id=?1",
                [request_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(persistence)?;
        json.map(|value| serde_json::from_slice(&value).map_err(serialization))
            .transpose()
    }

    pub fn list_terminals(&self, limit: usize) -> CapabilityResult<Vec<TerminalOutcome>> {
        if limit == 0 || limit > 10_000 {
            return Err(CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "terminal list limit must be in 1..=10000",
            ));
        }
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT outcome_json FROM terminal_outcomes ORDER BY rowid DESC LIMIT ?1")
            .map_err(persistence)?;
        let rows = statement
            .query_map([limit], |row| row.get::<_, Vec<u8>>(0))
            .map_err(persistence)?;
        rows.map(|row| {
            let json = row.map_err(persistence)?;
            serde_json::from_slice(&json).map_err(serialization)
        })
        .collect()
    }

    pub fn terminal_count(&self, session_id: &str, cycle_id: &str) -> CapabilityResult<usize> {
        let connection = self.lock()?;
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM terminal_outcomes WHERE session_id=?1 AND cycle_id=?2)",
                params![session_id, cycle_id],
                |row| row.get(0),
            )
            .map_err(persistence)?;
        Ok(usize::from(exists))
    }

    pub fn progress_for_cycle(
        &self,
        session_id: &str,
        cycle_id: &str,
    ) -> CapabilityResult<Vec<ProgressRecord>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT progress_json FROM progress_records WHERE session_id=?1 AND cycle_id=?2 ORDER BY rowid",
            )
            .map_err(persistence)?;
        let rows = statement
            .query_map(params![session_id, cycle_id], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .map_err(persistence)?;
        rows.map(|row| {
            let json = row.map_err(persistence)?;
            serde_json::from_slice(&json).map_err(serialization)
        })
        .collect()
    }

    pub fn effect_for_outcome(&self, outcome_id: &str) -> CapabilityResult<Option<EffectJob>> {
        let connection = self.lock()?;
        query_effect(
            &connection,
            "
            SELECT effect_id, outcome_id, request_id, kind, state, action_json, attempt,
                   lease_generation, lease_owner, lease_expires_at, error, result_json,
                   (SELECT decision_json FROM authorization_decisions
                    WHERE effect_id=effect_jobs.effect_id AND decision='approved'
                    ORDER BY recorded_at DESC, rowid DESC LIMIT 1)
            FROM effect_jobs WHERE outcome_id=?1
            ",
            [outcome_id],
        )
    }

    pub fn claim_next_effect(
        &self,
        worker: &str,
        request_id: &str,
        now: SystemTime,
        lease: Duration,
    ) -> CapabilityResult<Option<EffectJob>> {
        validate_identifier("effect worker", worker)?;
        validate_identifier("effect claim request id", request_id)?;
        let now = system_time_millis(now)?;
        let lease_millis = i64::try_from(lease.as_millis()).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "effect lease is too long",
            )
        })?;
        let fingerprint = fingerprint(
            &AuthenticatedToolContext::new("effect-claim", worker, std::iter::empty()),
            &self.policy.revision,
            &("claim_next_effect_v1", worker, lease_millis),
        )?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistence)?;
        if let Some(existing) =
            replay_request(&transaction, request_id, "effect_claim", &fingerprint)?
        {
            return Ok(existing);
        }
        let claimed = query_effect(
            &transaction,
            "
            UPDATE effect_jobs
            SET state='running', attempt=attempt+1, lease_generation=lease_generation+1,
                lease_owner=?1, lease_expires_at=?2
            WHERE effect_id = (
                SELECT effect_id FROM effect_jobs
                WHERE state='pending'
                ORDER BY rowid
                LIMIT 1
            )
            AND state='pending'
            RETURNING effect_id, outcome_id, request_id, kind, state, action_json, attempt,
                      lease_generation, lease_owner, lease_expires_at, error, result_json,
                      (SELECT decision_json FROM authorization_decisions
                       WHERE effect_id=effect_jobs.effect_id AND decision='approved'
                       ORDER BY recorded_at DESC, rowid DESC LIMIT 1)
            ",
            params![worker, now.saturating_add(lease_millis)],
        )?;
        record_request(
            &transaction,
            request_id,
            "effect_claim",
            &fingerprint,
            &claimed,
        )?;
        transaction.commit().map_err(persistence)?;
        Ok(claimed)
    }

    pub(crate) fn claim_effect_for_outcome(
        &self,
        outcome_id: &str,
        worker: &str,
        request_id: &str,
        now: SystemTime,
        lease: Duration,
    ) -> CapabilityResult<Option<EffectJob>> {
        validate_identifier("effect worker", worker)?;
        validate_identifier("outcome id", outcome_id)?;
        validate_identifier("effect claim request id", request_id)?;
        let now = system_time_millis(now)?;
        let lease_millis = i64::try_from(lease.as_millis()).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "effect lease is too long",
            )
        })?;
        let fingerprint = fingerprint(
            &AuthenticatedToolContext::new("effect-claim", worker, std::iter::empty()),
            &self.policy.revision,
            &(
                "claim_effect_for_outcome_v1",
                outcome_id,
                worker,
                lease_millis,
            ),
        )?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistence)?;
        if let Some(existing) =
            replay_request(&transaction, request_id, "effect_claim", &fingerprint)?
        {
            return Ok(existing);
        }
        let claimed = query_effect(
            &transaction,
            "
            UPDATE effect_jobs
            SET state='running', attempt=attempt+1, lease_generation=lease_generation+1,
                lease_owner=?2, lease_expires_at=?3
            WHERE outcome_id=?1 AND state='pending'
            RETURNING effect_id, outcome_id, request_id, kind, state, action_json, attempt,
                      lease_generation, lease_owner, lease_expires_at, error, result_json,
                      (SELECT decision_json FROM authorization_decisions
                       WHERE effect_id=effect_jobs.effect_id AND decision='approved'
                       ORDER BY recorded_at DESC, rowid DESC LIMIT 1)
            ",
            params![outcome_id, worker, now.saturating_add(lease_millis)],
        )?;
        record_request(
            &transaction,
            request_id,
            "effect_claim",
            &fingerprint,
            &claimed,
        )?;
        transaction.commit().map_err(persistence)?;
        Ok(claimed)
    }

    pub fn recover_expired_effects(
        &self,
        request_id: &str,
        now: SystemTime,
    ) -> CapabilityResult<usize> {
        validate_identifier("effect recovery request id", request_id)?;
        let now = system_time_millis(now)?;
        let actor = AuthenticatedToolContext::new("effect-recovery", "system", std::iter::empty());
        let fingerprint = fingerprint(&actor, &self.policy.revision, &("recover_effects_v1", now))?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistence)?;
        if let Some(existing) =
            replay_request(&transaction, request_id, "effect_recovery", &fingerprint)?
        {
            return Ok(existing);
        }
        let recovered = transaction
            .execute(
                "UPDATE effect_jobs
                 SET state='indeterminate', error='effect lease expired; execution outcome is unknown'
                 WHERE state='running' AND lease_expires_at <= ?1",
                [now],
            )
            .map_err(persistence)?;
        record_request(
            &transaction,
            request_id,
            "effect_recovery",
            &fingerprint,
            &recovered,
        )?;
        transaction.commit().map_err(persistence)?;
        Ok(recovered)
    }

    pub fn renew_effect(
        &self,
        lease: &EffectJob,
        request_id: &str,
        now: SystemTime,
        extension: Duration,
    ) -> CapabilityResult<()> {
        validate_identifier("effect mutation request id", request_id)?;
        let owner = effect_lease_owner(lease)?;
        let now = system_time_millis(now)?;
        let extension = i64::try_from(extension.as_millis()).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "effect lease extension is too long",
            )
        })?;
        if extension <= 0 {
            return Err(CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "effect lease extension must be positive",
            ));
        }
        let expires_at = now.saturating_add(extension);
        let fingerprint = fingerprint(
            &system_actor(lease),
            &self.policy.revision,
            &(
                "effect_renew_v1",
                lease.effect_id.as_str(),
                owner,
                lease.lease_generation,
                extension,
            ),
        )?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistence)?;
        if replay_request::<serde_json::Value>(
            &transaction,
            request_id,
            "effect_renew",
            &fingerprint,
        )?
        .is_some()
        {
            return Ok(());
        }
        let changed = transaction
            .execute(
                "UPDATE effect_jobs SET lease_expires_at=?5
                 WHERE effect_id=?1 AND state='running' AND lease_owner=?2
                   AND lease_generation=?3 AND lease_expires_at>?4",
                params![
                    lease.effect_id,
                    owner,
                    lease.lease_generation,
                    now,
                    expires_at
                ],
            )
            .map_err(persistence)?;
        if changed != 1 {
            return Err(stale_lease(&lease.effect_id));
        }
        record_request(
            &transaction,
            request_id,
            "effect_renew",
            &fingerprint,
            &serde_json::Value::Null,
        )?;
        transaction.commit().map_err(persistence)
    }

    pub(crate) fn block_effect_authorization(
        &self,
        job: &EffectJob,
        request_id: &str,
        reason: &str,
    ) -> CapabilityResult<()> {
        validate_identifier("effect authorization request id", request_id)?;
        let fingerprint = fingerprint(
            &system_actor(job),
            &self.policy.revision,
            &(
                "effect_authorization_block_v1",
                job.effect_id.as_str(),
                job.lease_owner.as_deref(),
                job.lease_generation,
                reason,
            ),
        )?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistence)?;
        if replay_request::<serde_json::Value>(
            &transaction,
            request_id,
            "effect_authorization_block",
            &fingerprint,
        )?
        .is_some()
        {
            return Ok(());
        }
        let recorded_at = now_millis();
        let decision_id = Uuid::now_v7().to_string();
        let decision = serde_json::to_vec(&serde_json::json!({
            "decision_id": decision_id,
            "effect_id": job.effect_id,
            "outcome_id": job.outcome_id,
            "request_id": job.request_id,
            "decision": "blocked",
            "reason": reason,
            "recorded_at_unix_millis": recorded_at,
        }))
        .map_err(serialization)?;
        transaction
            .execute(
                "INSERT INTO authorization_decisions(
                    decision_id, effect_id, decision, decision_json, recorded_at
                 ) VALUES (?1, ?2, 'blocked', ?3, ?4)",
                params![decision_id, job.effect_id, decision, recorded_at],
            )
            .map_err(persistence)?;
        let changed = if job.state.as_str() == "running" {
            let owner = effect_lease_owner(job)?;
            transaction
                .execute(
                    "UPDATE effect_jobs
                     SET state='blocked', error=?2, lease_owner=NULL, lease_expires_at=NULL
                     WHERE effect_id=?1 AND state='running' AND lease_owner=?3
                       AND lease_generation=?4 AND lease_expires_at>?5",
                    params![
                        job.effect_id,
                        reason,
                        owner,
                        job.lease_generation,
                        now_millis()
                    ],
                )
                .map_err(persistence)?
        } else {
            transaction
                .execute(
                    "UPDATE effect_jobs
                     SET state='blocked', error=?2, lease_owner=NULL, lease_expires_at=NULL
                     WHERE effect_id=?1 AND state='pending'",
                    params![job.effect_id, reason],
                )
                .map_err(persistence)?
        };
        if changed != 1 {
            return Err(stale_lease(&job.effect_id));
        }
        record_request(
            &transaction,
            request_id,
            "effect_authorization_block",
            &fingerprint,
            &serde_json::Value::Null,
        )?;
        transaction.commit().map_err(persistence)
    }

    pub fn release_effect_for_retry(
        &self,
        lease: &EffectJob,
        request_id: &str,
        now: SystemTime,
        error: &str,
    ) -> CapabilityResult<()> {
        validate_identifier("effect mutation request id", request_id)?;
        let owner = effect_lease_owner(lease)?;
        let now = system_time_millis(now)?;
        let fingerprint = fingerprint(
            &system_actor(lease),
            &self.policy.revision,
            &(
                "effect_retry_v1",
                lease.effect_id.as_str(),
                owner,
                lease.lease_generation,
                error,
            ),
        )?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistence)?;
        if replay_request::<serde_json::Value>(
            &transaction,
            request_id,
            "effect_retry",
            &fingerprint,
        )?
        .is_some()
        {
            return Ok(());
        }
        let changed = transaction
            .execute(
                "UPDATE effect_jobs
                 SET state='pending', error=?2, lease_owner=NULL, lease_expires_at=NULL
                 WHERE effect_id=?1 AND state='running' AND lease_owner=?3
                   AND lease_generation=?4 AND lease_expires_at>?5",
                params![lease.effect_id, error, owner, lease.lease_generation, now],
            )
            .map_err(persistence)?;
        if changed != 1 {
            return Err(stale_lease(&lease.effect_id));
        }
        record_request(
            &transaction,
            request_id,
            "effect_retry",
            &fingerprint,
            &serde_json::Value::Null,
        )?;
        transaction.commit().map_err(persistence)
    }

    pub fn finish_effect(
        &self,
        lease: &EffectJob,
        request_id: &str,
        now: SystemTime,
        result: &EffectResult,
    ) -> CapabilityResult<()> {
        validate_identifier("effect mutation request id", request_id)?;
        let owner = effect_lease_owner(lease)?;
        let now = system_time_millis(now)?;
        let fingerprint = fingerprint(
            &system_actor(lease),
            &self.policy.revision,
            &(
                "effect_finish_v1",
                lease.effect_id.as_str(),
                owner,
                lease.lease_generation,
                result,
            ),
        )?;
        let (state, error) = match result {
            EffectResult::Succeeded { .. } => ("succeeded", None),
            EffectResult::Failed { error } => ("failed", Some(error.as_str())),
        };
        let result_json = serde_json::to_vec(result).map_err(serialization)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistence)?;
        if replay_request::<EffectResult>(&transaction, request_id, "effect_finish", &fingerprint)?
            .is_some()
        {
            return Ok(());
        }
        let changed = transaction
            .execute(
                "UPDATE effect_jobs
                 SET state=?2, error=?3, result_json=?4, lease_owner=NULL, lease_expires_at=NULL
                 WHERE effect_id=?1 AND state='running' AND lease_owner=?5
                   AND lease_generation=?6 AND lease_expires_at>?7",
                params![
                    lease.effect_id,
                    state,
                    error,
                    result_json,
                    owner,
                    lease.lease_generation,
                    now
                ],
            )
            .map_err(persistence)?;
        if changed != 1 {
            return Err(stale_lease(&lease.effect_id));
        }
        record_request(
            &transaction,
            request_id,
            "effect_finish",
            &fingerprint,
            result,
        )?;
        transaction.commit().map_err(persistence)
    }

    fn commit_terminal(
        &self,
        actor: &AuthenticatedToolContext,
        identity: &TerminalRequestIdentity,
        payload: TypedOutcomePayload,
        raw_semantic: OpaqueBytes,
        evidence: Vec<EvidenceRef>,
        fingerprint: String,
    ) -> CapabilityResult<TerminalOutcome> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistence)?;
        if let Some(existing) =
            replay_request(&transaction, &identity.request_id, "terminal", &fingerprint)?
        {
            return Ok(existing);
        }
        ensure_cycle_open(&transaction, identity)?;
        let outcome = self.new_outcome(actor, identity, payload, raw_semantic, evidence);
        let outcome_json = serde_json::to_vec(&outcome).map_err(serialization)?;
        insert_terminal(&transaction, &outcome, &fingerprint, &outcome_json)?;
        record_request_json(
            &transaction,
            &outcome.request_id,
            "terminal",
            &fingerprint,
            &outcome_json,
        )?;
        transaction.commit().map_err(persistence)?;
        Ok(outcome)
    }

    fn record_action_denied(
        &self,
        actor: &AuthenticatedToolContext,
        request: RecordActionRequest,
        grant: CapabilityGrant,
    ) -> CapabilityResult<TerminalOutcome> {
        self.validate_common(&request.raw_semantic, &request.evidence)?;
        let fingerprint = fingerprint(
            actor,
            &self.policy.revision,
            &(
                "authorization_blocked",
                grant,
                &request,
                crate::read_only_guard::observe_only_enabled(),
            ),
        )?;
        let reason = if actor.is_observe_only() || crate::read_only_guard::observe_only_enabled() {
            "SIMARD_OBSERVE_ONLY denied the requested mutation"
        } else {
            "the authenticated actor is not granted the requested mutation"
        };
        self.commit_terminal(
            actor,
            &request.identity,
            TypedOutcomePayload::Blocked(BlockedOutcomePayload {
                reason: OpaqueBytes::from(reason.as_bytes().to_vec()),
                blocker: BlockerRef::Authorization {
                    capability: format!("{grant:?}"),
                },
                retry: RetryPolicy::AfterSignal {
                    provider: "simard-capability-policy".to_string(),
                    signal_id: self.policy.revision.clone(),
                },
            }),
            request.raw_semantic,
            request.evidence,
            fingerprint,
        )
    }

    fn new_outcome(
        &self,
        actor: &AuthenticatedToolContext,
        identity: &TerminalRequestIdentity,
        payload: TypedOutcomePayload,
        raw_semantic: OpaqueBytes,
        evidence: Vec<EvidenceRef>,
    ) -> TerminalOutcome {
        let kind = payload.kind();
        TerminalOutcome {
            outcome_id: Uuid::now_v7().to_string(),
            request_id: identity.request_id.clone(),
            session_id: identity.session_id.clone(),
            actor_identity: actor.actor_identity.clone(),
            repository: actor.bound_repository().cloned(),
            goal_id: identity.goal_id.clone(),
            cycle_id: identity.cycle_id.clone(),
            kind,
            payload,
            raw_semantic,
            evidence,
            recorded_at_unix_millis: now_millis(),
        }
    }

    fn authorize(
        &self,
        actor: &AuthenticatedToolContext,
        identity: &TerminalRequestIdentity,
        grant: CapabilityGrant,
    ) -> CapabilityResult<()> {
        self.validate_actor_target(actor, identity)?;
        if !actor.allows(grant) || !self.policy.allows(grant) {
            return Err(CapabilityError::new(
                CapabilityErrorCode::PermissionDenied,
                "actor is not authorized for the requested capability",
            ));
        }
        Ok(())
    }

    fn validate_actor_target(
        &self,
        actor: &AuthenticatedToolContext,
        identity: &TerminalRequestIdentity,
    ) -> CapabilityResult<()> {
        self.validate_identity(identity)?;
        if actor.actor_identity.is_empty() || actor.session_id != identity.session_id {
            return Err(CapabilityError::new(
                CapabilityErrorCode::Unauthenticated,
                "authenticated actor session does not match request session",
            ));
        }
        validate_identifier("actor identity", &actor.actor_identity)?;
        match (actor.bound_cycle_id(), actor.bound_goal_id()) {
            (Some(cycle_id), Some(goal_id))
                if cycle_id == identity.cycle_id && goal_id == identity.goal_id => {}
            _ => {
                return Err(CapabilityError::new(
                    CapabilityErrorCode::AuthorizationScopeViolation,
                    "mutation target does not match the actor's server-bound cycle and goal",
                ));
            }
        }
        Ok(())
    }

    fn authorize_action_scope(
        &self,
        actor: &AuthenticatedToolContext,
        action: &Action,
    ) -> CapabilityResult<()> {
        let Some(repository) = action.repository() else {
            return Ok(());
        };
        let Some(bound) = actor.bound_repository() else {
            return Err(CapabilityError::new(
                CapabilityErrorCode::PermissionDenied,
                "mutating actor is not bound to a goal repository",
            ));
        };
        if repository != bound {
            return Err(CapabilityError::new(
                CapabilityErrorCode::PermissionDenied,
                format!(
                    "requested repository {}/{} does not match authenticated goal repository {}/{}",
                    repository.owner, repository.name, bound.owner, bound.name
                ),
            ));
        }
        Ok(())
    }

    fn authorize_engineer_scope(
        &self,
        actor: &AuthenticatedToolContext,
        action: &Action,
    ) -> CapabilityResult<()> {
        let Action::SpawnEngineer(spawn) = action else {
            return Ok(());
        };
        if spawn.base_type != BaseType::Copilot
            || !spawn
                .requested_permissions
                .is_subset(actor.engineer_permissions())
            || spawn
                .requested_permissions
                .iter()
                .any(|permission| !COPILOT_ENGINEER_PERMISSIONS.contains(&permission.as_str()))
        {
            return Err(CapabilityError::new(
                CapabilityErrorCode::AuthorizationScopeViolation,
                "engineer base_type or requested permissions exceed authenticated actor scope",
            ));
        }
        Ok(())
    }

    fn validate_identity(&self, identity: &TerminalRequestIdentity) -> CapabilityResult<()> {
        validate_identifier("request id", &identity.request_id)?;
        validate_identifier("session id", &identity.session_id)?;
        validate_identifier("cycle id", &identity.cycle_id)?;
        validate_identifier("goal id", &identity.goal_id)
    }

    fn validate_common(
        &self,
        raw_semantic: &OpaqueBytes,
        evidence: &[EvidenceRef],
    ) -> CapabilityResult<()> {
        self.validate_opaque("raw semantic", raw_semantic, false)?;
        validate_evidence(evidence)
    }

    fn validate_opaque(
        &self,
        name: &str,
        value: &OpaqueBytes,
        require_nonempty: bool,
    ) -> CapabilityResult<()> {
        if require_nonempty && value.as_bytes().is_empty() {
            return Err(CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                format!("{name} must not be empty"),
            ));
        }
        if value.as_bytes().len() > self.policy.max_semantic_payload_bytes {
            return Err(CapabilityError::new(
                CapabilityErrorCode::PayloadTooLarge,
                format!(
                    "{name} exceeds {} byte policy limit",
                    self.policy.max_semantic_payload_bytes
                ),
            ));
        }
        Ok(())
    }

    fn validate_action(&self, action: &Action) -> CapabilityResult<()> {
        match action {
            Action::SpawnEngineer(value) => {
                if value.base_type != BaseType::Copilot {
                    return Err(CapabilityError::new(
                        CapabilityErrorCode::AuthorizationScopeViolation,
                        "engineer dispatch requires the canonical copilot base_type",
                    ));
                }
                self.validate_opaque("engineer task", &value.task, true)?;
                self.validate_governed_repository(&value.repository)?;
                validate_identifier("claim key", &value.claim_key)?;
                if value.requested_permissions.is_empty() {
                    return Err(CapabilityError::new(
                        CapabilityErrorCode::InvalidArgument,
                        "spawn engineer requires at least one requested permission",
                    ));
                }
                if !value
                    .requested_permissions
                    .is_subset(&self.policy.allowed_engineer_permissions)
                {
                    return Err(CapabilityError::new(
                        CapabilityErrorCode::PermissionDenied,
                        "engineer requested a permission outside capability policy",
                    ));
                }
            }
            Action::FileIssue(value) => {
                self.validate_governed_repository(&value.repository)?;
                self.validate_opaque("issue title", &value.title, true)?;
                self.validate_opaque("issue body", &value.body, false)?;
                for label in &value.labels {
                    validate_identifier("issue label", label)?;
                }
            }
            Action::RequestMerge(value) => {
                self.validate_governed_repository(&value.pull_request.repository)?;
                if value.pull_request.number == 0 {
                    return Err(CapabilityError::new(
                        CapabilityErrorCode::InvalidArgument,
                        "pull request number must be nonzero",
                    ));
                }
                validate_sha(&value.expected_head_sha)?;
                if value.strategy != "squash"
                    && value.strategy != "merge"
                    && value.strategy != "rebase"
                {
                    return Err(CapabilityError::new(
                        CapabilityErrorCode::InvalidArgument,
                        "unsupported merge strategy",
                    ));
                }
            }
            Action::RequestDeploy(value) => {
                validate_identifier("deployment environment", &value.environment.name)?;
                if !self
                    .policy
                    .allowed_deployment_environments
                    .contains(&value.environment.name)
                {
                    return Err(CapabilityError::new(
                        CapabilityErrorCode::PermissionDenied,
                        "deployment environment is outside capability policy",
                    ));
                }
                validate_sha(&value.artifact.source_commit)?;
                let digest = value.artifact.digest.strip_prefix("sha256:").unwrap_or("");
                if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(CapabilityError::new(
                        CapabilityErrorCode::InvalidArgument,
                        "artifact digest must be sha256 followed by 64 hexadecimal characters",
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_governed_repository(&self, repository: &RepositoryRef) -> CapabilityResult<()> {
        validate_repository(repository)?;
        if !self.policy.allowed_repositories.contains(repository)
            && !self
                .policy
                .allowed_repository_owners
                .contains(&repository.owner)
        {
            return Err(CapabilityError::new(
                CapabilityErrorCode::PermissionDenied,
                "repository is outside capability policy",
            ));
        }
        Ok(())
    }

    fn admit(&self, action: &Action, snapshot: &AdmissionSnapshot) -> CapabilityResult<()> {
        if snapshot.policy_revision != self.policy.revision {
            return Err(CapabilityError::new(
                CapabilityErrorCode::AdmissionRejected,
                "admission policy revision does not match the active capability policy",
            ));
        }
        if snapshot.disk_used_percent > self.policy.max_disk_used_percent {
            return Err(CapabilityError::new(
                CapabilityErrorCode::AdmissionRejected,
                "disk usage exceeds admission ceiling",
            ));
        }
        if let Action::SpawnEngineer(spawn) = action {
            if snapshot.concurrent_engineers >= self.policy.max_concurrent_engineers {
                return Err(CapabilityError::new(
                    CapabilityErrorCode::AdmissionRejected,
                    "engineer concurrency limit reached",
                ));
            }
            if snapshot.active_claims.contains(&spawn.claim_key) {
                return Err(CapabilityError::new(
                    CapabilityErrorCode::AdmissionRejected,
                    "engineer claim conflicts with active work",
                ));
            }
        }
        Ok(())
    }

    fn lock(&self) -> CapabilityResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| persistence_message("outcome ledger lock is poisoned"))
    }
}

fn load_actor_binding(
    connection: &Connection,
    session_id: &str,
) -> CapabilityResult<Option<ActorBinding>> {
    connection
        .query_row(
            "SELECT cycle_id, goal_id, actor_identity, repository_json, grants_json,
                    engineer_permissions_json, working_directory_json, observe_only
             FROM actor_sessions WHERE session_id=?1",
            [session_id],
            actor_binding_from_row,
        )
        .optional()
        .map_err(persistence)
}

fn load_actor_session(
    connection: &Connection,
    session_id: &str,
) -> CapabilityResult<Option<StoredActorSession>> {
    connection
        .query_row(
            "SELECT cycle_id, goal_id, actor_identity, repository_json, grants_json,
                    engineer_permissions_json, working_directory_json, observe_only,
                    token_hash, expires_at
             FROM actor_sessions WHERE session_id=?1",
            [session_id],
            |row| {
                Ok(StoredActorSession {
                    binding: actor_binding_from_row(row)?,
                    token_hash: row.get(8)?,
                    expires_at: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(persistence)
}

fn actor_binding_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ActorBinding> {
    Ok(ActorBinding {
        cycle_id: row.get(0)?,
        goal_id: row.get(1)?,
        actor_identity: row.get(2)?,
        repository_json: row.get(3)?,
        grants_json: row.get(4)?,
        engineer_permissions_json: row.get(5)?,
        working_directory_json: row.get(6)?,
        observe_only: row.get(7)?,
    })
}

fn reserve_process_slot(
    transaction: &Transaction<'_>,
    identity: &TerminalRequestIdentity,
    limit: usize,
) -> CapabilityResult<()> {
    let reserved = transaction
        .query_row(
            "INSERT INTO mutation_scope_counters(
                session_id, cycle_id, goal_id, mutation_type, spent
             ) VALUES (?1, ?2, ?3, 'process_exec', 1)
             ON CONFLICT(session_id, cycle_id, goal_id, mutation_type)
             DO UPDATE SET spent=spent+1
             WHERE spent < ?4
             RETURNING spent",
            params![
                identity.session_id,
                identity.cycle_id,
                identity.goal_id,
                limit
            ],
            |row| row.get::<_, usize>(0),
        )
        .optional()
        .map_err(persistence)?;
    if reserved.is_none() || limit == 0 {
        return Err(CapabilityError::new(
            CapabilityErrorCode::MutationCapExhausted,
            "process_exec mutation cap is exhausted for this cycle",
        ));
    }
    Ok(())
}

fn new_process_execution(request: &ProcessExecRequest) -> ProcessExecutionRecord {
    ProcessExecutionRecord {
        execution_id: Uuid::now_v7().to_string(),
        request_id: request.identity.request_id.clone(),
        session_id: request.identity.session_id.clone(),
        cycle_id: request.identity.cycle_id.clone(),
        goal_id: request.identity.goal_id.clone(),
        status: ProcessExecutionStatus::Reserved,
        exit_code: None,
        stdout: Vec::new(),
        stderr: Vec::new(),
    }
}

fn insert_process_execution(
    transaction: &Transaction<'_>,
    request: &ProcessExecRequest,
    record: &ProcessExecutionRecord,
    fingerprint: &str,
    result_json: &[u8],
) -> CapabilityResult<()> {
    let request_json = serde_json::to_vec(request).map_err(serialization)?;
    transaction
        .execute(
            "INSERT INTO process_executions(
                execution_id, request_id, request_hash, session_id, cycle_id,
                goal_id, status, request_json, result_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.execution_id,
                record.request_id,
                fingerprint,
                record.session_id,
                record.cycle_id,
                record.goal_id,
                record.status.as_str(),
                request_json,
                result_json
            ],
        )
        .map_err(persistence)?;
    Ok(())
}

fn insert_terminal(
    transaction: &Transaction<'_>,
    outcome: &TerminalOutcome,
    fingerprint: &str,
    json: &[u8],
) -> CapabilityResult<()> {
    transaction
        .execute(
            "INSERT INTO terminal_outcomes(request_id, request_hash, session_id, cycle_id, outcome_id, outcome_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                outcome.request_id,
                fingerprint,
                outcome.session_id,
                outcome.cycle_id,
                outcome.outcome_id,
                json
            ],
        )
        .map_err(persistence)?;
    Ok(())
}

fn replay_request<T: DeserializeOwned>(
    transaction: &Transaction<'_>,
    request_id: &str,
    mutation_type: &str,
    fingerprint: &str,
) -> CapabilityResult<Option<T>> {
    let existing: Option<(String, String, Vec<u8>, u32, u32)> = transaction
        .query_row(
            "SELECT mutation_type, request_hash, result_json,
                    request_format_version, result_format_version
             FROM mutation_requests WHERE request_id=?1",
            [request_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(persistence)?;
    let Some((stored_type, stored_hash, json, request_version, result_version)) = existing else {
        return Ok(None);
    };
    if stored_type != mutation_type
        || stored_hash != fingerprint
        || request_version != 2
        || result_version != 1
    {
        return Err(request_conflict(request_id));
    }
    serde_json::from_slice(&json)
        .map(Some)
        .map_err(serialization)
}

fn record_request<T: Serialize>(
    transaction: &Transaction<'_>,
    request_id: &str,
    mutation_type: &str,
    fingerprint: &str,
    result: &T,
) -> CapabilityResult<()> {
    let json = serde_json::to_vec(result).map_err(serialization)?;
    record_request_json(transaction, request_id, mutation_type, fingerprint, &json)
}

fn record_request_json(
    transaction: &Transaction<'_>,
    request_id: &str,
    mutation_type: &str,
    fingerprint: &str,
    json: &[u8],
) -> CapabilityResult<()> {
    transaction
        .execute(
            "INSERT INTO mutation_requests(
                request_id, mutation_type, request_hash, result_json,
                request_format_version, result_format_version
             ) VALUES (?1, ?2, ?3, ?4, 2, 1)",
            params![request_id, mutation_type, fingerprint, json],
        )
        .map_err(|error| {
            if is_constraint(&error) {
                request_conflict(request_id)
            } else {
                persistence(error)
            }
        })?;
    Ok(())
}

fn ensure_cycle_open(
    transaction: &Transaction<'_>,
    identity: &TerminalRequestIdentity,
) -> CapabilityResult<()> {
    let existing: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM terminal_outcomes WHERE session_id=?1 AND cycle_id=?2)",
            params![identity.session_id, identity.cycle_id],
            |row| row.get(0),
        )
        .map_err(persistence)?;
    if existing {
        return Err(CapabilityError::new(
            CapabilityErrorCode::TerminalAlreadyRecorded,
            "cycle already has a terminal outcome",
        ));
    }
    Ok(())
}

fn insert_effect(
    transaction: &Transaction<'_>,
    outcome: &TerminalOutcome,
    action: &Action,
) -> CapabilityResult<()> {
    let kind = match action.kind() {
        ActionKind::SpawnEngineer => "spawn_engineer",
        ActionKind::FileIssue => "file_issue",
        ActionKind::RequestMerge => "request_merge",
        ActionKind::RequestDeploy => "request_deploy",
    };
    let action_json = serde_json::to_vec(action).map_err(serialization)?;
    transaction
        .execute(
            "INSERT INTO effect_jobs(effect_id, outcome_id, request_id, kind, state, action_json) VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
            params![
                Uuid::now_v7().to_string(),
                outcome.outcome_id,
                outcome.request_id,
                kind,
                action_json
            ],
        )
        .map_err(persistence)?;
    Ok(())
}

fn query_effect<P>(
    connection: &Connection,
    sql: &str,
    parameters: P,
) -> CapabilityResult<Option<EffectJob>>
where
    P: rusqlite::Params,
{
    type EffectRow = (
        String,
        String,
        String,
        String,
        String,
        Vec<u8>,
        u32,
        u64,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
    );
    let row: Option<EffectRow> = connection
        .query_row(sql, parameters, |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
                row.get(11)?,
                row.get(12)?,
            ))
        })
        .optional()
        .map_err(persistence)?;
    row.map(
        |(
            effect_id,
            outcome_id,
            request_id,
            kind,
            state,
            action_json,
            attempt,
            lease_generation,
            lease_owner,
            lease_expires_at_unix_millis,
            error,
            result_json,
            approval_json,
        )| {
            let outcome = terminal_for_outcome_id(connection, &outcome_id)?;
            Ok(EffectJob {
                effect_id,
                outcome_id,
                request_id,
                goal_id: outcome.goal_id,
                repository: outcome.repository,
                kind: EffectKind(kind),
                state: EffectState(state),
                action: serde_json::from_slice(&action_json).map_err(serialization)?,
                attempt,
                lease_generation,
                lease_owner,
                lease_expires_at_unix_millis,
                error,
                result: result_json
                    .map(|json| serde_json::from_slice(&json).map_err(serialization))
                    .transpose()?,
                approval: approval_json
                    .map(|json| serde_json::from_slice(&json).map_err(serialization))
                    .transpose()?,
            })
        },
    )
    .transpose()
}

fn query_effect_by_id(
    connection: &Connection,
    effect_id: &str,
) -> CapabilityResult<Option<EffectJob>> {
    query_effect(
        connection,
        "
        SELECT effect_id, outcome_id, request_id, kind, state, action_json, attempt,
               lease_generation, lease_owner, lease_expires_at, error, result_json,
               (SELECT decision_json FROM authorization_decisions
                WHERE effect_id=effect_jobs.effect_id AND decision='approved'
                ORDER BY recorded_at DESC, rowid DESC LIMIT 1)
        FROM effect_jobs WHERE effect_id=?1
        ",
        [effect_id],
    )
}

fn terminal_for_outcome_id(
    connection: &Connection,
    outcome_id: &str,
) -> CapabilityResult<TerminalOutcome> {
    let json: Vec<u8> = connection
        .query_row(
            "SELECT outcome_json FROM terminal_outcomes WHERE outcome_id=?1",
            [outcome_id],
            |row| row.get(0),
        )
        .map_err(persistence)?;
    serde_json::from_slice(&json).map_err(serialization)
}

pub fn action_payload_hash(action: &Action) -> CapabilityResult<String> {
    let bytes = serde_json::to_vec(action).map_err(serialization)?;
    Ok(sha256_hex(&bytes))
}

fn approval_signature(
    signing_key: &[u8],
    approval: &PrivilegedApproval,
) -> CapabilityResult<String> {
    #[derive(Serialize)]
    struct UnsignedApproval<'a> {
        approval_id: &'a str,
        principal: &'a str,
        effect_id: &'a str,
        outcome_id: &'a str,
        session_id: &'a str,
        cycle_id: &'a str,
        goal_id: &'a str,
        action_kind: ActionKind,
        canonical_payload_hash: &'a str,
        repository: &'a Option<RepositoryRef>,
        policy_revision: &'a str,
        issued_at_unix_millis: i64,
    }
    let unsigned = UnsignedApproval {
        approval_id: &approval.approval_id,
        principal: &approval.principal,
        effect_id: &approval.effect_id,
        outcome_id: &approval.outcome_id,
        session_id: &approval.session_id,
        cycle_id: &approval.cycle_id,
        goal_id: &approval.goal_id,
        action_kind: approval.action_kind,
        canonical_payload_hash: &approval.canonical_payload_hash,
        repository: &approval.repository,
        policy_revision: &approval.policy_revision,
        issued_at_unix_millis: approval.issued_at_unix_millis,
    };
    let message = serde_json::to_vec(&unsigned).map_err(serialization)?;
    Ok(hmac_sha256_hex(signing_key, &message))
}

fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    const BLOCK_SIZE: usize = 64;
    let mut normalized = [0_u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        normalized[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; BLOCK_SIZE];
    let mut outer_pad = [0x5c_u8; BLOCK_SIZE];
    for index in 0..BLOCK_SIZE {
        inner_pad[index] ^= normalized[index];
        outer_pad[index] ^= normalized[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    format!("{:x}", outer.finalize())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn fingerprint<T: Serialize>(
    actor: &AuthenticatedToolContext,
    policy_revision: &str,
    request: &T,
) -> CapabilityResult<String> {
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(
        &mut writer,
        &(
            "simard-canonical-request-v2",
            "sha256",
            actor.actor_identity.as_str(),
            actor.session_id.as_str(),
            actor.grants(),
            actor.bound_repository(),
            actor.bound_cycle_id(),
            actor.bound_goal_id(),
            actor.bound_working_directory(),
            actor.engineer_permissions(),
            actor.is_observe_only(),
            policy_revision,
            request,
        ),
    )
    .map_err(serialization)?;
    Ok(format!("{:x}", writer.0.finalize()))
}

struct HashWriter(Sha256);

impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn system_time_millis(time: SystemTime) -> CapabilityResult<i64> {
    let duration = time.duration_since(UNIX_EPOCH).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::InvalidArgument,
            "time must not precede the Unix epoch",
        )
    })?;
    i64::try_from(duration.as_millis()).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::InvalidArgument,
            "time exceeds supported range",
        )
    })
}

fn persistence(error: rusqlite::Error) -> CapabilityError {
    CapabilityError::new(
        CapabilityErrorCode::PersistenceFailed,
        format!("typed outcome persistence failed: {error}"),
    )
}

fn persistence_message(message: impl Into<String>) -> CapabilityError {
    CapabilityError::new(CapabilityErrorCode::PersistenceFailed, message)
}

fn serialization(error: serde_json::Error) -> CapabilityError {
    CapabilityError::new(
        CapabilityErrorCode::PersistenceFailed,
        format!("typed record serialization failed: {error}"),
    )
}

fn request_conflict(request_id: &str) -> CapabilityError {
    CapabilityError::new(
        CapabilityErrorCode::RequestConflict,
        format!("request id {request_id:?} conflicts with a recorded mutation"),
    )
}

fn stale_lease(effect_id: &str) -> CapabilityError {
    CapabilityError::new(
        CapabilityErrorCode::StaleLease,
        format!("effect {effect_id:?} lease owner, generation, or expiry is stale"),
    )
}

fn effect_lease_owner(lease: &EffectJob) -> CapabilityResult<&str> {
    lease
        .lease_owner
        .as_deref()
        .filter(|owner| !owner.is_empty() && lease.lease_generation > 0)
        .ok_or_else(|| stale_lease(&lease.effect_id))
}

fn system_actor(lease: &EffectJob) -> AuthenticatedToolContext {
    AuthenticatedToolContext::new(
        "effect-worker",
        format!("effect:{}", lease.effect_id),
        std::iter::empty(),
    )
}

fn is_constraint(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(inner, _)
            if inner.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
pub enum EffectResult {
    Succeeded { evidence: Vec<EvidenceRef> },
    Failed { error: String },
}

#[cfg(test)]
mod approval_tests {
    use super::*;

    #[test]
    fn issued_approval_is_bound_to_the_exact_privileged_effect() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = CapabilityHandler::open(
            dir.path().join("outcomes.sqlite3"),
            CapabilityPolicy::new("policy-v1"),
        )
        .expect("handler");
        let actor = AuthenticatedToolContext::new(
            "goal-session-actor",
            "session-approval",
            [CapabilityGrant::RecordAction(ActionKind::RequestMerge)],
        )
        .scoped_to_repository(RepositoryRef::new("rysweet", "Simard"))
        .bound_to_cycle_goal("cycle-approval", "goal-approval");
        let outcome = handler
            .record_action(
                &actor,
                RecordActionRequest {
                    identity: TerminalRequestIdentity::new(
                        "request-approval",
                        "session-approval",
                        "cycle-approval",
                        "goal-approval",
                    ),
                    action: Action::RequestMerge(RequestMergeAction {
                        pull_request: PullRequestRef {
                            repository: RepositoryRef::new("rysweet", "Simard"),
                            number: 42,
                        },
                        expected_head_sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
                        strategy: "squash".to_string(),
                    }),
                    raw_semantic: OpaqueBytes::from(b"approved request".to_vec()),
                    evidence: Vec::new(),
                },
                &AdmissionSnapshot {
                    concurrent_engineers: 0,
                    disk_used_percent: 1,
                    active_claims: BTreeSet::new(),
                    policy_revision: "policy-v1".to_string(),
                },
            )
            .expect("record request");
        let effect = handler
            .effect_for_outcome(&outcome.outcome_id)
            .expect("query effect")
            .expect("effect");
        handler
            .block_effect_authorization(
                &effect,
                "request-block-privileged-effect",
                "approval required",
            )
            .expect("block effect");

        let authority = ApprovalAuthority::for_test("release-operator");
        let missing = handler
            .issue_privileged_approval(&authority, "", &effect.effect_id)
            .expect_err("approval request id is required");
        assert_eq!(missing.code(), CapabilityErrorCode::InvalidIdentifier);
        let cross_type = handler
            .issue_privileged_approval(
                &authority,
                "request-block-privileged-effect",
                &effect.effect_id,
            )
            .expect_err("authorization block request id cannot be reused for approval");
        assert_eq!(cross_type.code(), CapabilityErrorCode::RequestConflict);

        let approval = handler
            .issue_privileged_approval(
                &authority,
                "request-approve-privileged-effect",
                &effect.effect_id,
            )
            .expect("issue approval");
        let replay = handler
            .issue_privileged_approval(
                &authority,
                "request-approve-privileged-effect",
                &effect.effect_id,
            )
            .expect("approval replay");
        assert_eq!(replay, approval);
        let approved = handler
            .effect_for_outcome(&outcome.outcome_id)
            .expect("query approved effect")
            .expect("approved effect");
        assert_eq!(approved.state.as_str(), "pending");
        assert_eq!(approved.approval.as_ref(), Some(&approval));
        assert_eq!(approval.goal_id, "goal-approval");
        assert_eq!(approval.cycle_id, "cycle-approval");
        assert_eq!(approval.action_kind, ActionKind::RequestMerge);
        assert_eq!(
            approval.repository,
            Some(RepositoryRef::new("rysweet", "Simard"))
        );
        assert_eq!(
            approval.canonical_payload_hash,
            action_payload_hash(&approved.action).expect("payload hash")
        );
        assert!(!approval.signature.is_empty());
    }
}
