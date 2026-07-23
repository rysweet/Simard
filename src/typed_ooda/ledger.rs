use std::collections::BTreeSet;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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

/// Authoritative liveness signal for an engineer claim.
///
/// The `engineer_claims` admission gate uses this to decide, on a `claim_key`
/// collision, whether the existing claim belongs to a genuinely-live engineer
/// (reject the duplicate spawn) or a dead/orphaned one (reclaim it). The trait
/// is injected so the ledger stays free of a dependency on the engineer
/// worktree / `ooda_actions` layers that own the real sentinel + PID scan.
///
/// Implementations MUST be fail-closed: return `true` for anything that is not
/// *provable death*. See the production provider in
/// `src/ooda_actions/advance_goal/typed_goal_session.rs`.
pub trait EngineerLiveness: Send + Sync {
    /// True iff `claim_key`'s engineer is actually alive right now.
    fn is_claim_live(&self, claim_key: &str) -> bool;
}

pub struct CapabilityHandler {
    connection: Mutex<Connection>,
    policy: CapabilityPolicy,
    engineer_liveness: Option<Box<dyn EngineerLiveness>>,
}

#[derive(Debug)]
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

/// Identity + authorization-scope key for a reused actor session.
///
/// This is the ONLY comparison used to detect an `AuthorizationScopeViolation`
/// when re-leasing an existing `session_id`. It deliberately excludes the
/// mutable per-cycle lease metadata (`cycle_id`/`goal_id`): re-leasing the same
/// stable session for a new cycle of the SAME identity and scope is legitimate
/// and refreshed via `ON CONFLICT(session_id) DO UPDATE`. A change to any of
/// these six fields on a reused `session_id` is a genuine security violation.
#[derive(Debug, Eq, PartialEq)]
struct ActorScopeKey {
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

