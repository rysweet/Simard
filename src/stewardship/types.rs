//! Input/result types for the stewardship loop (issue #1167).

use crate::error::{SimardError, SimardResult};

/// Failure facts captured from a single Simard orchestrator run, supplied by
/// the caller as the input contract to [`process_orchestrator_run`].
///
/// All fields are required and must be non-empty after trimming.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratorRunSummary {
    pub run_id: String,
    pub recipe_name: String,
    pub failed_step: String,
    pub source_module: String,
    pub failure_kind: String,
    pub error_text: String,
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
