//! Data types for the governed-fleet CI-health sweep.
//!
//! These model exactly the two `gh` surfaces the sweep reads — workflow
//! *enablement state* (`gh workflow list --json name,state`) and the *latest
//! run* of each workflow on the default branch (`gh run list --json
//! workflowName,status,conclusion,event,createdAt,databaseId`). Keeping the
//! enablement state alongside the run conclusion is the whole point: a
//! `disabled_manually` workflow whose last run happened to be a `failure` is
//! **not** an actionable CI failure, and only a type that carries both facts
//! can express that.

/// GitHub Actions workflow enablement state, from `gh workflow list --json state`.
///
/// A disabled workflow will never run again, so a stale `failure` on its last
/// run is not an active-CI signal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkflowState {
    Active,
    DisabledManually,
    DisabledInactivity,
    /// A state string GitHub returned that we do not model explicitly.
    Unknown(String),
}

impl WorkflowState {
    /// Parse the `state` field GitHub emits. Never fails — unmodeled values
    /// fall through to [`WorkflowState::Unknown`] so a new GitHub state can
    /// never silently look "active".
    pub fn parse(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "disabled_manually" => Self::DisabledManually,
            "disabled_inactivity" => Self::DisabledInactivity,
            other => Self::Unknown(other.to_string()),
        }
    }

    /// True when the workflow is turned off and cannot produce new runs.
    pub fn is_disabled(&self) -> bool {
        matches!(self, Self::DisabledManually | Self::DisabledInactivity)
    }

    /// Canonical GitHub string for reporting.
    pub fn as_gh_str(&self) -> String {
        match self {
            Self::Active => "active".to_string(),
            Self::DisabledManually => "disabled_manually".to_string(),
            Self::DisabledInactivity => "disabled_inactivity".to_string(),
            Self::Unknown(s) => s.clone(),
        }
    }
}

/// GitHub Actions run conclusion, from `gh run list --json conclusion`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunConclusion {
    Success,
    Failure,
    Cancelled,
    Skipped,
    Neutral,
    TimedOut,
    ActionRequired,
    Stale,
    StartupFailure,
    /// A conclusion string GitHub returned that we do not model explicitly.
    Unknown(String),
}

impl RunConclusion {
    /// Parse the `conclusion` field GitHub emits. Never fails.
    pub fn parse(s: &str) -> Self {
        match s {
            "success" => Self::Success,
            "failure" => Self::Failure,
            "cancelled" => Self::Cancelled,
            "skipped" => Self::Skipped,
            "neutral" => Self::Neutral,
            "timed_out" => Self::TimedOut,
            "action_required" => Self::ActionRequired,
            "stale" => Self::Stale,
            "startup_failure" => Self::StartupFailure,
            other => Self::Unknown(other.to_string()),
        }
    }

    /// True for conclusions that represent a genuine failing run of an
    /// **active** workflow — the only conclusions the sweep treats as
    /// actionable. `cancelled`/`skipped`/`neutral`/`action_required`/`stale`
    /// are deliberately excluded: they are not failures.
    pub fn is_actionable_failure(&self) -> bool {
        matches!(self, Self::Failure | Self::TimedOut | Self::StartupFailure)
    }

    /// Canonical GitHub string for reporting.
    pub fn as_gh_str(&self) -> String {
        match self {
            Self::Success => "success".to_string(),
            Self::Failure => "failure".to_string(),
            Self::Cancelled => "cancelled".to_string(),
            Self::Skipped => "skipped".to_string(),
            Self::Neutral => "neutral".to_string(),
            Self::TimedOut => "timed_out".to_string(),
            Self::ActionRequired => "action_required".to_string(),
            Self::Stale => "stale".to_string(),
            Self::StartupFailure => "startup_failure".to_string(),
            Self::Unknown(s) => s.clone(),
        }
    }
}

/// The latest run of a single workflow on a repo's default branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowRun {
    /// Run status, e.g. `completed`, `in_progress`, `queued`.
    pub status: String,
    /// Conclusion; `None` until the run has completed.
    pub conclusion: Option<RunConclusion>,
    /// Triggering event, e.g. `push`, `pull_request`, `schedule`, `issues`.
    pub event: String,
    /// ISO-8601 creation timestamp, used only to pick the newest run.
    pub created_at: String,
    /// GitHub run database id, echoed into reports so a human can open the run.
    pub database_id: u64,
}

/// A workflow plus its enablement state and latest default-branch run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowSnapshot {
    pub name: String,
    pub state: WorkflowState,
    /// `None` when the workflow has never run on the default branch.
    pub latest_run: Option<WorkflowRun>,
}

/// Every workflow of one governed repo, resolved against its default branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoSnapshot {
    pub slug: String,
    pub default_branch: String,
    pub workflows: Vec<WorkflowSnapshot>,
}

/// A full sweep of the governed fleet: the sole input to classification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FleetSnapshot {
    pub repos: Vec<RepoSnapshot>,
}
