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

use super::cache::GreenShaCache;
use super::types::{FleetSnapshot, RepoSnapshot, RunConclusion, WorkflowSnapshot};

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
    /// True when this repo was served from the last-known-green cache (its head
    /// SHA was unchanged since the last green verdict) and therefore not
    /// re-collected this sweep. Its `workflows` list is empty.
    pub green_from_cache: bool,
    pub workflows: Vec<WorkflowReport>,
}

/// The whole-fleet report; `green` is the gate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FleetReport {
    pub green: bool,
    pub repos_checked: usize,
    /// How many of `repos_checked` were served from the last-known-green cache
    /// (skipped, not re-collected). The remaining `repos_checked -
    /// repos_from_cache` were freshly swept.
    pub repos_from_cache: usize,
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
/// `green` iff it contains zero actionable failures. Repos served from the
/// last-known-green cache ([`RepoSnapshot::green_from_cache`]) carry no
/// workflows and contribute zero actionable failures, so they keep the fleet
/// green by construction while advertising that they were not re-collected.
pub fn build_report(snapshot: &FleetSnapshot) -> FleetReport {
    let mut actionable_failures = Vec::new();
    let mut repos = Vec::with_capacity(snapshot.repos.len());
    let mut workflows_checked = 0usize;
    let mut repos_from_cache = 0usize;

    for repo in &snapshot.repos {
        if repo.green_from_cache {
            repos_from_cache += 1;
            repos.push(RepoReport {
                slug: repo.slug.clone(),
                default_branch: repo.default_branch.clone(),
                green_from_cache: true,
                workflows: Vec::new(),
            });
            continue;
        }
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
            green_from_cache: false,
            workflows: wf_reports,
        });
    }

    FleetReport {
        green: actionable_failures.is_empty(),
        repos_checked: snapshot.repos.len(),
        repos_from_cache,
        workflows_checked,
        actionable_failures,
        repos,
    }
}

/// Events that can only produce a new default-branch run when a **new commit**
/// lands on that branch. For a workflow whose latest run was one of these, an
/// unchanged head SHA proves it has not run again — the key soundness property
/// the green-SHA cache relies on. Any other event (`schedule`,
/// `workflow_dispatch`, `repository_dispatch`, `issues`, `workflow_run`, …) can
/// fire without a commit and so makes the repo ineligible for caching.
fn is_commit_driven(event: &str) -> bool {
    matches!(
        event,
        "push" | "pull_request" | "pull_request_target" | "merge_group"
    )
}

/// Whether a **freshly collected** repo may be recorded in the last-known-green
/// cache. Pure and total over the snapshot.
///
/// A repo is cacheable iff **no active workflow demonstrates that it can run
/// without a new default-branch commit**. Concretely, for every active workflow:
///
/// - a **disabled** workflow is ignored (it cannot run);
/// - a workflow that has **never run** on the default branch is allowed — a
///   workflow with a scheduled trigger would already have runs, so a no-run
///   active workflow is almost certainly triggered only by events that require a
///   commit or an explicit human action (PR, tag, `release`, `workflow_dispatch`
///   invoked by a person, Copilot agents, …), none of which fire on an unchanged
///   default branch;
/// - a workflow whose latest run is **still in progress** disqualifies the repo
///   (that run could yet conclude failure);
/// - a workflow whose latest run **completed with a non-commit-driven event**
///   (`schedule`, `workflow_dispatch`, `repository_dispatch`, `dynamic`,
///   `issues`, …) disqualifies the repo — it has *demonstrably* run without a
///   commit, so a future such run could fail on an unchanged head SHA;
/// - a workflow whose latest run **failed** disqualifies the repo (it is not
///   green — normally also caught by the report's failure set).
///
/// A commit-driven latest run (`push`, `pull_request`, `pull_request_target`,
/// `merge_group`) is the intended green case. A repo already served from cache
/// is trivially still cacheable.
///
/// The residual (a scheduled workflow so new it has never fired) is narrow,
/// self-heals on the next commit, and is covered by `--no-cache` / periodic full
/// sweeps.
pub fn repo_cacheable(repo: &RepoSnapshot) -> bool {
    if repo.green_from_cache {
        return true;
    }
    for wf in &repo.workflows {
        if wf.state.is_disabled() {
            continue;
        }
        let Some(run) = &wf.latest_run else {
            continue; // active workflow that has never run: see doc above.
        };
        if run.status != "completed" {
            return false; // in progress: could still conclude failure.
        }
        match &run.conclusion {
            Some(c) if c.is_actionable_failure() => return false,
            None => return false, // completed with null conclusion: indeterminate.
            Some(_) => {}         // success or a non-failure conclusion.
        }
        if !is_commit_driven(&run.event) {
            return false; // demonstrably ran without a commit; can do so again.
        }
    }
    true
}

/// Reconcile the last-known-green cache against a freshly built report.
///
/// For each repo in `snapshot`:
/// - **served from cache** → keep its existing entry (its SHA is unchanged).
/// - **freshly collected and failing** → invalidate (drop any stale green SHA).
/// - **freshly collected and green** → record its head SHA iff
///   [`repo_cacheable`]; otherwise invalidate so a now-uncacheable repo (e.g. one
///   that grew a scheduled workflow) is never skipped on a stale entry.
///
/// This is the only writer of the cache and is a pure function of the snapshot
/// and report, which keeps the cache's contents auditable and unit-testable.
pub fn update_cache_from_report(
    cache: &mut GreenShaCache,
    snapshot: &FleetSnapshot,
    report: &FleetReport,
) {
    use std::collections::HashSet;
    let failing: HashSet<&str> = report
        .actionable_failures
        .iter()
        .map(|f| f.repo.as_str())
        .collect();

    for repo in &snapshot.repos {
        if repo.green_from_cache {
            continue;
        }
        if failing.contains(repo.slug.as_str()) {
            cache.invalidate(&repo.slug);
        } else if repo_cacheable(repo) {
            cache.record_green(&repo.slug, &repo.head_sha);
        } else {
            cache.invalidate(&repo.slug);
        }
    }
}
