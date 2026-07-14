//! Input/result types for the stewardship loop (issue #1167).

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};
use crate::stewardship::GhIssue;

const MAX_TYPED_ID_LEN: usize = 200;
const MAX_REPO_LEN: usize = 200;
const MAX_ISSUE_TITLE_LEN: usize = 256;
const MAX_ISSUE_BODY_LEN: usize = 64 * 1024;
const MAX_LABELS: usize = 20;
const MAX_LABEL_LEN: usize = 100;
const MAX_ASSIGNEES: usize = 10;
const MAX_ASSIGNEE_LEN: usize = 100;

macro_rules! typed_id {
    ($name:ident, $field:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> SimardResult<Self> {
                let value = value.into();
                let trimmed = value.trim();
                if trimmed.is_empty()
                    || trimmed.len() > MAX_TYPED_ID_LEN
                    || !trimmed.chars().all(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/' | '#')
                    })
                {
                    return Err(SimardError::StewardshipInvalidMutation {
                        field: $field,
                        reason: format!(
                            "must be 1..={MAX_TYPED_ID_LEN} characters from [A-Za-z0-9._:/#-]"
                        ),
                    });
                }
                Ok(Self(trimmed.to_string()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

typed_id!(LineageId, "lineage_id");
typed_id!(CycleId, "cycle_id");
typed_id!(IssueMutationIdentity, "mutation_identity");

impl IssueMutationIdentity {
    /// Build a stable typed identity from opaque structural source bytes.
    ///
    /// This hashes identifiers; it does not infer meaning from prose.
    pub fn from_source(namespace: &str, source: &str) -> Self {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(namespace.as_bytes());
        hasher.update([0]);
        hasher.update(source.as_bytes());
        let digest = hasher.finalize();
        Self(format!("{namespace}:{digest:x}"))
    }
}

impl CycleId {
    /// Stable identity for one explicitly named scheduled cycle. Without a
    /// scheduler token the component remains in one conservative cycle forever.
    pub fn scheduled(component: &str) -> SimardResult<Self> {
        let token = match std::env::var("SIMARD_SCHEDULED_CYCLE_ID") {
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => "current".to_string(),
            Err(error) => {
                return Err(SimardError::StewardshipInvalidMutation {
                    field: "cycle_id",
                    reason: format!("cannot read SIMARD_SCHEDULED_CYCLE_ID: {error}"),
                });
            }
        };
        Self::new(format!("{component}:{token}"))
    }
}

/// Structural origin carried across stewardship recursion boundaries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactOrigin {
    Operator,
    System,
    External,
    Stewardship,
    #[default]
    LegacyUnknown,
}

/// Versioned provenance. Missing legacy fields deserialize fail-closed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactProvenance {
    #[serde(default)]
    pub version: u16,
    #[serde(default)]
    pub origin: ArtifactOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage_id: Option<LineageId>,
}

impl Default for ArtifactProvenance {
    fn default() -> Self {
        Self {
            version: 0,
            origin: ArtifactOrigin::LegacyUnknown,
            lineage_id: None,
        }
    }
}

impl ArtifactProvenance {
    pub const CURRENT_VERSION: u16 = 1;

    fn current(origin: ArtifactOrigin, lineage_id: LineageId) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            origin,
            lineage_id: Some(lineage_id),
        }
    }

    pub fn operator(lineage_id: LineageId) -> Self {
        Self::current(ArtifactOrigin::Operator, lineage_id)
    }

    pub fn system(lineage_id: LineageId) -> Self {
        Self::current(ArtifactOrigin::System, lineage_id)
    }

    pub fn external(lineage_id: LineageId) -> Self {
        Self::current(ArtifactOrigin::External, lineage_id)
    }

    pub fn stewardship(lineage_id: LineageId) -> Self {
        Self::current(ArtifactOrigin::Stewardship, lineage_id)
    }

    pub fn is_recursive_input_eligible(&self) -> bool {
        self.version == Self::CURRENT_VERSION
            && self.lineage_id.is_some()
            && matches!(
                self.origin,
                ArtifactOrigin::Operator | ArtifactOrigin::System | ArtifactOrigin::External
            )
    }
}

/// Finite GitHub-mutation budget for one explicitly started cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IssueMutationLimit(u32);

impl IssueMutationLimit {
    pub const DEFAULT: Self = Self(1);
    pub const MAX: u32 = 100;
    pub const ENV: &'static str = "SIMARD_STEWARDSHIP_ISSUE_MUTATION_LIMIT";
    pub const LEGACY_ENV: &'static str = "SIMARD_STEWARDSHIP_GITHUB_MUTATION_LIMIT";