    /// Extract the identity + authorization-scope key used for the reused-session
    /// violation check. Excludes mutable per-cycle metadata (`cycle_id`/`goal_id`).
    fn scope_key(&self) -> ActorScopeKey {
        ActorScopeKey {
            actor_identity: self.actor_identity.clone(),
            repository_json: self.repository_json.clone(),
            grants_json: self.grants_json.clone(),
            engineer_permissions_json: self.engineer_permissions_json.clone(),
            working_directory_json: self.working_directory_json.clone(),
            observe_only: self.observe_only,
        }
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
        // Issue #4483: absorb cross-process contention (daemon + engineer
        // worktree + reaper share one ledger file) by waiting-and-retrying for
        // up to 30s instead of erroring out with `database is locked`.
        connection
            .busy_timeout(Duration::from_secs(30))
            .map_err(persistence)?;
        // Issue #4483: apply WAL journal mode UNCONDITIONALLY at every open, not
        // only inside the one-time schema-migration branch. A ledger created
        // before WAL (or whose journal was reverted to rollback mode) is at
        // `user_version == 1`, so `schema::initialize` short-circuits before it
        // would re-assert WAL — leaving readers and writers serializing on a
        // whole-file lock and surfacing the persistence crash-loop.
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(persistence)?;
        // `pragma_update` ignores the journal-mode SQLite echoes back, so on an
        // exotic filesystem that silently refuses WAL (e.g. some network mounts)
        // the ledger would fall back to rollback journaling with no signal.
        // Read the mode back and warn — non-fatal, since rollback journaling
        // fails toward stricter (whole-file) locking, not data loss.
        match connection.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0)) {
            Ok(mode) if !mode.eq_ignore_ascii_case("wal") => {
                tracing::warn!(
                    journal_mode = %mode,
                    "ledger open() requested WAL but SQLite reports a different journal mode; \
                     concurrent readers/writers may serialize on a whole-file lock"
                );
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "ledger open() could not read back journal_mode to confirm WAL"
                );
            }
        }
        super::schema::initialize(&mut connection, now_millis()).map_err(persistence)?;
        Ok(Self {
            connection: Mutex::new(connection),
            policy,
            engineer_liveness: None,
        })
    }

    /// Inject the authoritative engineer-liveness provider used by the
    /// `engineer_claims` reclaim gate. Without a provider the gate is
    /// fail-closed: an existing claim is treated as live and a duplicate spawn
    /// is rejected, preserving the single-active-claim invariant.
    pub fn with_engineer_liveness(mut self, liveness: Box<dyn EngineerLiveness>) -> Self {
        self.engineer_liveness = Some(liveness);
        self
    }

    /// Release (delete) the engineer claim for `claim_key`.
    ///
    /// Idempotent: deleting a claim that does not exist is success (0 rows
    /// affected -> `Ok(())`). Runs in its own immediate transaction so a
    /// concurrent reclaim/insert cannot interleave a partial state.
    /// Fail-visible: a real SQL error is returned as `Err`, never swallowed.
    ///
    /// This is the deterministic release-on-termination call every engineer
    /// exit path flows through. See
    /// [`crate::typed_ooda::EngineerLiveness`] and the reference doc
    /// `docs/reference/engineer-claim-release-api.md`.
    pub fn release_engineer_claim(&self, claim_key: &str) -> CapabilityResult<()> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistence)?;
        transaction
            .execute(
                "DELETE FROM engineer_claims WHERE claim_key = ?1",
                params![claim_key],
            )
            .map_err(persistence)?;
        transaction.commit().map_err(persistence)?;
        Ok(())
    }

    /// List every `claim_key` currently held in `engineer_claims`.
    ///
    /// Read-only full scan of the tiny (cap-24) admission table. The periodic
    /// claim reaper (issue #4099) sweeps this to find claims whose engineer is
    /// provably dead, INDEPENDENT of whether that goal is being polled — the gap
    /// PR #4095's per-collision / per-goal reclaim paths do not close. Repo
    /// agnostic: keys span every repo the daemon serves.
    pub fn list_engineer_claims(&self) -> CapabilityResult<Vec<String>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT claim_key FROM engineer_claims")
            .map_err(persistence)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(persistence)?;
        rows.map(|row| row.map_err(persistence)).collect()
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
            .is_some_and(|existing| existing.scope_key() != binding.scope_key())
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
            self.insert_engineer_claim(
                &transaction,
                &spawn.claim_key,
                &outcome.outcome_id,
                &outcome.request_id,
            )?;
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

        match capture_process_output(&request) {
            Ok(captured) => {
                record.status = if captured.success {
                    ProcessExecutionStatus::Completed
                } else {
                    ProcessExecutionStatus::Failed
                };
                record.exit_code = captured.exit_code;
                record.stdout = captured.stdout;
                record.stderr = captured.stderr;
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

    /// Session-scoped terminal read-back (issue #4197): return the MOST RECENT
    /// terminal (highest rowid) recorded under `session_id`, regardless of which
    /// `cycle_id` recorded it, or `None` when the session has no terminal. With a
    /// stable per-goal `session_id`, this lets a later OODA tick recognise that a
    /// goal-session already reached a terminal state (and is therefore `done`)
    /// instead of perpetually re-surfacing it as blocked. Fails CLOSED on an
    /// invalid `session_id`.
    pub fn terminal_for_session(
        &self,
        session_id: &str,
    ) -> CapabilityResult<Option<TerminalOutcome>> {
        validate_identifier("session id", session_id)?;
        let connection = self.lock()?;
        let json: Option<Vec<u8>> = connection
            .query_row(
                "SELECT outcome_json FROM terminal_outcomes WHERE session_id=?1 \
                 ORDER BY rowid DESC LIMIT 1",
                [session_id],
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

    /// True iff the existing claim for `claim_key` corresponds to a live
    /// engineer. Fail-closed: with no liveness provider we cannot prove death,
    /// so the claim is treated as live and a duplicate spawn stays rejected.
    fn claim_is_live(&self, claim_key: &str) -> bool {
        match &self.engineer_liveness {
            Some(provider) => provider.is_claim_live(claim_key),
            None => true,
        }
    }

    /// Insert the `engineer_claims` row for a `SpawnEngineer` action, applying
    /// the liveness-verified reclaim gate on a `claim_key` collision.
    ///
    /// On a `PRIMARY KEY` violation the existing claim is inspected: a **live**
    /// claim keeps the `AdmissionRejected` rejection (single-active-claim
    /// invariant), while a **dead/orphaned** claim is reclaimed by deleting the
    /// stale row and retrying the insert **once, inside the same transaction**
    /// (no zero-claim window, no TOCTOU gap). The retried insert carries the new
    /// spawn's fresh `outcome_id`, so it never collides on `outcome_id UNIQUE`.
    fn insert_engineer_claim(
        &self,
        transaction: &Transaction<'_>,
        claim_key: &str,
        outcome_id: &str,
        request_id: &str,
    ) -> CapabilityResult<()> {
        let rejected = || {
            CapabilityError::new(
                CapabilityErrorCode::AdmissionRejected,
                format!("engineer claim is already active: {claim_key}"),
            )
        };
        let insert = |tx: &Transaction<'_>| {
            tx.execute(
                "INSERT INTO engineer_claims(claim_key, outcome_id, request_id) VALUES (?1, ?2, ?3)",
                params![claim_key, outcome_id, request_id],
            )
        };
        match insert(transaction) {
            Ok(_) => Ok(()),
            Err(error) if is_constraint(&error) => {
                if self.claim_is_live(claim_key) {
                    return Err(rejected());
                }
                // Stale/orphaned claim: reclaim it and retry once.
                transaction
                    .execute(
                        "DELETE FROM engineer_claims WHERE claim_key = ?1",
                        params![claim_key],
                    )
                    .map_err(persistence)?;
                insert(transaction).map_err(|retry_error| {
                    if is_constraint(&retry_error) {
                        rejected()
                    } else {
                        persistence(retry_error)
                    }
                })?;
                Ok(())
            }
            Err(error) => Err(persistence(error)),
        }
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

/// Per-stream cap on captured subprocess output. Bounds ledger row size and
/// peak memory without imposing any wall-clock timeout on the child.
const PROCESS_OUTPUT_LIMIT: usize = 1024 * 1024;

struct CapturedProcessOutput {
    success: bool,
    exit_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Run a process capturing at most [`PROCESS_OUTPUT_LIMIT`] bytes per stream.
///
/// stdout and stderr are drained concurrently on separate threads so a child
/// that fills one pipe cannot deadlock against a serial reader. There is no
/// timeout: the child runs to completion, matching the project rule against
/// wall-clock kills of working steps.
fn capture_process_output(request: &ProcessExecRequest) -> io::Result<CapturedProcessOutput> {
    let mut child = Command::new(&request.program)
        .args(&request.args)
        .current_dir(&request.working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_reader = spawn_capped_reader(stdout);
    let stderr_reader = spawn_capped_reader(stderr);

    let status = child.wait()?;
    let (stdout, stdout_truncated) = join_capped_reader(stdout_reader);
    let (stderr, stderr_truncated) = join_capped_reader(stderr_reader);

    Ok(CapturedProcessOutput {
        success: status.success(),
        exit_code: status.code(),
        stdout: mark_truncation(stdout, stdout_truncated),
        stderr: mark_truncation(stderr, stderr_truncated),
    })
}

fn spawn_capped_reader<R>(reader: Option<R>) -> std::thread::JoinHandle<(Vec<u8>, bool)>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut truncated = false;
        if let Some(mut reader) = reader {
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        // Fully drain the pipe so the child never blocks on a
                        // full buffer, but only retain up to the cap in memory.
                        if buffer.len() < PROCESS_OUTPUT_LIMIT {
                            let remaining = PROCESS_OUTPUT_LIMIT - buffer.len();
                            let keep = remaining.min(read);
                            buffer.extend_from_slice(&chunk[..keep]);
                            if keep < read {
                                truncated = true;
                            }
                        } else {
                            truncated = true;
                        }
                    }
                    Err(ref error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        }
        (buffer, truncated)
    })
}

fn join_capped_reader(handle: std::thread::JoinHandle<(Vec<u8>, bool)>) -> (Vec<u8>, bool) {
    handle.join().unwrap_or_else(|_| (Vec::new(), false))
}

fn mark_truncation(mut buffer: Vec<u8>, truncated: bool) -> Vec<u8> {
    if truncated {
        buffer.extend_from_slice(b"\n[process output truncated at 1048576 bytes]");
    }
    buffer
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

/// TDD regression suite for the engineer-claim liveness lease (issue #4094).
///
/// The append-only `engineer_claims` table had a `claim_key` PRIMARY KEY and
/// **no release path**: once a goal spawned its first engineer, every future
/// spawn for that goal was permanently rejected as "engineer claim is already
/// active", even after the engineer had terminated (success, failure, blocked,
/// crash, or zombie-reap). The row was never deleted, so a single spawn locked
/// a goal out for the lifetime of the store.
///
/// These tests pin the fix and MUST fail until Step 8 lands the implementation.
/// They are written against the intended API surface, which does not yet exist
/// (so this module is compile-red today — that is the TDD "red" contract):
///
///   * [`CapabilityHandler::release_engineer_claim`] — idempotent, fail-visible
///     `DELETE FROM engineer_claims WHERE claim_key = ?1`. This is the exact,
///     deterministic call the engineer-termination chokepoint
///     (`cleanup_engineer_worktree_for_goal`) makes on every one of its
///     terminal paths. Because the call is unconditional and idempotent,
///     covering the single chokepoint covers all termination paths.
///   * [`EngineerLiveness`] — trait injected into the handler
///     (`with_engineer_liveness`) whose `is_claim_live(claim_key)` reuses the
///     real sentinel + `is_pid_alive_public` liveness signal. Production wraps
///     `find_live_engineer_for_goal`; tests supply a deterministic double so no
///     real processes are spawned.
///   * The reclaim gate inside [`CapabilityHandler::record_action`]: on a
///     `claim_key` PK violation, a claim proven *not live* is reclaimed
///     (deleted + the new spawn admitted); a claim that *is* live still yields
///     `AdmissionRejected`.
///
/// Behaviours covered:
///   T1  release-on-termination frees a goal for re-spawn (core #4094 regression)
///   T2  a dead/orphaned claim is reclaimed, not blocking a new spawn
///   T3  a live engineer still blocks a duplicate concurrent spawn
///   T4  releasing a non-existent claim is a no-op success (idempotent)
///   T5  releasing the same claim twice is safe (all termination paths idempotent)
///   ST-1 release deletes ONLY the targeted claim_key
///   ST-2 a handler with no liveness provider is fail-closed (never blind-reclaims)
#[cfg(test)]
mod engineer_claim_lease_tests {
    use super::*;

    const REPO_OWNER: &str = "rysweet";
    const REPO_NAME: &str = "Simard";
    const POLICY_REVISION: &str = "policy-v1";

    /// Deterministic [`EngineerLiveness`] double reporting a fixed verdict for
    /// every claim key. `live == true` models a running engineer whose sentinel
    /// PID is alive; `live == false` models a dead/orphaned claim (dead sentinel
    /// PID) that must be reclaimable.
    struct FixedLiveness {
        live: bool,
    }

    impl EngineerLiveness for FixedLiveness {
        fn is_claim_live(&self, _claim_key: &str) -> bool {
            self.live
        }
    }

    fn claim_key_for(goal_id: &str) -> String {
        format!("{REPO_OWNER}/{REPO_NAME}:{goal_id}")
    }

    fn open_handler(dir: &std::path::Path) -> CapabilityHandler {
        CapabilityHandler::open(
            dir.join("outcomes.sqlite3"),
            CapabilityPolicy::new(POLICY_REVISION),
        )
        .expect("open capability handler")
    }

    fn open_handler_with_liveness(dir: &std::path::Path, live: bool) -> CapabilityHandler {
        open_handler(dir).with_engineer_liveness(Box::new(FixedLiveness { live }))
    }

    fn spawn_actor(session_id: &str, cycle_id: &str, goal_id: &str) -> AuthenticatedToolContext {
        AuthenticatedToolContext::new(
            "goal-session-actor",
            session_id,
            [CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)],
        )
        .scoped_to_repository(RepositoryRef::new(REPO_OWNER, REPO_NAME))
        .bound_to_cycle_goal(cycle_id, goal_id)
        .with_engineer_permissions(["repo_read"])
    }

    fn spawn_request(
        request_id: &str,
        session_id: &str,
        cycle_id: &str,
        goal_id: &str,
    ) -> RecordActionRequest {
        RecordActionRequest {
            identity: TerminalRequestIdentity::new(request_id, session_id, cycle_id, goal_id),
            action: Action::SpawnEngineer(SpawnEngineerAction {
                task: OpaqueBytes::from(b"advance the goal".to_vec()),
                repository: RepositoryRef::new(REPO_OWNER, REPO_NAME),
                base_type: BaseType::Copilot,
                requested_permissions: ["repo_read".to_string()].into_iter().collect(),
                // claim_key is `owner/repo:goal_id` — intentionally STABLE across
                // spawns of the same goal. That stability is exactly what the
                // leaked PK weaponized into a permanent lock.
                claim_key: claim_key_for(goal_id),
            }),
            raw_semantic: OpaqueBytes::from(b"spawn engineer".to_vec()),
            evidence: Vec::new(),
        }
    }

    fn admission() -> AdmissionSnapshot {
        AdmissionSnapshot {
            concurrent_engineers: 0,
            disk_used_percent: 1,
            active_claims: BTreeSet::new(),
            policy_revision: POLICY_REVISION.to_string(),
        }
    }

    /// Record a `SpawnEngineer` terminal outcome for `goal_id`. Every attempt
    /// uses a unique (session, cycle, request) triple because
    /// `terminal_outcomes` enforces `UNIQUE(session_id, cycle_id)` and a
    /// `request_id` PRIMARY KEY, so a genuine re-spawn always lands on a fresh
    /// cycle — while the `claim_key` stays constant for the goal.
    fn spawn_engineer(
        handler: &CapabilityHandler,
        attempt: &str,
        goal_id: &str,
    ) -> CapabilityResult<TerminalOutcome> {
        let session_id = format!("session-{goal_id}-{attempt}");
        let cycle_id = format!("cycle-{goal_id}-{attempt}");
        let request_id = format!("request-{goal_id}-{attempt}");
        let actor = spawn_actor(&session_id, &cycle_id, goal_id);
        handler.record_action(
            &actor,
            spawn_request(&request_id, &session_id, &cycle_id, goal_id),
            &admission(),
        )
    }

    /// T1 — CORE REGRESSION for issue #4094.
    ///
    /// Spawn an engineer, observe the (correct) single-active rejection while it
    /// holds the claim, then RELEASE the claim on termination and confirm the
    /// same goal can be spawned again. Before the fix this final spawn hit the
    /// append-only PK and was rejected forever.
    #[test]
    fn spawning_again_after_release_succeeds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler(dir.path());
        let goal = "goal-alpha";

        // First engineer spawns and takes the claim.
        spawn_engineer(&handler, "0", goal).expect("first spawn admitted");

        // While that engineer is (treated as) live, a duplicate is rejected.
        let blocked = spawn_engineer(&handler, "1", goal)
            .expect_err("duplicate spawn must be rejected while the claim is held");
        assert_eq!(blocked.code(), CapabilityErrorCode::AdmissionRejected);

        // The engineer terminates: its lifecycle releases the claim. This is the
        // exact deterministic call `cleanup_engineer_worktree_for_goal` makes.
        handler
            .release_engineer_claim(&claim_key_for(goal))
            .expect("release must succeed");

        // A fresh engineer for the SAME goal is now admitted — leak fixed.
        spawn_engineer(&handler, "2", goal)
            .expect("spawn after release must be admitted (claim was released)");
    }

    /// T2 — stale-claim reclaim: a dead engineer must not block a new spawn.
    ///
    /// The claim row still exists (no release ran — crash/zombie), but liveness
    /// reports the engineer as dead, so the next spawn reclaims the orphaned row
    /// instead of rejecting. This models the 31 orphaned claims from the live
    /// incident that permanently locked out every active goal.
    #[test]
    fn dead_claim_does_not_block_new_spawn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler_with_liveness(dir.path(), false);
        let goal = "goal-orphaned";

        spawn_engineer(&handler, "0", goal).expect("first spawn admitted");
        spawn_engineer(&handler, "1", goal)
            .expect("dead claim must be reclaimed, not block a new spawn");
    }

    /// T3 — single-active-claim preserved: a live engineer still blocks a
    /// duplicate concurrent spawn, so we never run duplicate work on a goal.
    #[test]
    fn live_claim_still_blocks_duplicate_spawn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler_with_liveness(dir.path(), true);
        let goal = "goal-busy";

        spawn_engineer(&handler, "0", goal).expect("first spawn admitted");
        let blocked = spawn_engineer(&handler, "1", goal)
            .expect_err("live engineer must still block a duplicate spawn");
        assert_eq!(blocked.code(), CapabilityErrorCode::AdmissionRejected);
    }

    /// T4 — release is idempotent when the claim does not exist (0 rows == Ok).
    #[test]
    fn release_is_idempotent_when_no_claim_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler(dir.path());
        handler
            .release_engineer_claim(&claim_key_for("never-spawned"))
            .expect("releasing a non-existent claim is a no-op success");
    }

    /// T5 — releasing the same claim twice is safe. Termination paths may race
    /// or re-run (e.g. reap after a clean exit already released it); a second
    /// release must not error, and the goal remains spawnable.
    #[test]
    fn releasing_the_same_claim_twice_is_ok() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler(dir.path());
        let goal = "goal-double-release";
        spawn_engineer(&handler, "0", goal).expect("first spawn admitted");

        let key = claim_key_for(goal);
        handler
            .release_engineer_claim(&key)
            .expect("first release ok");
        handler
            .release_engineer_claim(&key)
            .expect("second release is still a no-op success");

        spawn_engineer(&handler, "1", goal).expect("spawn after double release admitted");
    }

    /// ST-1 — release deletes ONLY the targeted `claim_key` (no key confusion).
    ///
    /// Two goals hold live claims; releasing goal A must not disturb goal B.
    #[test]
    fn release_only_deletes_the_targeted_claim() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler_with_liveness(dir.path(), true);
        let goal_a = "goal-a";
        let goal_b = "goal-b";

        spawn_engineer(&handler, "0", goal_a).expect("spawn A admitted");
        spawn_engineer(&handler, "0", goal_b).expect("spawn B admitted");

        handler
            .release_engineer_claim(&claim_key_for(goal_a))
            .expect("release A ok");

        // A was released → re-spawn admitted.
        spawn_engineer(&handler, "1", goal_a).expect("A re-spawn admitted after release");
        // B was untouched and is still live → duplicate rejected.
        let blocked = spawn_engineer(&handler, "1", goal_b)
            .expect_err("B claim must be untouched by A's release");
        assert_eq!(blocked.code(), CapabilityErrorCode::AdmissionRejected);
    }

    /// ST-2 — a handler with no liveness provider is FAIL-CLOSED.
    ///
    /// Without a liveness signal the handler must NOT silently reclaim a claim
    /// it cannot prove is dead; it treats an existing claim as live and keeps
    /// rejecting duplicates. This preserves the single-active guarantee and
    /// guards against a default that blind-reclaims and reintroduces duplicate
    /// engineers.
    #[test]
    fn default_handler_without_liveness_is_fail_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler(dir.path());
        let goal = "goal-failclosed";

        spawn_engineer(&handler, "0", goal).expect("first spawn admitted");
        let blocked = spawn_engineer(&handler, "1", goal)
            .expect_err("without a liveness signal the claim must be treated as live");
        assert_eq!(blocked.code(), CapabilityErrorCode::AdmissionRejected);
    }

    /// RP-1 (issue #4099) — `list_engineer_claims` reflects the real ledger and
    /// `release_engineer_claim` (the shared chokepoint the reaper reuses) removes
    /// the row. Proves the reaper never needs hand-rolled SQL: list to find the
    /// leak, release to reclaim it, list again to confirm it is gone.
    #[test]
    fn list_engineer_claims_reflects_ledger_and_release_removes_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler_with_liveness(dir.path(), true);

        // Empty ledger to start.
        assert!(
            handler
                .list_engineer_claims()
                .expect("list on empty ledger")
                .is_empty(),
            "a fresh ledger holds no engineer claims"
        );

        // Two goals across (conceptually) different repos hold claims.
        spawn_engineer(&handler, "0", "g1").expect("spawn g1 admitted");
        spawn_engineer(&handler, "0", "g2").expect("spawn g2 admitted");

        let mut listed = handler
            .list_engineer_claims()
            .expect("list with two claims");
        listed.sort();
        assert_eq!(
            listed,
            vec![claim_key_for("g1"), claim_key_for("g2")],
            "list_engineer_claims must return every held claim_key"
        );

        // Reclaim g1 through the shared release path (what the reaper calls).
        handler
            .release_engineer_claim(&claim_key_for("g1"))
            .expect("release g1");

        assert_eq!(
            handler.list_engineer_claims().expect("list after release"),
            vec![claim_key_for("g2")],
            "released claim must be gone; the untouched claim remains"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TDD (issue #4197): SESSION-SCOPED terminal read-back.
//
// With a stable per-goal `session_id` (see `derive_session_id`), the ledger
// needs a session-scoped read path so a later tick can recognise that a
// goal-session already reached a terminal state (and is therefore `done`),
// without knowing which specific `cycle_id` recorded it.
//
// Fix contract (additive; existing `terminal_for_cycle` / `terminal_for_request`
// are unchanged):
//
//   CapabilityHandler::terminal_for_session(&self, session_id: &str)
//       -> CapabilityResult<Option<TerminalOutcome>>
//
//   * Returns `Some(outcome)` if ANY terminal exists for `session_id`, choosing
//     the MOST RECENT one (highest rowid) when several cycles recorded terminals
//     under the same session.
//   * Returns `None` when no terminal exists for the session.
//   * Rejects an invalid `session_id` via `validate_identifier` (fail-closed).
//
// These tests are written FIRST and MUST FAIL until `terminal_for_session`
// exists.
#[cfg(test)]
mod terminal_for_session_tests {
    use super::*;

    const REPO_OWNER: &str = "rysweet";
    const REPO_NAME: &str = "Simard";
    const POLICY_REVISION: &str = "policy-v1";

    fn open_handler(dir: &std::path::Path) -> CapabilityHandler {
        CapabilityHandler::open(
            dir.join("outcomes.sqlite3"),
            CapabilityPolicy::new(POLICY_REVISION),
        )
        .expect("open capability handler")
    }

    fn admission() -> AdmissionSnapshot {
        AdmissionSnapshot {
            concurrent_engineers: 0,
            disk_used_percent: 1,
            active_claims: BTreeSet::new(),
            policy_revision: POLICY_REVISION.to_string(),
        }
    }

    /// Record one `SpawnEngineer` terminal outcome under an explicit
    /// (session, cycle, request) triple. Mirrors the production goal-session
    /// path: a stable `session_id` with a per-cycle `cycle_id`.
    fn record_terminal(
        handler: &CapabilityHandler,
        session_id: &str,
        cycle_id: &str,
        request_id: &str,
        goal_id: &str,
    ) -> TerminalOutcome {
        let claim_key = format!("{REPO_OWNER}/{REPO_NAME}:{goal_id}");
        let actor = AuthenticatedToolContext::new(
            "goal-session-actor",
            session_id,
            [CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)],
        )
        .scoped_to_repository(RepositoryRef::new(REPO_OWNER, REPO_NAME))
        .bound_to_cycle_goal(cycle_id, goal_id)
        .with_engineer_permissions(["repo_read"]);
        let request = RecordActionRequest {
            identity: TerminalRequestIdentity::new(request_id, session_id, cycle_id, goal_id),
            action: Action::SpawnEngineer(SpawnEngineerAction {
                task: OpaqueBytes::from(b"advance the goal".to_vec()),
                repository: RepositoryRef::new(REPO_OWNER, REPO_NAME),
                base_type: BaseType::Copilot,
                requested_permissions: ["repo_read".to_string()].into_iter().collect(),
                claim_key: claim_key.clone(),
            }),
            raw_semantic: OpaqueBytes::from(b"spawn engineer".to_vec()),
            evidence: Vec::new(),
        };
        let outcome = handler
            .record_action(&actor, request, &admission())
            .expect("record_action must record a terminal");
        // Release the per-goal engineer claim so the NEXT tick can spawn again —
        // mirrors the real cross-tick lifecycle, where the prior engineer's claim
        // is released before the goal is re-advanced under the same session id.
        handler
            .release_engineer_claim(&claim_key)
            .expect("release engineer claim between ticks");
        outcome
    }

    /// CORE REGRESSION for #4197: a terminal recorded under a stable session id
    /// is readable back by session, so a later tick can see the goal is done.
    #[test]
    fn terminal_for_session_reads_back_recorded_terminal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler(dir.path());
        let session = "ooda-goal-alpha";

        record_terminal(&handler, session, "cycle-0", "request-0", "goal-alpha");

        let found = handler
            .terminal_for_session(session)
            .expect("terminal_for_session query must succeed");
        let outcome = found.expect("a terminal recorded under this session must read back");
        assert_eq!(outcome.session_id, session);
    }

    /// A session with no recorded terminal reads back as `None` (still blocked).
    #[test]
    fn terminal_for_session_returns_none_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler(dir.path());

        let found = handler
            .terminal_for_session("ooda-never-recorded")
            .expect("query must succeed");
        assert!(found.is_none(), "no terminal recorded -> None");
    }

    /// CROSS-TICK: terminals from multiple cycles share one stable session id;
    /// the session-scoped read returns the MOST RECENT terminal.
    #[test]
    fn terminal_for_session_returns_latest_across_cycles() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler(dir.path());
        let session = "ooda-goal-beta";

        record_terminal(&handler, session, "cycle-0", "request-0", "goal-beta");
        record_terminal(&handler, session, "cycle-1", "request-1", "goal-beta");

        let outcome = handler
            .terminal_for_session(session)
            .expect("query must succeed")
            .expect("session has terminals");
        assert_eq!(outcome.session_id, session);
        assert_eq!(
            outcome.cycle_id, "cycle-1",
            "session-scoped read must return the most recent cycle's terminal"
        );
    }

    /// Fail-closed on a malformed session id.
    #[test]
    fn terminal_for_session_rejects_invalid_identifier() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler(dir.path());

        assert!(
            handler.terminal_for_session("bad id!").is_err(),
            "an invalid session identifier must be rejected"
        );
    }
}

