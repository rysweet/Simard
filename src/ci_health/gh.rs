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

use super::cache::GreenShaCache;
use super::types::{
    FleetSnapshot, RepoSnapshot, RunConclusion, WorkflowRun, WorkflowSnapshot, WorkflowState,
};

/// One row of `gh workflow list -R <repo> --all --json name,state,id`.
#[derive(Clone, Debug, Deserialize)]
pub struct RawWorkflowRow {
    pub name: String,
    pub state: String,
    /// Workflow id, used to fetch a specific workflow's latest run directly
    /// (`gh run list --workflow <id>`).
    pub id: u64,
}

/// One row of `gh run list -R <repo> --branch <b> --json
/// workflowName,workflowDatabaseId,status,conclusion,event,createdAt,databaseId`.
#[derive(Clone, Debug, Deserialize)]
pub struct RawRunRow {
    #[serde(rename = "workflowName")]
    pub workflow_name: String,
    /// The run's parent workflow id. Runs are keyed to workflows by this
    /// unique id rather than the (non-unique) display name.
    #[serde(rename = "workflowDatabaseId")]
    pub workflow_database_id: u64,
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
    head_sha: String,
    #[serde(default)]
    workflows: Vec<RawFixtureWorkflow>,
}

#[derive(Clone, Debug, Deserialize)]
struct RawFixtureFleet {
    repos: Vec<RawFixtureRepo>,
}

/// Parse the JSON array from `gh workflow list ... --json name,state,id`.
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

/// Reduce a run list to the newest run per workflow, keyed by the workflow's
/// unique id (`workflowDatabaseId`) rather than its non-unique display name.
/// ISO-8601 UTC timestamps sort chronologically as strings, so a lexicographic
/// max picks the latest run.
pub fn latest_run_by_workflow(rows: &[RawRunRow]) -> HashMap<u64, RawRunRow> {
    let mut latest: HashMap<u64, RawRunRow> = HashMap::new();
    for row in rows {
        match latest.get(&row.workflow_database_id) {
            Some(existing) if existing.created_at >= row.created_at => {}
            _ => {
                latest.insert(row.workflow_database_id, row.clone());
            }
        }
    }
    latest
}

