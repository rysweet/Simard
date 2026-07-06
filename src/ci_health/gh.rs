//! `gh`-backed collection for the CI-health sweep, plus the pure parse/join
//! helpers that turn `gh` JSON (or an offline fixture) into a
//! [`FleetSnapshot`].
//!
//! The network-touching [`RealGhWorkflowClient`] is intentionally thin; all of
//! the interesting logic — parsing, picking the newest run per workflow, and
//! joining workflows to their latest run — lives in pure functions so it can be
//! unit-tested without a network or `gh`.

use std::collections::HashMap;

use serde::Deserialize;

use crate::error::{SimardError, SimardResult};

use super::types::{
    FleetSnapshot, RepoSnapshot, RunConclusion, WorkflowRun, WorkflowSnapshot, WorkflowState,
};

/// One row of `gh workflow list -R <repo> --all --json name,state`.
#[derive(Clone, Debug, Deserialize)]
pub struct RawWorkflowRow {
    pub name: String,
    pub state: String,
}

/// One row of `gh run list -R <repo> --branch <b> --json
/// workflowName,status,conclusion,event,createdAt,databaseId`.
#[derive(Clone, Debug, Deserialize)]
pub struct RawRunRow {
    #[serde(rename = "workflowName")]
    pub workflow_name: String,
    pub status: String,
    /// GitHub emits `""` for runs that have not completed.
    #[serde(default)]
    pub conclusion: String,
    pub event: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "databaseId")]
    pub database_id: u64,
}

// ── Offline fixture shape (mirrors FleetSnapshot with string enums) ──────────

#[derive(Clone, Debug, Deserialize)]
struct RawFixtureRun {
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    event: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    database_id: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct RawFixtureWorkflow {
    name: String,
    state: String,
    #[serde(default)]
    latest_run: Option<RawFixtureRun>,
}

#[derive(Clone, Debug, Deserialize)]
struct RawFixtureRepo {
    slug: String,
    default_branch: String,
    #[serde(default)]
    workflows: Vec<RawFixtureWorkflow>,
}

#[derive(Clone, Debug, Deserialize)]
struct RawFixtureFleet {
    repos: Vec<RawFixtureRepo>,
}

/// Parse the JSON array from `gh workflow list ... --json name,state`.
pub fn parse_workflow_rows(json: &[u8]) -> SimardResult<Vec<RawWorkflowRow>> {
    serde_json::from_slice(json).map_err(|e| SimardError::CiHealthGhCommandFailed {
        reason: format!("failed to parse `gh workflow list` JSON: {e}"),
    })
}

/// Parse the JSON array from `gh run list ... --json workflowName,...`.
pub fn parse_run_rows(json: &[u8]) -> SimardResult<Vec<RawRunRow>> {
    serde_json::from_slice(json).map_err(|e| SimardError::CiHealthGhCommandFailed {
        reason: format!("failed to parse `gh run list` JSON: {e}"),
    })
}

fn conclusion_from_gh(s: &str) -> Option<RunConclusion> {
    if s.is_empty() {
        None
    } else {
        Some(RunConclusion::parse(s))
    }
}

fn run_from_row(row: &RawRunRow) -> WorkflowRun {
    WorkflowRun {
        status: row.status.clone(),
        conclusion: conclusion_from_gh(&row.conclusion),
        event: row.event.clone(),
        created_at: row.created_at.clone(),
        database_id: row.database_id,
    }
}

/// Reduce a run list to the newest run per workflow name. ISO-8601 UTC
/// timestamps sort chronologically as strings, so a lexicographic max picks
/// the latest run.
pub fn latest_run_by_workflow(rows: &[RawRunRow]) -> HashMap<String, RawRunRow> {
    let mut latest: HashMap<String, RawRunRow> = HashMap::new();
    for row in rows {
        match latest.get(&row.workflow_name) {
            Some(existing) if existing.created_at >= row.created_at => {}
            _ => {
                latest.insert(row.workflow_name.clone(), row.clone());
            }
        }
    }
    latest
}

/// Join workflow rows to their latest run into a [`RepoSnapshot`]. Pure — this
/// is the core of the collector and is unit-tested directly.
pub fn build_repo_snapshot(
    slug: &str,
    default_branch: &str,
    workflows: &[RawWorkflowRow],
    runs: &[RawRunRow],
) -> RepoSnapshot {
    let latest = latest_run_by_workflow(runs);
    let workflows = workflows
        .iter()
        .map(|wf| WorkflowSnapshot {
            name: wf.name.clone(),
            state: WorkflowState::parse(&wf.state),
            latest_run: latest.get(&wf.name).map(run_from_row),
        })
        .collect();
    RepoSnapshot {
        slug: slug.to_string(),
        default_branch: default_branch.to_string(),
        workflows,
    }
}

/// Load a [`FleetSnapshot`] from an offline fixture (`simard ci-health
/// --from-json <path>`). The fixture shape mirrors [`FleetSnapshot`] using the
/// same GitHub state/conclusion strings the live collector reads.
pub fn snapshot_from_fixture(json: &[u8]) -> SimardResult<FleetSnapshot> {
    let raw: RawFixtureFleet =
        serde_json::from_slice(json).map_err(|e| SimardError::CiHealthGhCommandFailed {
            reason: format!("failed to parse CI-health fixture JSON: {e}"),
        })?;
    let repos = raw
        .repos
        .into_iter()
        .map(|r| RepoSnapshot {
            slug: r.slug,
            default_branch: r.default_branch,
            workflows: r
                .workflows
                .into_iter()
                .map(|w| WorkflowSnapshot {
                    name: w.name,
                    state: WorkflowState::parse(&w.state),
                    latest_run: w.latest_run.map(|run| WorkflowRun {
                        status: run.status,
                        conclusion: run.conclusion.as_deref().and_then(conclusion_from_gh),
                        event: run.event,
                        created_at: run.created_at,
                        database_id: run.database_id,
                    }),
                })
                .collect(),
        })
        .collect();
    Ok(FleetSnapshot { repos })
}

/// Abstract the three `gh` reads the sweep needs, so [`collect_fleet`] is
/// testable with a fake client.
pub trait GhWorkflowClient {
    fn default_branch(&self, repo: &str) -> SimardResult<String>;
    fn list_workflows(&self, repo: &str) -> SimardResult<Vec<RawWorkflowRow>>;
    fn list_runs(&self, repo: &str, branch: &str) -> SimardResult<Vec<RawRunRow>>;
}

/// Collect a live snapshot of every governed repo. Fail-loud: any `gh` error
/// aborts the sweep rather than silently reporting a partial fleet as green.
pub fn collect_fleet(gh: &dyn GhWorkflowClient, repos: &[&str]) -> SimardResult<FleetSnapshot> {
    let mut out = Vec::with_capacity(repos.len());
    for &repo in repos {
        let branch = gh.default_branch(repo)?;
        let workflows = gh.list_workflows(repo)?;
        let runs = gh.list_runs(repo, &branch)?;
        out.push(build_repo_snapshot(repo, &branch, &workflows, &runs));
    }
    Ok(FleetSnapshot { repos: out })
}

/// Production [`GhWorkflowClient`] that shells out to the `gh` binary.
#[derive(Default)]
pub struct RealGhWorkflowClient;

impl RealGhWorkflowClient {
    pub fn new() -> Self {
        Self
    }

