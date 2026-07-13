use std::collections::BTreeSet;
use std::fmt::{self, Display, Formatter};

use base64::Engine;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OpaqueBytes(Vec<u8>);

impl OpaqueBytes {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for OpaqueBytes {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

impl Serialize for OpaqueBytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Wire<'a> {
            encoding: &'static str,
            data: &'a str,
        }

        let data = base64::engine::general_purpose::STANDARD.encode(&self.0);
        Wire {
            encoding: "base64",
            data: &data,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OpaqueBytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            encoding: String,
            data: String,
        }

        let wire = Wire::deserialize(deserializer)?;
        if wire.encoding != "base64" {
            return Err(serde::de::Error::custom("encoding must be base64"));
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&wire.data)
            .map_err(serde::de::Error::custom)?;
        if base64::engine::general_purpose::STANDARD.encode(&decoded) != wire.data {
            return Err(serde::de::Error::custom(
                "base64 data must use canonical padded encoding",
            ));
        }
        Ok(Self(decoded))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct RepositoryRef {
    pub owner: String,
    pub name: String,
}

impl RepositoryRef {
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaseType {
    Copilot,
    RustyClawd,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    SpawnEngineer,
    FileIssue,
    RequestMerge,
    RequestDeploy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpawnEngineerAction {
    pub task: OpaqueBytes,
    pub repository: RepositoryRef,
    pub base_type: BaseType,
    pub requested_permissions: BTreeSet<String>,
    pub claim_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileIssueAction {
    pub repository: RepositoryRef,
    pub title: OpaqueBytes,
    pub body: OpaqueBytes,
    pub labels: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PullRequestRef {
    pub repository: RepositoryRef,
    pub number: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RequestMergeAction {
    pub pull_request: PullRequestRef,
    pub expected_head_sha: String,
    pub strategy: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactRef {
    pub digest: String,
    pub source_commit: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentRef {
    pub name: String,
}

impl EnvironmentRef {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupPolicy {
    VerifiedFull,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RequestDeployAction {
    pub artifact: ArtifactRef,
    pub environment: EnvironmentRef,
    pub backup_policy: BackupPolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    SpawnEngineer(SpawnEngineerAction),
    FileIssue(FileIssueAction),
    RequestMerge(RequestMergeAction),
    RequestDeploy(RequestDeployAction),
}

impl Action {
    pub fn kind(&self) -> ActionKind {
        match self {
            Self::SpawnEngineer(_) => ActionKind::SpawnEngineer,
            Self::FileIssue(_) => ActionKind::FileIssue,
            Self::RequestMerge(_) => ActionKind::RequestMerge,
            Self::RequestDeploy(_) => ActionKind::RequestDeploy,
        }
    }

    pub fn as_spawn_engineer(&self) -> Option<&SpawnEngineerAction> {
        match self {
            Self::SpawnEngineer(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceRef {
    Commit {
        repository: RepositoryRef,
        sha: String,
    },
    CheckRun {
        repository: RepositoryRef,
        check_id: u64,
        conclusion: String,
    },
    Issue {
        repository: RepositoryRef,
        number: u64,
    },
    EngineerRun {
        session_id: String,
        claim_key: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BlockerRef {
    Goal { goal_id: String },
    Credential { name: String },
    Authorization { capability: String },
    Resource { resource: String },
    Operator { identity: String },
    External { provider: String, reference: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RetryPolicy {
    Never,
    AfterGoal { goal_id: String },
    AfterSignal { provider: String, signal_id: String },
    AfterTime { unix_millis: i64 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompletionRef {
    pub criterion_id: String,
    pub verification_evidence: Vec<EvidenceRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalRequestIdentity {
    pub request_id: String,
    pub session_id: String,
    pub cycle_id: String,
    pub goal_id: String,
}

impl TerminalRequestIdentity {
    pub fn new(
        request_id: impl Into<String>,
        session_id: impl Into<String>,
        cycle_id: impl Into<String>,
        goal_id: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            session_id: session_id.into(),
            cycle_id: cycle_id.into(),
            goal_id: goal_id.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordActionRequest {
    pub identity: TerminalRequestIdentity,
    pub action: Action,
    pub raw_semantic: OpaqueBytes,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordNoActionRequest {
    pub identity: TerminalRequestIdentity,
    pub reason: OpaqueBytes,
    pub raw_semantic: OpaqueBytes,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordBlockedRequest {
    pub identity: TerminalRequestIdentity,
    pub reason: OpaqueBytes,
    pub blocker: BlockerRef,
    pub retry: RetryPolicy,
    pub raw_semantic: OpaqueBytes,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordCompletedRequest {
    pub identity: TerminalRequestIdentity,
    pub summary: OpaqueBytes,
    pub completion: CompletionRef,
    pub raw_semantic: OpaqueBytes,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordProgressRequest {
    pub identity: TerminalRequestIdentity,
    pub percent: u8,
    pub summary: OpaqueBytes,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AdmissionSnapshot {
    pub concurrent_engineers: usize,
    pub disk_used_percent: u8,
    pub active_claims: BTreeSet<String>,
    pub policy_revision: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AdmissionDecision {
    pub policy_revision: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalKind {
    Action,
    NoAction,
    Blocked,
    Completed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActionOutcomePayload {
    pub action: Action,
    pub admission: AdmissionDecision,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NoActionOutcomePayload {
    pub reason: OpaqueBytes,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BlockedOutcomePayload {
    pub reason: OpaqueBytes,
    pub blocker: BlockerRef,
    pub retry: RetryPolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompletedOutcomePayload {
    pub summary: OpaqueBytes,
    pub completion: CompletionRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypedOutcomePayload {
    Action(ActionOutcomePayload),
    NoAction(NoActionOutcomePayload),
    Blocked(BlockedOutcomePayload),
    Completed(CompletedOutcomePayload),
}

impl TypedOutcomePayload {
    pub fn action(&self) -> Option<&Action> {
        match self {
            Self::Action(value) => Some(&value.action),
            _ => None,
        }
    }

    pub fn no_action(&self) -> Option<&NoActionOutcomePayload> {
        match self {
            Self::NoAction(value) => Some(value),
            _ => None,
        }
    }

    pub fn blocked(&self) -> Option<&BlockedOutcomePayload> {
        match self {
            Self::Blocked(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalOutcome {
    pub outcome_id: String,
    pub request_id: String,
    pub session_id: String,
    pub actor_identity: String,
    pub goal_id: String,
    pub cycle_id: String,
    pub kind: TerminalKind,
    pub payload: TypedOutcomePayload,
    pub raw_semantic: OpaqueBytes,
    pub evidence: Vec<EvidenceRef>,
    pub recorded_at_unix_millis: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProgressRecord {
    pub progress_id: String,
    pub request_id: String,
    pub session_id: String,
    pub actor_identity: String,
    pub goal_id: String,
    pub cycle_id: String,
    pub percent: u8,
    pub summary: OpaqueBytes,
    pub evidence: Vec<EvidenceRef>,
    pub recorded_at_unix_millis: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityErrorCode {
    InvalidArgument,
    PayloadTooLarge,
    Unauthenticated,
    PermissionDenied,
    AdmissionRejected,
    StateTransitionRejected,
    IdempotencyConflict,
    TerminalAlreadyRecorded,
    PersistenceFailed,
}

#[derive(Debug)]
pub struct CapabilityError {
    code: CapabilityErrorCode,
    message: String,
}

impl CapabilityError {
    pub(crate) fn new(code: CapabilityErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> CapabilityErrorCode {
        self.code
    }
}

impl Display for CapabilityError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CapabilityError {}

pub type CapabilityResult<T> = Result<T, CapabilityError>;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CapabilityGrant {
    RecordAction(ActionKind),
    RecordNoAction,
    RecordBlocked,
    RecordCompleted,
    RecordProgress,
    DirectMerge,
    DirectDeploy,
}

#[derive(Clone, Debug)]
pub struct AuthenticatedToolContext {
    pub actor_identity: String,
    pub session_id: String,
    grants: BTreeSet<CapabilityGrant>,
}

impl AuthenticatedToolContext {
    pub fn new(
        actor_identity: impl Into<String>,
        session_id: impl Into<String>,
        grants: impl IntoIterator<Item = CapabilityGrant>,
    ) -> Self {
        Self {
            actor_identity: actor_identity.into(),
            session_id: session_id.into(),
            grants: grants.into_iter().collect(),
        }
    }

    pub(crate) fn allows(&self, grant: CapabilityGrant) -> bool {
        self.grants.contains(&grant)
    }
}

#[derive(Clone, Debug)]
pub struct CapabilityPolicy {
    pub revision: String,
    grants: BTreeSet<CapabilityGrant>,
    pub max_semantic_payload_bytes: usize,
    pub max_concurrent_engineers: usize,
    pub max_disk_used_percent: u8,
    pub allowed_repository_owners: BTreeSet<String>,
    pub allowed_engineer_permissions: BTreeSet<String>,
}

impl CapabilityPolicy {
    pub fn new(revision: impl Into<String>) -> Self {
        Self {
            revision: revision.into(),
            grants: [
                CapabilityGrant::RecordAction(ActionKind::SpawnEngineer),
                CapabilityGrant::RecordAction(ActionKind::FileIssue),
                CapabilityGrant::RecordAction(ActionKind::RequestMerge),
                CapabilityGrant::RecordAction(ActionKind::RequestDeploy),
                CapabilityGrant::RecordNoAction,
                CapabilityGrant::RecordBlocked,
                CapabilityGrant::RecordCompleted,
                CapabilityGrant::RecordProgress,
            ]
            .into_iter()
            .collect(),
            max_semantic_payload_bytes: 1024 * 1024,
            max_concurrent_engineers: 8,
            max_disk_used_percent: 90,
            allowed_repository_owners: ["rysweet".to_string()].into_iter().collect(),
            allowed_engineer_permissions: [
                "repo_read".to_string(),
                "repo_write".to_string(),
                "github_issue_write".to_string(),
                "github_pr_write".to_string(),
            ]
            .into_iter()
            .collect(),
        }
    }

    pub fn goal_session_default(revision: impl Into<String>) -> Self {
        let mut policy = Self::new(revision);
        policy.grants.remove(&CapabilityGrant::RecordProgress);
        policy
    }

    pub fn with_max_semantic_payload_bytes(mut self, bytes: usize) -> Self {
        self.max_semantic_payload_bytes = bytes;
        self
    }

    pub fn allows(&self, grant: CapabilityGrant) -> bool {
        self.grants.contains(&grant)
    }
}

pub(crate) fn validate_identifier(name: &str, value: &str) -> CapabilityResult<()> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
        })
    {
        return Err(CapabilityError::new(
            CapabilityErrorCode::InvalidArgument,
            format!("{name} must be 1..=128 safe ASCII characters"),
        ));
    }
    Ok(())
}

pub(crate) fn validate_repository(repository: &RepositoryRef) -> CapabilityResult<()> {
    validate_identifier("repository owner", &repository.owner)?;
    validate_identifier("repository name", &repository.name)
}

pub(crate) fn validate_evidence(evidence: &[EvidenceRef]) -> CapabilityResult<()> {
    let unique: BTreeSet<_> = evidence.iter().collect();
    if unique.len() != evidence.len() {
        return Err(CapabilityError::new(
            CapabilityErrorCode::InvalidArgument,
            "duplicate evidence reference",
        ));
    }
    for item in evidence {
        match item {
            EvidenceRef::Commit { repository, sha } => {
                validate_repository(repository)?;
                validate_sha(sha)?;
            }
            EvidenceRef::CheckRun {
                repository,
                check_id,
                conclusion,
            } => {
                validate_repository(repository)?;
                if *check_id == 0 || conclusion.is_empty() {
                    return Err(CapabilityError::new(
                        CapabilityErrorCode::InvalidArgument,
                        "check evidence requires a nonzero id and conclusion",
                    ));
                }
            }
            EvidenceRef::Issue { repository, number } => {
                validate_repository(repository)?;
                if *number == 0 {
                    return Err(CapabilityError::new(
                        CapabilityErrorCode::InvalidArgument,
                        "issue number must be nonzero",
                    ));
                }
            }
            EvidenceRef::EngineerRun {
                session_id,
                claim_key,
            } => {
                validate_identifier("engineer session id", session_id)?;
                validate_identifier("claim key", claim_key)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_sha(sha: &str) -> CapabilityResult<()> {
    if sha.len() != 40 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CapabilityError::new(
            CapabilityErrorCode::InvalidArgument,
            "commit SHA must contain exactly 40 hexadecimal characters",
        ));
    }
    Ok(())
}