/// Join workflow rows to their latest run into a [`RepoSnapshot`]. Pure — this
/// is the core of the collector and is unit-tested directly. Runs are matched
/// to workflows by id, so two workflows sharing a display name never collapse.
pub fn build_repo_snapshot(
    slug: &str,
    default_branch: &str,
    head_sha: &str,
    workflows: &[RawWorkflowRow],
    runs: &[RawRunRow],
) -> RepoSnapshot {
    let latest = latest_run_by_workflow(runs);
    let workflows = workflows
        .iter()
        .map(|wf| WorkflowSnapshot {
            name: wf.name.clone(),
            state: WorkflowState::parse(&wf.state),
            latest_run: latest.get(&wf.id).map(run_from_row),
        })
        .collect();
    RepoSnapshot {
        slug: slug.to_string(),
        default_branch: default_branch.to_string(),
        head_sha: head_sha.to_string(),
        green_from_cache: false,
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
            head_sha: r.head_sha,
            green_from_cache: false,
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

/// Abstract the `gh` reads the sweep needs, so [`collect_fleet`] is testable
/// with a fake client.
pub trait GhWorkflowClient {
    fn default_branch(&self, repo: &str) -> SimardResult<String>;
    /// The current head commit SHA of `branch` (`gh api
    /// repos/<owner>/<repo>/commits/<branch> --jq .sha`). This is the key the
    /// last-known-green cache uses to decide whether a repo can be skipped.
    fn head_sha(&self, repo: &str, branch: &str) -> SimardResult<String>;
    fn list_workflows(&self, repo: &str) -> SimardResult<Vec<RawWorkflowRow>>;
    fn list_runs(&self, repo: &str, branch: &str) -> SimardResult<Vec<RawRunRow>>;
    /// The single latest run of one workflow (by id) on `branch`, or `None`
    /// when it has never run there. Used as a fallback for workflows whose
    /// latest run fell outside the branch-wide window fetched by [`list_runs`].
    fn latest_run(
        &self,
        repo: &str,
        branch: &str,
        workflow_id: u64,
    ) -> SimardResult<Option<RawRunRow>>;
}

/// Collect a live snapshot of every governed repo. Fail-loud: any `gh` error
/// aborts the sweep rather than silently reporting a partial fleet as green.
///
/// ## Last-known-green short-circuit
///
/// For each repo the collector first resolves the default branch and its head
/// SHA (two cheap `gh` calls). If `cache` records that repo as green at exactly
/// this head SHA, the expensive per-workflow collection (workflow list + run
/// window + per-workflow fallbacks) is **skipped** and the repo is emitted as
/// [`RepoSnapshot::green_from_cache`] with no workflows. Because a repo is only
/// ever *recorded* green when all its active workflows are commit-driven (see
/// [`super::classify::repo_cacheable`]), an unchanged head SHA proves no active
/// workflow has run since the green verdict, so skipping is sound. Pass an empty
/// cache to force a full sweep of every repo.
///
/// `list_runs` fetches a branch-wide window (the N most-recent runs across all
/// workflows). If a workflow appears in that window at all, the newest row for
/// it is necessarily its true latest run. The only gap is an **active**
/// workflow with *zero* rows in the window (its runs are all older than the
/// window) — that would otherwise look like `NoRun` and be silently ignored,
/// hiding a stale failing run. For exactly those workflows we query the latest
/// run directly so a truncated window can never be reported as green.
pub fn collect_fleet(
    gh: &dyn GhWorkflowClient,
    repos: &[&str],
    cache: &GreenShaCache,
) -> SimardResult<FleetSnapshot> {
    let mut out = Vec::with_capacity(repos.len());
    for &repo in repos {
        let branch = gh.default_branch(repo)?;
        let head_sha = gh.head_sha(repo, &branch)?;
        if cache.is_green(repo, &head_sha) {
            out.push(RepoSnapshot {
                slug: repo.to_string(),
                default_branch: branch,
                head_sha,
                green_from_cache: true,
                workflows: Vec::new(),
            });
            continue;
        }
        let workflows = gh.list_workflows(repo)?;
        let runs = gh.list_runs(repo, &branch)?;
        let mut snapshot = build_repo_snapshot(repo, &branch, &head_sha, &workflows, &runs);
        for (row, wf) in workflows.iter().zip(snapshot.workflows.iter_mut()) {
            let disabled = WorkflowState::parse(&row.state).is_disabled();
            if !disabled
                && wf.latest_run.is_none()
                && let Some(run) = gh.latest_run(repo, &branch, row.id)?
            {
                wf.latest_run = Some(run_from_row(&run));
            }
        }
        out.push(snapshot);
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

    fn head_sha(&self, repo: &str, branch: &str) -> SimardResult<String> {
        // `gh repo view --json defaultBranchRef` does not expose the target OID,
        // so read the branch head commit SHA via the REST API instead.
        let endpoint = format!("repos/{repo}/commits/{branch}");
        let out = Self::run_gh(&["api", &endpoint, "--jq", ".sha"])?;
        let sha = String::from_utf8_lossy(&out).trim().to_string();
        if sha.is_empty() {
            return Err(SimardError::CiHealthGhCommandFailed {
                reason: format!(
                    "`gh api {endpoint}` returned an empty head SHA for {repo}@{branch}"
                ),
            });
        }
        Ok(sha)
    }

    fn list_workflows(&self, repo: &str) -> SimardResult<Vec<RawWorkflowRow>> {
        let out = Self::run_gh(&[
            "workflow",
            "list",
            "-R",
            repo,
            "--all",
            "--json",
            "name,state,id",
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
            "workflowName,workflowDatabaseId,status,conclusion,event,createdAt,databaseId",
        ])?;
        parse_run_rows(&out)
    }

    fn latest_run(
        &self,
        repo: &str,
        branch: &str,
        workflow_id: u64,
    ) -> SimardResult<Option<RawRunRow>> {
        let id = workflow_id.to_string();
        let out = Self::run_gh(&[
            "run",
            "list",
            "-R",
            repo,
            "--branch",
            branch,
            "--workflow",
            &id,
            "--limit",
            "1",
            "--json",
            "workflowName,workflowDatabaseId,status,conclusion,event,createdAt,databaseId",
        ])?;
        Ok(parse_run_rows(&out)?.into_iter().next())
    }
}
