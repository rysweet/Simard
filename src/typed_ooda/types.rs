use std::collections::BTreeSet;
use std::fmt::{self, Display, Formatter};
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub(crate) const COPILOT_ENGINEER_PERMISSIONS: [&str; 5] = [
    "repo_read",
    "repo_write",
    "process_exec",
    "github_issue_write",
    "github_pr_write",
];

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

    /// Normalize a goal's stored repository slug into a canonical
    /// owner-qualified [`RepositoryRef`] for the spawn admission check.
    ///
    /// This is the **single source of truth** for goal-repo normalization; the
    /// `goal_repository` helper in the (test-excluded) effect handler delegates
    /// to it so the rule is unit-testable here.
    ///
    /// Thin deterministic rail (BUG 2): goals frequently store a BARE repo
    /// name (e.g. `"agent-kgpacks-rs-audit"` or `"skwaq"`), while the actor
    /// always produces an owner-qualified request (`rysweet/<name>`). This
    /// binds them to the same canonical form:
    ///
    /// - `None` => `rysweet/Simard` (the default goal repository)
    /// - a bare name (no `'/'`) => `rysweet/<name>`
    /// - an `owner/name` slug => split verbatim, so a genuinely different
    ///   owner is preserved and still correctly mismatches — the rail is not
    ///   loosened.
    pub fn from_goal_slug(slug: Option<&str>) -> Self {
        match slug {
            None => Self::new("rysweet", "Simard"),
            Some(value) => match value.split_once('/') {
                Some((owner, name)) => Self::new(owner, name),
                None => Self::new("rysweet", value),
            },
        }
    }

    /// Deterministic engineer-claim key for `goal_id`: `{owner}/{name}:{goal_id}`.
    ///
    /// **Single source of truth** for the `engineer_claims` key. The
    /// spawn-admission path (which inserts the claim) and the
    /// release/reclaim-on-termination paths MUST produce the identical string,
    /// so both go through here. A silent divergence in this formula is exactly
    /// the bug class that leaked claims and permanently locked out goals
    /// (issue #4094).
    pub fn claim_key(&self, goal_id: &str) -> String {
        format!("{}/{}:{}", self.owner, self.name, goal_id)
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

    pub fn repository(&self) -> Option<&RepositoryRef> {
        match self {
            Self::SpawnEngineer(value) => Some(&value.repository),
            Self::FileIssue(value) => Some(&value.repository),
            Self::RequestMerge(value) => Some(&value.pull_request.repository),
            Self::RequestDeploy(_) => None,
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct RecordActionRequest {
    pub identity: TerminalRequestIdentity,
    pub action: Action,
    pub raw_semantic: OpaqueBytes,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordNoActionRequest {
    pub identity: TerminalRequestIdentity,
    pub reason: OpaqueBytes,
    pub raw_semantic: OpaqueBytes,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordBlockedRequest {
    pub identity: TerminalRequestIdentity,
    pub reason: OpaqueBytes,
    pub blocker: BlockerRef,
    pub retry: RetryPolicy,
    pub raw_semantic: OpaqueBytes,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordCompletedRequest {
    pub identity: TerminalRequestIdentity,
    pub summary: OpaqueBytes,
    pub completion: CompletionRef,
    pub raw_semantic: OpaqueBytes,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordProgressRequest {
    pub identity: TerminalRequestIdentity,
    pub percent: u8,
    pub summary: OpaqueBytes,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessExecRequest {
    pub identity: TerminalRequestIdentity,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub working_directory: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessExecutionStatus {
    Reserved,
    Running,
    Completed,
    Failed,
    Indeterminate,
}

impl ProcessExecutionStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Indeterminate => "indeterminate",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessExecutionRecord {
    pub execution_id: String,
    pub request_id: String,
    pub session_id: String,
    pub cycle_id: String,
    pub goal_id: String,
    pub status: ProcessExecutionStatus,
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
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
    pub fn kind(&self) -> TerminalKind {
        match self {
            Self::Action(_) => TerminalKind::Action,
            Self::NoAction(_) => TerminalKind::NoAction,
            Self::Blocked(_) => TerminalKind::Blocked,
            Self::Completed(_) => TerminalKind::Completed,
        }
    }

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
    #[serde(default)]
    pub repository: Option<RepositoryRef>,
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
    InvalidIdentifier,
    PayloadTooLarge,
    Unauthenticated,
    PermissionDenied,
    AuthorizationScopeViolation,
    AdmissionRejected,
    StateTransitionRejected,
    IdempotencyConflict,
    RequestConflict,
    TerminalAlreadyRecorded,
    StaleLease,
    MutationCapExhausted,
    IndeterminateExecution,
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityGrant {
    RecordAction(ActionKind),
    RecordNoAction,
    RecordBlocked,
    RecordCompleted,
    RecordProgress,
    ProcessExec,
    DirectMerge,
    DirectDeploy,
}

#[derive(Clone, Debug)]
pub struct AuthenticatedToolContext {
    pub actor_identity: String,
    pub session_id: String,
    grants: BTreeSet<CapabilityGrant>,
    bound_repository: Option<RepositoryRef>,
    bound_cycle_id: Option<String>,
    bound_goal_id: Option<String>,
    bound_working_directory: Option<PathBuf>,
    engineer_permissions: BTreeSet<String>,
    observe_only: bool,
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
            bound_repository: None,
            bound_cycle_id: None,
            bound_goal_id: None,
            bound_working_directory: None,
            engineer_permissions: BTreeSet::new(),
            observe_only: false,
        }
    }

    pub fn scoped_to_repository(mut self, repository: RepositoryRef) -> Self {
        self.bound_repository = Some(repository);
        self
    }

    pub fn bound_to_cycle_goal(
        mut self,
        cycle_id: impl Into<String>,
        goal_id: impl Into<String>,
    ) -> Self {
        self.bound_cycle_id = Some(cycle_id.into());
        self.bound_goal_id = Some(goal_id.into());
        self
    }

    pub fn with_engineer_permissions(
        mut self,
        permissions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.engineer_permissions = permissions.into_iter().map(Into::into).collect();
        self
    }

    pub fn scoped_to_working_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.bound_working_directory = Some(path.into());
        self
    }

    pub fn with_observe_only(mut self, observe_only: bool) -> Self {
        self.observe_only = observe_only;
        self
    }

    pub(crate) fn allows(&self, grant: CapabilityGrant) -> bool {
        self.grants.contains(&grant)
    }

    pub(crate) fn grants(&self) -> &BTreeSet<CapabilityGrant> {
        &self.grants
    }

    pub(crate) fn bound_repository(&self) -> Option<&RepositoryRef> {
        self.bound_repository.as_ref()
    }

    pub(crate) fn bound_cycle_id(&self) -> Option<&str> {
        self.bound_cycle_id.as_deref()
    }

    pub(crate) fn bound_goal_id(&self) -> Option<&str> {
        self.bound_goal_id.as_deref()
    }

    pub(crate) fn engineer_permissions(&self) -> &BTreeSet<String> {
        &self.engineer_permissions
    }

    pub(crate) fn bound_working_directory(&self) -> Option<&Path> {
        self.bound_working_directory.as_deref()
    }

    pub(crate) fn is_observe_only(&self) -> bool {
        self.observe_only
    }
}

#[derive(Clone, Debug)]
pub struct CapabilityPolicy {
    pub revision: String,
    grants: BTreeSet<CapabilityGrant>,
    pub max_semantic_payload_bytes: usize,
    pub max_concurrent_engineers: usize,
    pub max_disk_used_percent: u8,
    pub allowed_repositories: BTreeSet<RepositoryRef>,
    pub allowed_repository_owners: BTreeSet<String>,
    pub allowed_engineer_permissions: BTreeSet<String>,
    pub allowed_deployment_environments: BTreeSet<String>,
    pub process_exec_mutations_per_cycle: usize,
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
                CapabilityGrant::ProcessExec,
            ]
            .into_iter()
            .collect(),
            max_semantic_payload_bytes: 1024 * 1024,
            max_concurrent_engineers: 8,
            max_disk_used_percent: 90,
            allowed_repositories: [RepositoryRef::new("rysweet", "Simard")]
                .into_iter()
                .collect(),
            allowed_repository_owners: ["rysweet".to_string()].into_iter().collect(),
            allowed_engineer_permissions: [
                "repo_read".to_string(),
                "repo_write".to_string(),
                "process_exec".to_string(),
                "github_issue_write".to_string(),
                "github_pr_write".to_string(),
            ]
            .into_iter()
            .collect(),
            allowed_deployment_environments: ["production".to_string()].into_iter().collect(),
            process_exec_mutations_per_cycle: 8,
        }
    }

    pub fn goal_session_default(revision: impl Into<String>) -> Self {
        let mut policy = Self::new(revision);
        policy.grants = [
            CapabilityGrant::RecordAction(ActionKind::SpawnEngineer),
            CapabilityGrant::RecordNoAction,
            CapabilityGrant::RecordBlocked,
            CapabilityGrant::RecordCompleted,
        ]
        .into_iter()
        .collect();
        policy.allowed_engineer_permissions = ["repo_read".to_string(), "repo_write".to_string()]
            .into_iter()
            .collect();
        policy.allowed_deployment_environments.clear();
        policy
    }

    pub fn with_max_semantic_payload_bytes(mut self, bytes: usize) -> Self {
        self.max_semantic_payload_bytes = bytes;
        self
    }

    pub fn with_process_exec_mutations_per_cycle(mut self, limit: usize) -> Self {
        self.process_exec_mutations_per_cycle = limit;
        self
    }

    pub fn from_toml_file(path: impl AsRef<Path>) -> CapabilityResult<Self> {
        #[derive(Deserialize)]
        struct PolicyDocument {
            policy_id: String,
            actor: String,
            terminal_calls_per_cycle: u8,
            capabilities: Vec<String>,
            limits: PolicyLimits,
            identity: PolicyIdentity,
            repositories: Vec<RepositoryRef>,
            repository_owners: Vec<String>,
            engineer_permissions: Vec<String>,
            deployment_environments: Vec<String>,
        }

        #[derive(Deserialize)]
        struct PolicyLimits {
            max_semantic_payload_bytes: usize,
            max_concurrent_engineers: usize,
            max_disk_used_percent: u8,
            process_exec_mutations_per_cycle: Option<usize>,
        }

        #[derive(Deserialize)]
        struct PolicyIdentity {
            bind_session: bool,
            stable_request_id_required: bool,
        }

        let bytes = std::fs::read(path.as_ref()).map_err(|error| {
            CapabilityError::new(
                CapabilityErrorCode::PersistenceFailed,
                format!(
                    "capability policy {} could not be read: {error}",
                    path.as_ref().display()
                ),
            )
        })?;
        let source = std::str::from_utf8(&bytes).map_err(|error| {
            CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                format!(
                    "capability policy {} is not UTF-8: {error}",
                    path.as_ref().display()
                ),
            )
        })?;
        let document: PolicyDocument = toml::from_str(source).map_err(|error| {
            CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                format!(
                    "capability policy {} is invalid: {error}",
                    path.as_ref().display()
                ),
            )
        })?;
        if document.actor != "goal-session-actor"
            || document.terminal_calls_per_cycle != 1
            || !document.identity.bind_session
            || !document.identity.stable_request_id_required
            || (document.repositories.is_empty() && document.repository_owners.is_empty())
        {
            return Err(CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "capability policy must bind one goal-session actor call to a stable session request and at least one repository",
            ));
        }
        if document.limits.max_semantic_payload_bytes == 0
            || !(1..=64).contains(&document.limits.max_concurrent_engineers)
            || !(1..=99).contains(&document.limits.max_disk_used_percent)
            || document
                .limits
                .process_exec_mutations_per_cycle
                .unwrap_or(8)
                == 0
        {
            return Err(CapabilityError::new(
                CapabilityErrorCode::InvalidArgument,
                "capability policy limits are outside supported ranges",
            ));
        }

        let mut grants = BTreeSet::new();
        for capability in document.capabilities {
            grants.insert(Self::parse_capability_grant(&capability)?);
        }
        if document
            .engineer_permissions
            .iter()
            .any(|permission| !COPILOT_ENGINEER_PERMISSIONS.contains(&permission.as_str()))
        {
            return Err(CapabilityError::new(
                CapabilityErrorCode::AuthorizationScopeViolation,
                "capability policy contains a permission outside the canonical Copilot base-type allowlist",
            ));
        }
        Ok(Self {
            revision: document.policy_id,
            grants,
            max_semantic_payload_bytes: document.limits.max_semantic_payload_bytes,
            max_concurrent_engineers: document.limits.max_concurrent_engineers,
            max_disk_used_percent: document.limits.max_disk_used_percent,
            allowed_repositories: document.repositories.into_iter().collect(),
            allowed_repository_owners: document.repository_owners.into_iter().collect(),
            allowed_engineer_permissions: document.engineer_permissions.into_iter().collect(),
            allowed_deployment_environments: document.deployment_environments.into_iter().collect(),
            process_exec_mutations_per_cycle: document
                .limits
                .process_exec_mutations_per_cycle
                .unwrap_or(8),
        })
    }

    pub fn allows(&self, grant: CapabilityGrant) -> bool {
        self.grants.contains(&grant)
    }

    fn parse_capability_grant(value: &str) -> CapabilityResult<CapabilityGrant> {
        let grant = match value {
            "record_action.spawn_engineer" => {
                CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)
            }
            "record_action.file_issue" => CapabilityGrant::RecordAction(ActionKind::FileIssue),
            "record_action.request_merge" => {
                CapabilityGrant::RecordAction(ActionKind::RequestMerge)
            }
            "record_action.request_deploy" => {
                CapabilityGrant::RecordAction(ActionKind::RequestDeploy)
            }
            "record_no_action" => CapabilityGrant::RecordNoAction,
            "record_blocked" => CapabilityGrant::RecordBlocked,
            "record_completed" => CapabilityGrant::RecordCompleted,
            "record_progress" => CapabilityGrant::RecordProgress,
            "process_exec" => CapabilityGrant::ProcessExec,
            _ => {
                return Err(CapabilityError::new(
                    CapabilityErrorCode::InvalidArgument,
                    format!("unknown capability policy grant {value:?}"),
                ));
            }
        };
        Ok(grant)
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
            CapabilityErrorCode::InvalidIdentifier,
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

#[cfg(test)]
mod goal_slug_normalization_tests {
    //! BUG 2 regression: the typed spawn-engineer repo rail must bind a BARE
    //! goal repo slug (e.g. `agent-kgpacks-rs-audit`) to the canonical
    //! `rysweet` owner before comparing it against the actor's — always
    //! owner-qualified — request, while still REJECTING a genuinely different
    //! owner.
    //!
    //! On the live daemon the old inline `"Simard"`-only special case rejected
    //! ~11/20 goals (`typed spawn repository ... does not match goal repository
    //! ...`) because every bare slug other than `"Simard"` failed the naive
    //! `"rysweet/<name>" != "<name>"` compare. `RepositoryRef::from_goal_slug`
    //! is the single normalization source of truth the rail routes through
    //! (via `goal_repository` -> `require_goal_repository`), replacing that
    //! special case with the general bare-name rule.
    //!
    //! These are pure, side-effect-free tests: the effect handler
    //! `LiveGoalSessionEffects::spawn_engineer` is compiled only under
    //! `cfg(not(test))`, so the contract is pinned here at its normalization
    //! seam rather than through the (test-excluded) effect handler.
    use super::RepositoryRef;

    #[test]
    fn bare_name_binds_to_rysweet_owner_and_matches_request() {
        // (a) bare "agent-kgpacks-rs-audit" + requested
        //     "rysweet/agent-kgpacks-rs-audit" => allowed
        let requested = RepositoryRef::new("rysweet", "agent-kgpacks-rs-audit");
        let expected = RepositoryRef::from_goal_slug(Some("agent-kgpacks-rs-audit"));
        assert_eq!(
            expected,
            RepositoryRef::new("rysweet", "agent-kgpacks-rs-audit"),
        );
        assert_eq!(
            expected, requested,
            "a bare goal slug must be admitted against the actor's rysweet-scoped request",
        );
    }

    #[test]
    fn bare_simard_binds_to_rysweet_simard() {
        // (b) bare "Simard" + requested "rysweet/Simard" => allowed
        let requested = RepositoryRef::new("rysweet", "Simard");
        let expected = RepositoryRef::from_goal_slug(Some("Simard"));
        assert_eq!(expected, RepositoryRef::new("rysweet", "Simard"));
        assert_eq!(expected, requested);
    }

    #[test]
    fn none_defaults_to_rysweet_simard() {
        // (c) None goal.repo + requested "rysweet/Simard" => allowed
        let requested = RepositoryRef::new("rysweet", "Simard");
        let expected = RepositoryRef::from_goal_slug(None);
        assert_eq!(expected, RepositoryRef::new("rysweet", "Simard"));
        assert_eq!(expected, requested);
    }

    #[test]
    fn explicit_other_owner_is_preserved_and_rejects_rysweet_request() {
        // (d) explicit "otherowner/thing" + requested "rysweet/thing"
        //     => rejected (mismatch preserved — rail NOT loosened)
        let requested = RepositoryRef::new("rysweet", "thing");
        let expected = RepositoryRef::from_goal_slug(Some("otherowner/thing"));
        assert_eq!(expected, RepositoryRef::new("otherowner", "thing"));
        assert_ne!(
            expected, requested,
            "an explicit non-rysweet owner must never be normalized into the rysweet namespace",
        );
    }
}
