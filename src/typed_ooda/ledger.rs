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
    pub kind: EffectKind,
    pub state: EffectState,
    pub action: Action,
    pub attempt: u32,
    pub lease_owner: Option<String>,
    pub lease_expires_at_unix_millis: Option<i64>,
    pub error: Option<String>,
    pub result: Option<EffectResult>,
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
                ",
            )
            .map_err(persistence)?;
        ensure_effect_result_column(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
            policy,
        })
    }

    pub fn record_action(
        &self,
        actor: &AuthenticatedToolContext,
        request: RecordActionRequest,
        admission: &AdmissionSnapshot,
    ) -> CapabilityResult<TerminalOutcome> {
        self.authorize(
            actor,
            &request.identity,
            CapabilityGrant::RecordAction(request.action.kind()),
        )?;
        self.validate_common(&request.identity, &request.raw_semantic, &request.evidence)?;
        self.validate_action(&request.action)?;

        let payload = TypedOutcomePayload::Action(ActionOutcomePayload {
            action: request.action.clone(),
            admission: AdmissionDecision {
                policy_revision: admission.policy_revision.clone(),
            },
        });
        let fingerprint = fingerprint(actor, &request)?;
        let outcome = self.new_outcome(
            actor,
            &request.identity,
            TerminalKind::Action,
            payload,
            request.raw_semantic,
            request.evidence,
        );

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
        insert_terminal(&transaction, &outcome, &fingerprint)?;
        if let Action::SpawnEngineer(spawn) = &request.action {
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
        insert_effect(&transaction, &outcome, &request.action)?;
        transaction.commit().map_err(persistence)?;
        Ok(outcome)
    }

    pub fn record_no_action(
        &self,
        actor: &AuthenticatedToolContext,
        request: RecordNoActionRequest,
    ) -> CapabilityResult<TerminalOutcome> {
        self.authorize(actor, &request.identity, CapabilityGrant::RecordNoAction)?;
        self.validate_common(&request.identity, &request.raw_semantic, &request.evidence)?;
        self.validate_opaque("reason", &request.reason, true)?;
        self.commit_terminal(
            actor,
            &request.identity,
            TerminalKind::NoAction,
            TypedOutcomePayload::NoAction(NoActionOutcomePayload {
                reason: request.reason.clone(),
            }),
            request.raw_semantic.clone(),
            request.evidence.clone(),
            fingerprint(actor, &request)?,
        )
    }

    pub fn record_blocked(
        &self,
        actor: &AuthenticatedToolContext,
        request: RecordBlockedRequest,
    ) -> CapabilityResult<TerminalOutcome> {
        self.authorize(actor, &request.identity, CapabilityGrant::RecordBlocked)?;
        self.validate_common(&request.identity, &request.raw_semantic, &request.evidence)?;
        self.validate_opaque("reason", &request.reason, true)?;
        self.commit_terminal(
            actor,
            &request.identity,
            TerminalKind::Blocked,
            TypedOutcomePayload::Blocked(BlockedOutcomePayload {
                reason: request.reason.clone(),
                blocker: request.blocker.clone(),
                retry: request.retry.clone(),
            }),
            request.raw_semantic.clone(),
            request.evidence.clone(),
            fingerprint(actor, &request)?,
        )
    }

    pub fn record_completed(
        &self,
        actor: &AuthenticatedToolContext,
        request: RecordCompletedRequest,
    ) -> CapabilityResult<TerminalOutcome> {
        self.authorize(actor, &request.identity, CapabilityGrant::RecordCompleted)?;
        self.validate_common(&request.identity, &request.raw_semantic, &request.evidence)?;
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
        self.commit_terminal(
            actor,
            &request.identity,
            TerminalKind::Completed,
            TypedOutcomePayload::Completed(CompletedOutcomePayload {
                summary: request.summary.clone(),
                completion: request.completion.clone(),
            }),
            request.raw_semantic.clone(),
            request.evidence.clone(),
            fingerprint(actor, &request)?,
        )
    }

    pub fn record_progress(
        &self,
        actor: &AuthenticatedToolContext,
        request: RecordProgressRequest,
    ) -> CapabilityResult<ProgressRecord> {
        self.authorize(actor, &request.identity, CapabilityGrant::RecordProgress)?;
        self.validate_identity(&request.identity)?;
        self.validate_opaque("progress summary", &request.summary, true)?;
        validate_evidence(&request.evidence)?;
        if request.percent > 100 {
            return Err(CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "progress percent must be in 0..=100",
            ));
        }
        let fingerprint = fingerprint(actor, &request)?;
        let record = ProgressRecord {
            progress_id: Uuid::now_v7().to_string(),
            request_id: request.identity.request_id.clone(),
            session_id: request.identity.session_id.clone(),
            actor_identity: actor.actor_identity.clone(),
            goal_id: request.identity.goal_id.clone(),
            cycle_id: request.identity.cycle_id.clone(),
            percent: request.percent,
            summary: request.summary,
            evidence: request.evidence,
            recorded_at_unix_millis: now_millis(),
        };
        let json = serde_json::to_vec(&record).map_err(serialization)?;
        let connection = self.lock()?;
        let existing: Option<(String, Vec<u8>)> = connection
            .query_row(
                "SELECT request_hash, progress_json FROM progress_records WHERE request_id=?1",
                [&record.request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(persistence)?;
        if let Some((stored_hash, stored_json)) = existing {
            if stored_hash != fingerprint {
                return Err(idempotency_conflict(&record.request_id));
            }
            return serde_json::from_slice(&stored_json).map_err(serialization);
        }
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
        connection
            .query_row(
                "SELECT COUNT(*) FROM terminal_outcomes WHERE session_id=?1 AND cycle_id=?2",
                params![session_id, cycle_id],
                |row| row.get(0),
            )
            .map_err(persistence)
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
        query_effect(&connection, "WHERE outcome_id=?1", [outcome_id])
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
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(persistence)?;
        let effect_id: Option<String> = transaction
            .query_row(
                "SELECT effect_id FROM effect_jobs WHERE state='pending' ORDER BY rowid LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(persistence)?;
        let Some(effect_id) = effect_id else {
            transaction.commit().map_err(persistence)?;
            return Ok(None);
        };
        transaction
            .execute(
                "UPDATE effect_jobs SET state='running', attempt=attempt+1, lease_owner=?2, lease_expires_at=?3 WHERE effect_id=?1 AND state='pending'",
                params![effect_id, worker, now.saturating_add(lease_millis)],
            )
            .map_err(persistence)?;
        let job = query_effect(&transaction, "WHERE effect_id=?1", [&effect_id])?
            .ok_or_else(|| persistence_message("claimed effect disappeared"))?;
        transaction.commit().map_err(persistence)?;
        Ok(Some(job))
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
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(persistence)?;
        let changed = transaction
            .execute(
                "UPDATE effect_jobs SET state='running', attempt=attempt+1, lease_owner=?2, lease_expires_at=?3 WHERE outcome_id=?1 AND state='pending'",
                params![outcome_id, worker, now.saturating_add(lease_millis)],
            )
            .map_err(persistence)?;
        if changed == 0 {
            transaction.commit().map_err(persistence)?;
            return Ok(None);
        }
        let job = query_effect(&transaction, "WHERE outcome_id=?1", [outcome_id])?
            .ok_or_else(|| persistence_message("claimed effect disappeared"))?;
        transaction.commit().map_err(persistence)?;
        Ok(Some(job))
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

    #[allow(clippy::too_many_arguments)]
    fn commit_terminal(
        &self,
        actor: &AuthenticatedToolContext,
        identity: &TerminalRequestIdentity,
        kind: TerminalKind,
        payload: TypedOutcomePayload,
        raw_semantic: OpaqueBytes,
        evidence: Vec<EvidenceRef>,
        fingerprint: String,
    ) -> CapabilityResult<TerminalOutcome> {
        let outcome = self.new_outcome(actor, identity, kind, payload, raw_semantic, evidence);
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(persistence)?;
        if let Some(existing) = replay_terminal(&transaction, &identity.request_id, &fingerprint)? {
            return Ok(existing);
        }
        ensure_cycle_open(&transaction, identity)?;
        insert_terminal(&transaction, &outcome, &fingerprint)?;
        transaction.commit().map_err(persistence)?;
        Ok(outcome)
    }

    fn new_outcome(
        &self,
        actor: &AuthenticatedToolContext,
        identity: &TerminalRequestIdentity,
        kind: TerminalKind,
        payload: TypedOutcomePayload,
        raw_semantic: OpaqueBytes,
        evidence: Vec<EvidenceRef>,
    ) -> TerminalOutcome {
        TerminalOutcome {
            outcome_id: Uuid::now_v7().to_string(),
            request_id: identity.request_id.clone(),
            session_id: identity.session_id.clone(),
            actor_identity: actor.actor_identity.clone(),
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

    fn validate_identity(&self, identity: &TerminalRequestIdentity) -> CapabilityResult<()> {
        validate_identifier("request id", &identity.request_id)?;
        validate_identifier("session id", &identity.session_id)?;
        validate_identifier("cycle id", &identity.cycle_id)?;
        validate_identifier("goal id", &identity.goal_id)
    }

    fn validate_common(
        &self,
        identity: &TerminalRequestIdentity,
        raw_semantic: &OpaqueBytes,
        evidence: &[EvidenceRef],
    ) -> CapabilityResult<()> {
        self.validate_identity(identity)?;
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
        if !self
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
    let mut statement = connection
        .prepare("PRAGMA table_info(effect_jobs)")
        .map_err(persistence)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(persistence)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(persistence)?;
    if !columns.iter().any(|column| column == "result_json") {
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
    clause: &str,
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
    );
    let sql = format!(
        "SELECT effect_id, outcome_id, request_id, kind, state, action_json, attempt, lease_owner, lease_expires_at, error, result_json FROM effect_jobs {clause}"
    );
    let row: Option<EffectRow> = connection
        .query_row(&sql, parameters, |row| {
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
        )| {
            Ok(EffectJob {
                effect_id,
                outcome_id,
                request_id,
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
            })
        },
    )
    .transpose()
}

fn fingerprint<T: Serialize>(
    actor: &AuthenticatedToolContext,
    request: &T,
) -> CapabilityResult<String> {
    let bytes =
        serde_json::to_vec(&(actor.actor_identity.as_str(), request)).map_err(serialization)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
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
