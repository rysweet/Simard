//! Pure CI-health classification — the root-cause fix.
//!
//! Given a [`FleetSnapshot`], decide, per workflow, whether its latest
//! default-branch run is a **genuine actionable failure**, a **green** result,
//! or a non-actionable signal to **ignore** (with a reason). The fleet is
//! healthy iff no workflow is an actionable failure.
//!
//! This module is deliberately free of `gh`, I/O, and time: it is a total
//! function over the snapshot, which makes the sweep's verdict reproducible
//! and exhaustively unit-testable.

use serde::Serialize;

use super::types::{FleetSnapshot, RunConclusion, WorkflowSnapshot};

/// Why a workflow's latest run is not counted as an actionable failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IgnoreReason {
    /// The workflow is turned off (`disabled_manually`/`disabled_inactivity`);
    /// a stale last-run failure cannot recur and is not active CI.
    WorkflowDisabled,
    /// The latest run completed with a non-failure conclusion such as
    /// `cancelled`, `skipped`, `neutral`, `action_required`, or `stale`.
    NonFailureConclusion(RunConclusion),
    /// The workflow has never run on the default branch.
    NoRun,
    /// The latest run has not completed yet.
    InProgress,
}

/// The verdict for a single workflow's latest default-branch run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkflowVerdict {
    /// Active workflow whose latest run concluded `success`.
    Green,
    /// Active workflow whose latest run genuinely failed — the only verdict
    /// that makes the fleet non-green.
    ActionableFailure { conclusion: RunConclusion },
    /// A non-actionable signal; see [`IgnoreReason`].
    Ignored { reason: IgnoreReason },
}

/// Classify one workflow. Disabled state is checked **first** so an active-
/// looking stale failure on a turned-off workflow is correctly ignored.
pub fn classify_workflow(wf: &WorkflowSnapshot) -> WorkflowVerdict {
    if wf.state.is_disabled() {
        return WorkflowVerdict::Ignored {
            reason: IgnoreReason::WorkflowDisabled,
        };
    }
    let Some(run) = &wf.latest_run else {
        return WorkflowVerdict::Ignored {
            reason: IgnoreReason::NoRun,
        };
    };
    if run.status != "completed" {
        return WorkflowVerdict::Ignored {
            reason: IgnoreReason::InProgress,
        };
    }
    match &run.conclusion {
        Some(c) if c.is_actionable_failure() => WorkflowVerdict::ActionableFailure {
            conclusion: c.clone(),
        },
        Some(RunConclusion::Success) => WorkflowVerdict::Green,
        Some(other) => WorkflowVerdict::Ignored {
            reason: IgnoreReason::NonFailureConclusion(other.clone()),
        },
        // Completed with a null conclusion is indeterminate, not a failure.
        None => WorkflowVerdict::Ignored {
            reason: IgnoreReason::InProgress,
        },
    }
}

// ── Serializable report DTOs ────────────────────────────────────────────────

/// One actionable failure, hoisted to the top of the report for triage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ActionableFailure {
    pub repo: String,
    pub default_branch: String,
    pub workflow: String,
    pub conclusion: String,
    pub run_id: Option<u64>,
    pub run_url: Option<String>,
}

/// Per-workflow verdict, flattened to stable strings for JSON/consumers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkflowReport {
    pub name: String,
    /// One of `green`, `actionable_failure`, `ignored`.
    pub verdict: String,
    /// GitHub conclusion string for `actionable_failure`; `None` otherwise.
    pub conclusion: Option<String>,
    /// Ignore reason for `ignored`; `None` otherwise.
    pub reason: Option<String>,
    pub run_id: Option<u64>,
}

/// Per-repo report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RepoReport {
    pub slug: String,
    pub default_branch: String,
    pub workflows: Vec<WorkflowReport>,
}

/// The whole-fleet report; `green` is the gate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FleetReport {
    pub green: bool,
    pub repos_checked: usize,
    pub workflows_checked: usize,
    pub actionable_failures: Vec<ActionableFailure>,
    pub repos: Vec<RepoReport>,
}

fn run_url(slug: &str, run_id: u64) -> String {
    format!("https://github.com/{slug}/actions/runs/{run_id}")
}

fn ignore_reason_str(reason: &IgnoreReason) -> String {
    match reason {
        IgnoreReason::WorkflowDisabled => "workflow_disabled".to_string(),
        IgnoreReason::NonFailureConclusion(c) => {
            format!("non_failure_conclusion:{}", c.as_gh_str())
        }
        IgnoreReason::NoRun => "no_default_branch_run".to_string(),
        IgnoreReason::InProgress => "run_in_progress".to_string(),
    }
}

/// Classify a whole fleet snapshot into a serializable report. The fleet is
/// `green` iff it contains zero actionable failures.
pub fn build_report(snapshot: &FleetSnapshot) -> FleetReport {
    let mut actionable_failures = Vec::new();
    let mut repos = Vec::with_capacity(snapshot.repos.len());
    let mut workflows_checked = 0usize;

    for repo in &snapshot.repos {
        let mut wf_reports = Vec::with_capacity(repo.workflows.len());
        for wf in &repo.workflows {
            workflows_checked += 1;
            let run_id = wf.latest_run.as_ref().map(|r| r.database_id);
            let verdict = classify_workflow(wf);
            let report = match &verdict {
                WorkflowVerdict::Green => WorkflowReport {
                    name: wf.name.clone(),
                    verdict: "green".to_string(),
                    conclusion: None,
                    reason: None,
                    run_id,
                },
                WorkflowVerdict::ActionableFailure { conclusion } => {
                    actionable_failures.push(ActionableFailure {
                        repo: repo.slug.clone(),
                        default_branch: repo.default_branch.clone(),
                        workflow: wf.name.clone(),
                        conclusion: conclusion.as_gh_str(),
                        run_id,
                        run_url: run_id.map(|id| run_url(&repo.slug, id)),
                    });
                    WorkflowReport {
                        name: wf.name.clone(),
                        verdict: "actionable_failure".to_string(),
                        conclusion: Some(conclusion.as_gh_str()),
                        reason: None,
                        run_id,
                    }
                }
                WorkflowVerdict::Ignored { reason } => WorkflowReport {
                    name: wf.name.clone(),
                    verdict: "ignored".to_string(),
                    conclusion: None,
                    reason: Some(ignore_reason_str(reason)),
                    run_id,
                },
            };
            wf_reports.push(report);
        }
        repos.push(RepoReport {
            slug: repo.slug.clone(),
            default_branch: repo.default_branch.clone(),
            workflows: wf_reports,
        });
    }

    FleetReport {
        green: actionable_failures.is_empty(),
        repos_checked: snapshot.repos.len(),
        workflows_checked,
        actionable_failures,
        repos,
    }
}