/// Regression tests for issue #4197 — the typed-OODA actor-session
/// identity-binding false violation.
///
/// A PERPETUAL/STANDING goal re-leases the SAME stable per-goal `session_id`
/// (`ooda-<sha256(goal_id)[..16]>`) on every cycle, while the per-cycle
/// `cycle_id` advances each tick. With a 30-day lease the prior row never
/// expires between the ~7-minute cycle retries, so `register_actor_session`
/// loads the still-live binding and — before this fix — compared the WHOLE
/// binding (including the mutable per-cycle `cycle_id`). The stored binding
/// differed only in `cycle_id`, so every re-lease raised a false
/// `AuthorizationScopeViolation` ("actor session is already bound to a
/// different identity or authorization scope"), crash-looping the goal forever
/// while the board still showed it "not-started".
///
/// The fix narrows the violation guard to compare ONLY the identity /
/// authorization-scope fields (actor_identity, repository, grants,
/// engineer_permissions, working_directory, observe_only). A changed per-cycle
/// `cycle_id` (and the 1:1 `goal_id`) is legitimate lease metadata refreshed by
/// the existing `ON CONFLICT(session_id) DO UPDATE` upsert, and MUST NOT be a
/// violation. A genuine change to any of the six scope fields on a reused
/// `session_id` MUST still be rejected.
#[cfg(test)]
mod actor_session_scope_tests {
    use super::*;