    pub fn new(value: u32) -> SimardResult<Self> {
        if value == 0 || value > Self::MAX {
            return Err(SimardError::StewardshipInvalidMutation {
                field: "mutation_limit",
                reason: format!("must be finite and within 1..={}", Self::MAX),
            });
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u32 {
        self.0
    }

    pub fn configured() -> SimardResult<Self> {
        let configured = match std::env::var(Self::ENV) {
            Ok(raw) => Some((Self::ENV, raw)),
            Err(std::env::VarError::NotPresent) => match std::env::var(Self::LEGACY_ENV) {
                Ok(raw) => Some((Self::LEGACY_ENV, raw)),
                Err(std::env::VarError::NotPresent) => None,
                Err(error) => {
                    return Err(SimardError::StewardshipInvalidMutation {
                        field: "mutation_limit",
                        reason: format!("cannot read {}: {error}", Self::LEGACY_ENV),
                    });
                }
            },
            Err(error) => {
                return Err(SimardError::StewardshipInvalidMutation {
                    field: "mutation_limit",
                    reason: format!("cannot read {}: {error}", Self::ENV),
                });
            }
        };
        let Some((name, raw)) = configured else {
            return Ok(Self::DEFAULT);
        };
        let value = raw
            .parse::<u32>()
            .map_err(|_| SimardError::StewardshipInvalidMutation {
                field: "mutation_limit",
                reason: format!("{name} must be an integer"),
            })?;
        Self::new(value)
    }
}

/// Autonomous issue mutation kinds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IssueMutation {
    Create {
        title: String,
        body: String,
        #[serde(default)]
        labels: Vec<String>,
        #[serde(default)]
        assignees: Vec<String>,
    },
    Edit {
        number: u64,
        title: Option<String>,
        body: Option<String>,
    },
    Close {
        number: u64,
    },
    Reopen {
        number: u64,
    },
}

/// Fully typed authorization input to the guarded mutation boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueMutationRequest {
    pub repo: String,
    pub identity: IssueMutationIdentity,
    pub provenance: ArtifactProvenance,
    pub mutation: IssueMutation,
}

