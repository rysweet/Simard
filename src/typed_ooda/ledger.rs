use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
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
    pub lease_owner: Option<String>,
    pub lease_expires_at_unix_millis: Option<i64>,
    pub error: Option<String>,
    pub result: Option<EffectResult>,
    pub approval: Option<PrivilegedApproval>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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

impl std::fmt::Debug for CapabilityHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapabilityHandler")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl CapabilityHandler {
    pub fn open(path: impl AsRef<Path>, policy: CapabilityPolicy) -> CapabilityResult<Self> {
        let connection = Connection::open(path.as_ref()).map_err(persistence)?;
        connection
            .execute_batch(
                "
                PRAGMA foreign_keys = ON;
                PRAGMA journal_mode = WAL;
                CREATE TABLE IF NOT EXISTS terminal_outcomes (
                    request_id TEXT PRIMARY KEY,
                    request_hash TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    cycle_id TEXT NOT NULL,
                    outcome_id TEXT NOT NULL UNIQUE,
                    outcome_json BLOB NOT NULL,
                    UNIQUE(session_id, cycle_id)
                );
                CREATE TABLE IF NOT EXISTS progress_records (
                    request_id TEXT PRIMARY KEY,
                    request_hash TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    cycle_id TEXT NOT NULL,
                    progress_json BLOB NOT NULL
                );
                CREATE TABLE IF NOT EXISTS effect_jobs (
                    effect_id TEXT PRIMARY KEY,
                    outcome_id TEXT NOT NULL UNIQUE,
                    request_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    state TEXT NOT NULL,
                    action_json BLOB NOT NULL,
                    attempt INTEGER NOT NULL DEFAULT 0,
                    lease_owner TEXT,
                    lease_expires_at INTEGER,
                    error TEXT,
                    result_json BLOB,
                    FOREIGN KEY(outcome_id) REFERENCES terminal_outcomes(outcome_id)
                );
                CREATE TABLE IF NOT EXISTS engineer_claims (
                    claim_key TEXT PRIMARY KEY,
                    outcome_id TEXT NOT NULL UNIQUE,
                    request_id TEXT NOT NULL,
                    FOREIGN KEY(outcome_id) REFERENCES terminal_outcomes(outcome_id)
                );
                CREATE TABLE IF NOT EXISTS actor_sessions (
                    session_id TEXT NOT NULL,
                    cycle_id TEXT NOT NULL,
                    goal_id TEXT NOT NULL,
                    actor_identity TEXT NOT NULL,
                    repository_json BLOB NOT NULL,
                    grants_json BLOB NOT NULL,
                    observe_only INTEGER NOT NULL,
                    token_hash TEXT NOT NULL,
                    expires_at INTEGER NOT NULL,
                    PRIMARY KEY(session_id, cycle_id)
                );
                CREATE TABLE IF NOT EXISTS authorization_decisions (
                    decision_id TEXT PRIMARY KEY,
                    effect_id TEXT NOT NULL,
                    decision TEXT NOT NULL,
                    decision_json BLOB NOT NULL,
                    recorded_at INTEGER NOT NULL,
                    FOREIGN KEY(effect_id) REFERENCES effect_jobs(effect_id)
                );
                CREATE INDEX IF NOT EXISTS progress_records_cycle_idx
                    ON progress_records(session_id, cycle_id);
                CREATE INDEX IF NOT EXISTS effect_jobs_state_lease_idx
                    ON effect_jobs(state, lease_expires_at);
                CREATE INDEX IF NOT EXISTS authorization_decisions_effect_idx
                    ON authorization_decisions(effect_id, recorded_at);
                ",
            )
            .map_err(persistence)?;
        ensure_effect_result_column(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
            policy,
        })
    }

    pub fn register_actor_session(
        &self,
        actor: &AuthenticatedToolContext,
        cycle_id: &str,
        goal_id: &str,
        ttl: Duration,
    ) -> CapabilityResult<ActorSessionLease> {
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

        let token = Uuid::new_v4().simple().to_string();
        let expires_at_unix_millis = now_millis().saturating_add(ttl_millis);
        let repository_json = serde_json::to_vec(repository).map_err(serialization)?;
        let grants_json = serde_json::to_vec(actor.grants()).map_err(serialization)?;
        let token_hash = sha256_hex(token.as_bytes());
        let connection = self.lock()?;
        connection
            .execute(
                "DELETE FROM actor_sessions WHERE expires_at < ?1",
                [now_millis()],
            )
            .map_err(persistence)?;
        connection
            .execute(
                "INSERT INTO actor_sessions(
                    session_id, cycle_id, goal_id, actor_identity, repository_json,
                    grants_json, observe_only, token_hash, expires_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(session_id, cycle_id) DO UPDATE SET
                    goal_id=excluded.goal_id,
                    actor_identity=excluded.actor_identity,
                    repository_json=excluded.repository_json,
                    grants_json=excluded.grants_json,
                    observe_only=excluded.observe_only,
                    token_hash=excluded.token_hash,
                    expires_at=excluded.expires_at",
                params![
                    actor.session_id,
                    cycle_id,
                    goal_id,
                    actor.actor_identity,
                    repository_json,
                    grants_json,
                    actor.is_observe_only(),
                    token_hash,
                    expires_at_unix_millis
                ],
            )
            .map_err(persistence)?;
        Ok(ActorSessionLease {
            token,
            expires_at_unix_millis,
        })
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
        type SessionRow = (String, String, Vec<u8>, Vec<u8>, bool, String, i64);
        let row: Option<SessionRow> = connection
            .query_row(
                "SELECT goal_id, actor_identity, repository_json, grants_json,
                        observe_only, token_hash, expires_at
                 FROM actor_sessions WHERE session_id=?1 AND cycle_id=?2",
                params![session_id, cycle_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .optional()
            .map_err(persistence)?;
        let Some((
            stored_goal,
            actor_identity,
            repository_json,
            grants_json,
            observe_only,
            token_hash,
            expires_at,
        )) = row
        else {
            return Err(CapabilityError::new(
                CapabilityErrorCode::Unauthenticated,
                "actor session lease was not found",
            ));
        };
        if stored_goal != goal_id
            || expires_at < now_millis()
            || token_hash != sha256_hex(token.as_bytes())
        {
            return Err(CapabilityError::new(
                CapabilityErrorCode::Unauthenticated,
                "actor session lease is expired or does not match the invocation",
            ));
        }
        let repository: RepositoryRef =
            serde_json::from_slice(&repository_json).map_err(serialization)?;
        let grants: BTreeSet<CapabilityGrant> =
            serde_json::from_slice(&grants_json).map_err(serialization)?;
        Ok(
            AuthenticatedToolContext::new(actor_identity, session_id, grants)
                .scoped_to_repository(repository)
                .with_observe_only(observe_only),
        )
    }

    pub fn issue_privileged_approval(
        &self,
        authority: &ApprovalAuthority,
        effect_id: &str,
    ) -> CapabilityResult<PrivilegedApproval> {
        validate_identifier("effect id", effect_id)?;
        let connection = self.lock()?;
        let job = query_effect_by_id(&connection, effect_id)?.ok_or_else(|| {
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
        let outcome = terminal_for_outcome_id(&connection, &job.outcome_id)?;
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
        let transaction = connection.unchecked_transaction().map_err(persistence)?;
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
        transaction.commit().map_err(persistence)?;
        Ok(approval)
    }

    pub fn record_action(
        &self,
        actor: &AuthenticatedToolContext,
        request: RecordActionRequest,
        admission: &AdmissionSnapshot,
    ) -> CapabilityResult<TerminalOutcome> {
        self.validate_identity(&request.identity)?;
        if actor.actor_identity.is_empty() || actor.session_id != request.identity.session_id {
            return Err(CapabilityError::new(
                CapabilityErrorCode::Unauthenticated,
                "authenticated actor session does not match request session",
            ));
        }
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
        self.validate_common(&request.raw_semantic, &request.evidence)?;
        if let Err(error) = self.validate_action(&request.action) {
            if error.code() == CapabilityErrorCode::PermissionDenied {
                return self.record_action_denied(actor, request, grant);
            }
            return Err(error);
        }

        let fingerprint = fingerprint(actor, &self.policy.revision, &request)?;

        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(persistence)?;
        if let Some(existing) =
            replay_terminal(&transaction, &request.identity.request_id, &fingerprint)?
        {
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
        insert_terminal(&transaction, &outcome, &fingerprint)?;
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
        let connection = self.lock()?;
        let existing: Option<(String, Vec<u8>)> = connection
            .query_row(
                "SELECT request_hash, progress_json FROM progress_records WHERE request_id=?1",
                [&request.identity.request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(persistence)?;
        if let Some((stored_hash, stored_json)) = existing {
            if stored_hash != fingerprint {
                return Err(idempotency_conflict(&request.identity.request_id));
            }
            return serde_json::from_slice(&stored_json).map_err(serialization);
        }
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
        connection
            .execute(
                "INSERT INTO progress_records(request_id, request_hash, session_id, cycle_id, progress_json) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![record.request_id, fingerprint, record.session_id, record.cycle_id, json],
            )
            .map_err(persistence)?;
        Ok(record)
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
                   lease_owner, lease_expires_at, error, result_json,
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
        now: SystemTime,
        lease: Duration,
    ) -> CapabilityResult<Option<EffectJob>> {
        validate_identifier("effect worker", worker)?;
        let now = system_time_millis(now)?;
        let lease_millis = i64::try_from(lease.as_millis()).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "effect lease is too long",
            )
        })?;
        let connection = self.lock()?;
        query_effect(
            &connection,
            "
            UPDATE effect_jobs
            SET state='running', attempt=attempt+1, lease_owner=?1, lease_expires_at=?2
            WHERE effect_id = (
                SELECT effect_id FROM effect_jobs
                WHERE state='pending'
                ORDER BY rowid
                LIMIT 1
            )
            AND state='pending'
            RETURNING effect_id, outcome_id, request_id, kind, state, action_json, attempt,
                      lease_owner, lease_expires_at, error, result_json,
                      (SELECT decision_json FROM authorization_decisions
                       WHERE effect_id=effect_jobs.effect_id AND decision='approved'
                       ORDER BY recorded_at DESC, rowid DESC LIMIT 1)
            ",
            params![worker, now.saturating_add(lease_millis)],
        )
    }

    pub(crate) fn claim_effect_for_outcome(
        &self,
        outcome_id: &str,
        worker: &str,
        now: SystemTime,
        lease: Duration,
    ) -> CapabilityResult<Option<EffectJob>> {
        validate_identifier("effect worker", worker)?;
        validate_identifier("outcome id", outcome_id)?;
        let now = system_time_millis(now)?;
        let lease_millis = i64::try_from(lease.as_millis()).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "effect lease is too long",
            )
        })?;
        let connection = self.lock()?;
        query_effect(
            &connection,
            "
            UPDATE effect_jobs
            SET state='running', attempt=attempt+1, lease_owner=?2, lease_expires_at=?3
            WHERE outcome_id=?1 AND state='pending'
            RETURNING effect_id, outcome_id, request_id, kind, state, action_json, attempt,
                      lease_owner, lease_expires_at, error, result_json,
                      (SELECT decision_json FROM authorization_decisions
                       WHERE effect_id=effect_jobs.effect_id AND decision='approved'
                       ORDER BY recorded_at DESC, rowid DESC LIMIT 1)
            ",
            params![outcome_id, worker, now.saturating_add(lease_millis)],
        )
    }

    pub fn recover_expired_effects(&self, now: SystemTime) -> CapabilityResult<usize> {
        let now = system_time_millis(now)?;
        let connection = self.lock()?;
        connection
            .execute(
                "UPDATE effect_jobs SET state='pending', lease_owner=NULL, lease_expires_at=NULL WHERE state='running' AND lease_expires_at <= ?1",
                [now],
            )
            .map_err(persistence)
    }

    pub(crate) fn block_effect_authorization(
        &self,
        job: &EffectJob,
        reason: &str,
    ) -> CapabilityResult<()> {
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
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction().map_err(persistence)?;
        transaction
            .execute(
                "INSERT INTO authorization_decisions(
                    decision_id, effect_id, decision, decision_json, recorded_at
                 ) VALUES (?1, ?2, 'blocked', ?3, ?4)",
                params![decision_id, job.effect_id, decision, recorded_at],
            )
            .map_err(persistence)?;
        let changed = transaction
            .execute(
                "UPDATE effect_jobs
                 SET state='blocked',
                     error=?2,
                     lease_owner=NULL,
                     lease_expires_at=NULL
                 WHERE effect_id=?1 AND state IN ('pending', 'running')",
                params![job.effect_id, reason],
            )
            .map_err(persistence)?;
        if changed != 1 {
            return Err(persistence_message(
                "privileged effect did not transition to blocked",
            ));
        }
        transaction.commit().map_err(persistence)
    }

    pub(crate) fn release_effect_for_retry(
        &self,
        effect_id: &str,
        error: &str,
    ) -> CapabilityResult<()> {
        let connection = self.lock()?;
        let changed = connection
            .execute(
                "UPDATE effect_jobs
                 SET state='pending', error=?2, lease_owner=NULL, lease_expires_at=NULL
                 WHERE effect_id=?1 AND state='running'",
                params![effect_id, error],
            )
            .map_err(persistence)?;
        if changed != 1 {
            return Err(persistence_message(
                "retryable effect did not return to pending",
            ));
        }
        Ok(())
    }

    pub(crate) fn finish_effect(
        &self,
        effect_id: &str,
        result: &EffectResult,
    ) -> CapabilityResult<()> {
        let (state, error) = match result {
            EffectResult::Succeeded { .. } => ("succeeded", None),
            EffectResult::Failed { error } => ("failed", Some(error.as_str())),
        };
        let result_json = serde_json::to_vec(result).map_err(serialization)?;
        let connection = self.lock()?;
        let changed = connection
            .execute(
                "UPDATE effect_jobs SET state=?2, error=?3, result_json=?4, lease_owner=NULL, lease_expires_at=NULL WHERE effect_id=?1 AND state='running'",
                params![effect_id, state, error, result_json],
            )
            .map_err(persistence)?;
        if changed != 1 {
            return Err(persistence_message(
                "effect state transition did not update exactly one running job",
            ));
        }
        Ok(())
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
        let transaction = connection.transaction().map_err(persistence)?;
        if let Some(existing) = replay_terminal(&transaction, &identity.request_id, &fingerprint)? {
            return Ok(existing);
        }
        ensure_cycle_open(&transaction, identity)?;
        let outcome = self.new_outcome(actor, identity, payload, raw_semantic, evidence);
        insert_terminal(&transaction, &outcome, &fingerprint)?;
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
        self.validate_identity(identity)?;
        if actor.actor_identity.is_empty() || actor.session_id != identity.session_id {
            return Err(CapabilityError::new(
                CapabilityErrorCode::Unauthenticated,
                "authenticated actor session does not match request session",
            ));
        }
        validate_identifier("actor identity", &actor.actor_identity)?;
        if !actor.allows(grant) || !self.policy.allows(grant) {
            return Err(CapabilityError::new(
                CapabilityErrorCode::PermissionDenied,
                "actor is not authorized for the requested capability",
            ));
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
        if snapshot.policy_revision.is_empty() {
            return Err(CapabilityError::new(
                CapabilityErrorCode::AdmissionRejected,
                "admission policy revision is missing",
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

fn insert_terminal(
    transaction: &Transaction<'_>,
    outcome: &TerminalOutcome,
    fingerprint: &str,
) -> CapabilityResult<()> {
    let json = serde_json::to_vec(outcome).map_err(serialization)?;
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

fn ensure_effect_result_column(connection: &Connection) -> CapabilityResult<()> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM pragma_table_info('effect_jobs') WHERE name='result_json'
            )",
            [],
            |row| row.get(0),
        )
        .map_err(persistence)?;
    if !exists {
        connection
            .execute("ALTER TABLE effect_jobs ADD COLUMN result_json BLOB", [])
            .map_err(persistence)?;
    }
    Ok(())
}

fn replay_terminal(
    transaction: &Transaction<'_>,
    request_id: &str,
    fingerprint: &str,
) -> CapabilityResult<Option<TerminalOutcome>> {
    let existing: Option<(String, Vec<u8>)> = transaction
        .query_row(
            "SELECT request_hash, outcome_json FROM terminal_outcomes WHERE request_id=?1",
            [request_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(persistence)?;
    let Some((stored_hash, json)) = existing else {
        return Ok(None);
    };
    if stored_hash != fingerprint {
        return Err(idempotency_conflict(request_id));
    }
    serde_json::from_slice(&json)
        .map(Some)
        .map_err(serialization)
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
               lease_owner, lease_expires_at, error, result_json,
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
        &(actor.actor_identity.as_str(), policy_revision, request),
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

fn idempotency_conflict(request_id: &str) -> CapabilityError {
    CapabilityError::new(
        CapabilityErrorCode::IdempotencyConflict,
        format!("request id {request_id:?} was reused with different arguments"),
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
            CapabilityPolicy::goal_session_default("policy-v1"),
        )
        .expect("handler");
        let actor = AuthenticatedToolContext::new(
            "goal-session-actor",
            "session-approval",
            [CapabilityGrant::RecordAction(ActionKind::RequestMerge)],
        )
        .scoped_to_repository(RepositoryRef::new("rysweet", "Simard"));
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
            .block_effect_authorization(&effect, "approval required")
            .expect("block effect");

        let approval = handler
            .issue_privileged_approval(
                &ApprovalAuthority::for_test("release-operator"),
                &effect.effect_id,
            )
            .expect("issue approval");
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