    const REPO_OWNER: &str = "rysweet";
    const REPO_NAME: &str = "Simard";
    const POLICY_REVISION: &str = "policy-v1";

    /// Mirror production: a 30-day actor-session lease (`route.rs`), which
    /// never expires between the short cycle retries that trip the bug.
    const LEASE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

    fn open_handler(dir: &std::path::Path) -> CapabilityHandler {
        CapabilityHandler::open(
            dir.join("outcomes.sqlite3"),
            CapabilityPolicy::new(POLICY_REVISION),
        )
        .expect("open capability handler")
    }

    /// A goal-session actor mirroring `typed_goal_session.rs`: a STABLE
    /// per-goal `session_id` with a PER-CYCLE `cycle_id`, holding one fixed
    /// identity + authorization scope.
    fn goal_actor(session_id: &str, cycle_id: &str, goal_id: &str) -> AuthenticatedToolContext {
        AuthenticatedToolContext::new(
            "goal-session-actor",
            session_id,
            [CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)],
        )
        .scoped_to_repository(RepositoryRef::new(REPO_OWNER, REPO_NAME))
        .bound_to_cycle_goal(cycle_id, goal_id)
        .with_engineer_permissions(["repo_read"])
    }

    /// Read the persisted `actor_sessions` row straight from the ledger so a
    /// test can assert the mutable lease metadata was refreshed by the upsert.
    fn stored_binding(handler: &CapabilityHandler, session_id: &str) -> ActorBinding {
        let connection = handler.lock().expect("lock ledger");
        load_actor_binding(&connection, session_id)
            .expect("query actor binding")
            .expect("a binding must be persisted for this session")
    }

    /// Register `cycle-1`, then attempt to re-lease the SAME `session_id` under
    /// `cycle-2` with `mutated` (a changed identity/authorization scope) and
    /// assert it is rejected as an `AuthorizationScopeViolation`.
    fn assert_scope_change_rejected(mutated: AuthenticatedToolContext, what: &str) {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler(dir.path());
        let session = "ooda-stable-session";
        let goal = "goal-perpetual";

        handler
            .register_actor_session(
                &goal_actor(session, "cycle-1", goal),
                "request-cycle-1",
                "cycle-1",
                goal,
                LEASE,
            )
            .expect("baseline lease of the stable session id must succeed");

        let err = handler
            .register_actor_session(&mutated, "request-cycle-2", "cycle-2", goal, LEASE)
            .expect_err("a changed identity/authorization scope must be rejected, not accepted");
        assert_eq!(
            err.code(),
            CapabilityErrorCode::AuthorizationScopeViolation,
            "a changed {what} on a reused session id must be a scope violation"
        );
    }

    /// TEST 1 — CORE REGRESSION.
    ///
    /// Two `register_actor_session` calls, SAME `session_id` + SAME
    /// identity/scope, DIFFERENT per-cycle `cycle_id`: both must succeed and the
    /// persisted row's mutable metadata (`cycle_id`/`goal_id`/token) must be
    /// refreshed to the new cycle.
    #[test]
    fn re_leasing_same_session_with_new_cycle_refreshes_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler(dir.path());
        let session = "ooda-stable-session";
        let goal = "goal-perpetual";

        // Cycle 1: first lease of the stable session id.
        let lease1 = handler
            .register_actor_session(
                &goal_actor(session, "cycle-1", goal),
                "request-cycle-1",
                "cycle-1",
                goal,
                LEASE,
            )
            .expect("first cycle must lease the stable session id");

        // Cycle 2 (~7 min later, the 30-day lease is NOT expired): SAME identity
        // and scope, only the per-cycle cycle_id advances. This is a legitimate
        // re-lease and MUST succeed rather than raise a false violation.
        let lease2 = handler
            .register_actor_session(
                &goal_actor(session, "cycle-2", goal),
                "request-cycle-2",
                "cycle-2",
                goal,
                LEASE,
            )
            .expect("re-leasing the same session for a new cycle must succeed");

        // Each re-lease mints a fresh token (token rotation preserved).
        assert_ne!(
            lease1.token, lease2.token,
            "each re-lease must mint a fresh token"
        );

        // The persisted row's mutable lease metadata is refreshed to cycle 2.
        let binding = stored_binding(&handler, session);
        assert_eq!(
            binding.cycle_id, "cycle-2",
            "the persisted cycle_id must be refreshed to the new cycle"
        );
        assert_eq!(
            binding.goal_id, goal,
            "the persisted goal_id must be refreshed via the upsert"
        );

        // The refreshed lease authenticates against the NEW cycle...
        handler
            .authenticate_actor_session(&lease2.token, session, "cycle-2", goal)
            .expect("the cycle-2 token must authenticate against cycle-2");
        // ...and the superseded cycle no longer authenticates.
        assert!(
            handler
                .authenticate_actor_session(&lease2.token, session, "cycle-1", goal)
                .is_err(),
            "the superseded cycle must no longer authenticate"
        );
    }

    /// TEST 2a — a genuinely changed `actor_identity` on the SAME `session_id`
    /// must still be rejected as an `AuthorizationScopeViolation`.
    #[test]
    fn changed_actor_identity_on_reused_session_is_rejected() {
        let mutated = AuthenticatedToolContext::new(
            "intruder-identity",
            "ooda-stable-session",
            [CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)],
        )
        .scoped_to_repository(RepositoryRef::new(REPO_OWNER, REPO_NAME))
        .bound_to_cycle_goal("cycle-2", "goal-perpetual")
        .with_engineer_permissions(["repo_read"]);
        assert_scope_change_rejected(mutated, "actor_identity");
    }

    /// TEST 2b — a changed bound `repository` (even one still inside policy) on
    /// the SAME `session_id` must still be rejected.
    #[test]
    fn changed_repository_on_reused_session_is_rejected() {
        let mutated = AuthenticatedToolContext::new(
            "goal-session-actor",
            "ooda-stable-session",
            [CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)],
        )
        // A different repo under the same policy-allowed owner: governance still
        // permits it, but it is a DIFFERENT authorization scope.
        .scoped_to_repository(RepositoryRef::new(REPO_OWNER, "OtherRepo"))
        .bound_to_cycle_goal("cycle-2", "goal-perpetual")
        .with_engineer_permissions(["repo_read"]);
        assert_scope_change_rejected(mutated, "repository");
    }

    /// TEST 2c — ESCALATED grants on the SAME `session_id` must still be
    /// rejected (no silent privilege widening).
    #[test]
    fn escalated_grants_on_reused_session_is_rejected() {
        let mutated = AuthenticatedToolContext::new(
            "goal-session-actor",
            "ooda-stable-session",
            [
                CapabilityGrant::RecordAction(ActionKind::SpawnEngineer),
                // A new, more powerful grant the baseline lease never held.
                CapabilityGrant::DirectMerge,
            ],
        )
        .scoped_to_repository(RepositoryRef::new(REPO_OWNER, REPO_NAME))
        .bound_to_cycle_goal("cycle-2", "goal-perpetual")
        .with_engineer_permissions(["repo_read"]);
        assert_scope_change_rejected(mutated, "grants");
    }

    /// TEST 2d — flipping `observe_only` on the SAME `session_id` must still be
    /// rejected (an observe-only lease cannot silently gain mutation scope, nor
    /// vice-versa).
    #[test]
    fn changed_observe_only_on_reused_session_is_rejected() {
        let mutated =
            goal_actor("ooda-stable-session", "cycle-2", "goal-perpetual").with_observe_only(true);
        assert_scope_change_rejected(mutated, "observe_only");
    }

    /// TEST 3 — a PERPETUAL/STANDING goal re-entering the typed goal-session
    /// across two consecutive cycles no longer fails with the identity-binding
    /// error. This is the end-to-end shape of the crash-loop from #4197.
    #[test]
    fn perpetual_goal_reentry_across_consecutive_cycles_succeeds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler(dir.path());
        // A stable per-goal session id (#4197) shared across EVERY cycle of a
        // perpetual goal; the per-cycle cycle_id mirrors the production format
        // `cycle-<n>-<goal_id>`.
        let session = "ooda-continuously-research";
        let goal = "continuously-research-and-improve-your-own-cogn-70ab8541";

        for cycle in 1..=2 {
            let cycle_id = format!("cycle-{cycle}-{goal}");
            let request_id = format!("request-{cycle}");
            handler
                .register_actor_session(
                    &goal_actor(session, &cycle_id, goal),
                    &request_id,
                    &cycle_id,
                    goal,
                    LEASE,
                )
                .unwrap_or_else(|err| {
                    panic!(
                        "perpetual goal cycle {cycle} must re-lease the stable session id \
                         without an identity-binding violation, got: {err:?}"
                    )
                });
        }
    }
}