impl IssueMutationRequest {
    pub fn create(
        repo: impl Into<String>,
        identity: IssueMutationIdentity,
        provenance: ArtifactProvenance,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> SimardResult<Self> {
        let request = Self {
            repo: repo.into(),
            identity,
            provenance,
            mutation: IssueMutation::Create {
                title: title.into(),
                body: body.into(),
                labels: Vec::new(),
                assignees: Vec::new(),
            },
        };
        request.validate()?;
        Ok(request)
    }

    pub fn create_with_labels(
        repo: impl Into<String>,
        identity: IssueMutationIdentity,
        provenance: ArtifactProvenance,
        title: impl Into<String>,
        body: impl Into<String>,
        labels: Vec<String>,
    ) -> SimardResult<Self> {
        let mut request = Self::create(repo, identity, provenance, title, body)?;
        if let IssueMutation::Create {
            labels: request_labels,
            ..
        } = &mut request.mutation
        {
            *request_labels = labels;
        }
        request.validate()?;
        Ok(request)
    }

    pub fn create_with_metadata(
        repo: impl Into<String>,
        identity: IssueMutationIdentity,
        provenance: ArtifactProvenance,
        title: impl Into<String>,
        body: impl Into<String>,
        labels: Vec<String>,
        assignees: Vec<String>,
    ) -> SimardResult<Self> {
        let mut request = Self::create(repo, identity, provenance, title, body)?;
        if let IssueMutation::Create {
            labels: request_labels,
            assignees: request_assignees,
            ..
        } = &mut request.mutation
        {
            *request_labels = labels;
            *request_assignees = assignees;
        }
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> SimardResult<()> {
        validate_repo(&self.repo)?;
        match &self.mutation {
            IssueMutation::Create {
                title,
                body,
                labels,
                assignees,
            } => {
                if title.trim().is_empty() || title.len() > MAX_ISSUE_TITLE_LEN {
                    return Err(SimardError::StewardshipInvalidMutation {
                        field: "title",
                        reason: format!("must be 1..={MAX_ISSUE_TITLE_LEN} bytes"),
                    });
                }
                if body.trim().is_empty() || body.len() > MAX_ISSUE_BODY_LEN {
                    return Err(SimardError::StewardshipInvalidMutation {
                        field: "body",
                        reason: format!("must be 1..={MAX_ISSUE_BODY_LEN} bytes"),
                    });
                }
                if labels.len() > MAX_LABELS
                    || labels
                        .iter()
                        .any(|label| label.trim().is_empty() || label.len() > MAX_LABEL_LEN)
                {
                    return Err(SimardError::StewardshipInvalidMutation {
                        field: "labels",
                        reason: format!(
                            "must contain at most {MAX_LABELS} non-empty labels of at most {MAX_LABEL_LEN} bytes"
                        ),
                    });
                }
                if assignees.len() > MAX_ASSIGNEES
                    || assignees.iter().any(|assignee| {
                        assignee.trim().is_empty() || assignee.len() > MAX_ASSIGNEE_LEN
                    })
                {
                    return Err(SimardError::StewardshipInvalidMutation {
                        field: "assignees",
                        reason: format!(
                            "must contain at most {MAX_ASSIGNEES} non-empty assignees of at most {MAX_ASSIGNEE_LEN} bytes"
                        ),
                    });
                }
            }
            IssueMutation::Edit {
                number,
                title,
                body,
            } => {
                if *number == 0
                    || (title.is_none() && body.is_none())
                    || title.as_ref().is_some_and(|title| {
                        title.trim().is_empty() || title.len() > MAX_ISSUE_TITLE_LEN
                    })
                    || body.as_ref().is_some_and(|body| {
                        body.trim().is_empty() || body.len() > MAX_ISSUE_BODY_LEN
                    })
                {
                    return Err(SimardError::StewardshipInvalidMutation {
                        field: "edit",
                        reason:
                            "requires a positive issue number and bounded non-empty title or body"
                                .to_string(),
                    });
                }
            }
            IssueMutation::Close { number } | IssueMutation::Reopen { number } => {
                if *number == 0 {
                    return Err(SimardError::StewardshipInvalidMutation {
                        field: "issue_number",
                        reason: "must be positive".to_string(),
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueMutationOutcome {
    Completed { issue: GhIssue },
    AlreadyCompleted { issue: GhIssue },
}

fn validate_repo(repo: &str) -> SimardResult<()> {
    let mut repo_parts = repo.split('/');
    let owner = repo_parts.next().unwrap_or_default();
    let name = repo_parts.next().unwrap_or_default();
    if repo.len() > MAX_REPO_LEN
        || owner.is_empty()
        || name.is_empty()
        || repo_parts.next().is_some()
        || !owner
            .chars()
            .chain(name.chars())
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(SimardError::StewardshipInvalidMutation {
            field: "repo",
            reason: format!(
                "must be an owner/repo slug up to {MAX_REPO_LEN} ASCII identifier characters"
            ),
        });
    }
    Ok(())
}

/// Failure facts captured from a single Simard orchestrator run, supplied by
/// the caller as the input contract to [`process_orchestrator_run`].
///
/// All fields are required and must be non-empty after trimming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StewardshipDisposition {
    ObservationOnly,
    AuthorizedIssue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratorRunSummary {
    pub run_id: String,
    pub recipe_name: String,
    pub failed_step: String,
    pub source_module: String,
    pub failure_kind: String,
    pub error_text: String,
    /// Stable semantic condition identity supplied by the agentic producer.
    pub condition_id: IssueMutationIdentity,
    /// Explicit cycle identity; the same scheduled cycle retains its budget
    /// across process restarts.
    pub cycle_id: CycleId,
    /// Structural source provenance; stewardship lineage is never authorized.
    pub provenance: ArtifactProvenance,
    /// Typed semantic decision supplied by the producing agent/adapter.
    pub disposition: StewardshipDisposition,
}

/// Repo selected by the source-module routing matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetRepo {
    Amplihack,
    Simard,
}

impl TargetRepo {
    /// Canonical `owner/repo` slug used by the `gh` CLI and links.
    pub fn slug(&self) -> &'static str {
        match self {
            TargetRepo::Amplihack => "rysweet/amplihack",
            TargetRepo::Simard => "rysweet/Simard",
        }
    }
}

/// Outcome of a stewardship cycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StewardshipOutcome {
    /// A new issue was created.
    FiledNew {
        repo: String,
        issue_number: u64,
        url: String,
        signature: String,
    },
    /// An open issue with the same signature already existed; no new issue filed.
    MatchedExisting {
        repo: String,
        issue_number: u64,
        url: String,
        signature: String,
    },
}

/// Validate that all required fields are non-empty. Fail-loud — no defaults.
pub(crate) fn validate(run: &OrchestratorRunSummary) -> SimardResult<()> {
    if run.run_id.trim().is_empty() {
        return Err(SimardError::StewardshipInvalidRunSummary { field: "run_id" });
    }
    if run.recipe_name.trim().is_empty() {
        return Err(SimardError::StewardshipInvalidRunSummary {
            field: "recipe_name",
        });
    }
    if run.failed_step.trim().is_empty() {
        return Err(SimardError::StewardshipInvalidRunSummary {
            field: "failed_step",
        });
    }
    if run.source_module.trim().is_empty() {
        return Err(SimardError::StewardshipInvalidRunSummary {
            field: "source_module",
        });
    }
    if run.failure_kind.trim().is_empty() {
        return Err(SimardError::StewardshipInvalidRunSummary {
            field: "failure_kind",
        });
    }
    if run.error_text.trim().is_empty() {
        return Err(SimardError::StewardshipInvalidRunSummary {
            field: "error_text",
        });
    }
    Ok(())
}