    fn run_gh(args: &[&str]) -> SimardResult<Vec<u8>> {
        let output = std::process::Command::new("gh")
            .args(args)
            .output()
            .map_err(|e| SimardError::CiHealthGhCommandFailed {
                reason: format!("failed to spawn `gh {}`: {e}", args.join(" ")),
            })?;
        if !output.status.success() {
            return Err(SimardError::CiHealthGhCommandFailed {
                reason: format!(
                    "`gh {}` exited {}: {}",
                    args.join(" "),
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        Ok(output.stdout)
    }
}

impl GhWorkflowClient for RealGhWorkflowClient {
    fn default_branch(&self, repo: &str) -> SimardResult<String> {
        let out = Self::run_gh(&[
            "repo",
            "view",
            repo,
            "--json",
            "defaultBranchRef",
            "-q",
            ".defaultBranchRef.name",
        ])?;
        let branch = String::from_utf8_lossy(&out).trim().to_string();
        if branch.is_empty() {
            return Err(SimardError::CiHealthGhCommandFailed {
                reason: format!("`gh repo view {repo}` returned an empty default branch"),
            });
        }
        Ok(branch)
    }

    fn list_workflows(&self, repo: &str) -> SimardResult<Vec<RawWorkflowRow>> {
        let out = Self::run_gh(&[
            "workflow",
            "list",
            "-R",
            repo,
            "--all",
            "--json",
            "name,state",
        ])?;
        parse_workflow_rows(&out)
    }

    fn list_runs(&self, repo: &str, branch: &str) -> SimardResult<Vec<RawRunRow>> {
        let out = Self::run_gh(&[
            "run",
            "list",
            "-R",
            repo,
            "--branch",
            branch,
            "--limit",
            "200",
            "--json",
            "workflowName,status,conclusion,event,createdAt,databaseId",
        ])?;
        parse_run_rows(&out)
    }
}