/// Issue #4483 — typed-OODA outcome-persistence "database is locked" crash-loop
/// (RED phase, TDD Step 7). These tests specify the connection-tuning contract in
/// `docs/reference/typed-ooda-ledger-concurrency.md`:
///
///   1. WAL journal mode is applied UNCONDITIONALLY at every `open()`, so a
///      pre-existing v1 database (created before WAL, or whose journal was
///      reverted) still gets WAL — not only during the one-time schema
///      migration branch (`schema.rs`). Without this, two OS processes sharing
///      the ledger serialize on a whole-file lock and a writer that cannot
///      acquire it within the busy timeout fails with `database is locked`.
///   2. A generous busy timeout (>= 30s) is set at open so a briefly-contended
///      writer waits and retries instead of erroring out.
///   3. Concurrent writers on SEPARATE connections to the same file never
///      surface a `database is locked` persistence error.
///
/// They MUST fail before the fix lands (WAL-in-migration-only, 5s timeout) and
/// MUST pass once the fix lands without further test edits.
#[cfg(test)]
mod ledger_concurrency_tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    const POLICY_REVISION: &str = "policy-v1";

    fn open_handler(path: &std::path::Path) -> CapabilityHandler {
        CapabilityHandler::open(path, CapabilityPolicy::new(POLICY_REVISION))
            .expect("open capability handler")
    }

    /// Read the live `PRAGMA journal_mode` of the handler's own connection.
    fn journal_mode(handler: &CapabilityHandler) -> String {
        let connection = handler.lock().expect("lock ledger");
        connection
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .expect("query journal_mode")
    }

    /// Read the live `PRAGMA busy_timeout` (milliseconds) of the handler's own
    /// connection.
    fn busy_timeout_millis(handler: &CapabilityHandler) -> i64 {
        let connection = handler.lock().expect("lock ledger");
        connection
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .expect("query busy_timeout")
    }

    /// A1 (issue #4483): opening a PRE-EXISTING v1 ledger whose journal mode is
    /// NOT WAL must still leave the database in WAL mode. This is the exact
    /// crash-loop trigger: the WAL pragma lives only inside the schema-migration
    /// branch, which is skipped once `user_version == 1`, so a database created
    /// before WAL (or reverted to rollback journaling) opens WITHOUT the
    /// concurrency mode that lets a reader and a writer coexist.
    #[test]
    fn open_applies_wal_journal_mode_on_preexisting_v1_db() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("outcomes.sqlite3");

        // First open creates the schema at v1. Drop it so nothing else holds the
        // file, then forcibly revert the journal to rollback mode to emulate a
        // ledger created before WAL was introduced.
        drop(open_handler(&path));
        {
            let raw = rusqlite::Connection::open(&path).expect("raw open");
            let mode: String = raw
                .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
                .expect("revert journal mode");
            assert_eq!(
                mode.to_ascii_lowercase(),
                "delete",
                "precondition: journal must be reverted to rollback mode"
            );
        }

        // Re-open the now-pre-existing v1 database. The open path MUST re-assert
        // WAL unconditionally.
        let handler = open_handler(&path);
        assert_eq!(
            journal_mode(&handler).to_ascii_lowercase(),
            "wal",
            "open() must apply WAL journal mode even on a pre-existing v1 database"
        );
    }

    /// A generous busy timeout (>= 30s) must be configured at open so a briefly
    /// contended writer waits-and-retries instead of failing with
    /// `database is locked`.
    #[test]
    fn open_sets_busy_timeout_at_least_30s() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handler = open_handler(&dir.path().join("outcomes.sqlite3"));
        let timeout = busy_timeout_millis(&handler);
        assert!(
            timeout >= 30_000,
            "busy_timeout must be >= 30000ms to absorb cross-process contention, got {timeout}ms"
        );
    }

    /// Regression: concurrent writers on SEPARATE connections to the same ledger
    /// file must never surface a `database is locked` persistence error. Each
    /// thread opens its OWN `CapabilityHandler` (a distinct SQLite connection,
    /// modelling the daemon + engineer-worktree + reaper processes) and hammers a
    /// real write path (`release_engineer_claim`, an idempotent immediate-txn
    /// DELETE) against a shared, pre-existing database.
    #[test]
    fn concurrent_cross_connection_writers_never_hit_database_locked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("outcomes.sqlite3");
        // Materialise the schema once so every worker opens a pre-existing DB.
        drop(open_handler(&path));

        const WRITERS: usize = 6;
        const ITERATIONS: usize = 40;
        let barrier = Arc::new(Barrier::new(WRITERS));

        let mut handles = Vec::with_capacity(WRITERS);
        for writer in 0..WRITERS {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                let handler = open_handler(&path);
                barrier.wait();
                for i in 0..ITERATIONS {
                    let claim_key = format!("rysweet/Simard:goal-{writer}-{i}");
                    if let Err(err) = handler.release_engineer_claim(&claim_key) {
                        return Err(err.to_string());
                    }
                }
                Ok(())
            }));
        }

        for handle in handles {
            let result = handle.join().expect("writer thread must not panic");
            if let Err(message) = result {
                assert!(
                    !message.to_ascii_lowercase().contains("database is locked"),
                    "concurrent cross-connection writers must not hit a lock error: {message}"
                );
                panic!("concurrent writer failed: {message}");
            }
        }
    }
}
