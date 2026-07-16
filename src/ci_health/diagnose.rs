//! Root-cause diagnosis for a CI-health actionable failure — the
//! "diagnose root cause" half of the standing CI-health stewardship goal.
//!
//! The sweep ([`build_report`](super::build_report)) *detects* a broken
//! default-branch workflow and [`steward`](super::steward) *tracks* it as a
//! deduplicated issue. On its own that issue only records the failing
//! *conclusion*; a human (or a downstream `ci-diagnostic` fixer) still has to
//! open the run and hunt for what actually broke. This module closes that gap:
//! given a failing run id it reads the run's jobs and distills the **failing
//! jobs and their failing steps** into a compact, embeddable Root-cause block.
//!
//! Design:
//! - **Structured, not log-scraped.** Diagnosis reads `gh run view <id> --json
//!   jobs`, whose `jobs[].conclusion` / `jobs[].steps[].conclusion` name the
//!   failing job and step directly. No fragile log parsing.
//! - **Best-effort, never blocks tracking.** Filing the tracking issue is the
//!   correctness-critical act; a diagnosis fetch that fails must not stop it.
//!   [`steward`](super::steward) therefore treats a diagnosis error as
//!   "unavailable" and *writes that fact into the issue* rather than degrading
//!   silently — the same fail-loud spirit, applied to enrichment.
//! - **Only for genuinely-new issues.** The steward fetches diagnosis solely
//!   when it is about to file a new issue (after dedup search found none), so a
//!   re-swept, already-tracked failure costs zero extra `gh` calls.
//!
//! See `docs/reference/ci-health-sweep.md`.

use serde::Deserialize;

use crate::error::{SimardError, SimardResult};

use super::types::RunConclusion;

/// One failing job of a run, with the names of the steps that failed inside it.
/// A job whose steps all "succeeded" but which itself failed (e.g. a job that
/// `timed_out`, or failed at setup/teardown) yields an empty `failed_steps`;
/// its own `conclusion` is retained so that case is rendered factually rather
/// than guessed at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedJob {
    pub name: String,
    /// The job's own failing conclusion (`failure` / `timed_out` /
    /// `startup_failure`), echoed verbatim so a stepless failure is described
    /// by what GitHub reported, not by speculation.
    pub conclusion: String,
    pub failed_steps: Vec<String>,
}

/// The distilled root cause of a failing run: every failing job and its failing
/// steps, in `gh`'s reported order. `run_id` is echoed so the rendered block is
/// self-describing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunDiagnosis {
    pub run_id: u64,
    pub failed_jobs: Vec<FailedJob>,
}

impl RunDiagnosis {
    /// True when no failing job was found. A run can conclude `failure` yet
    /// expose no failing job (e.g. it failed before any job started); callers
    /// render this case as an explicit "no failing job identified" note rather
    /// than an empty section.
    pub fn is_empty(&self) -> bool {
        self.failed_jobs.is_empty()
    }

    /// Render the diagnosis as a compact Markdown block for embedding in a
    /// tracking issue. `run_url` is linked so a reader can jump straight to the
    /// run. Deterministic (no time, no I/O) so it is exhaustively testable.
    pub fn render(&self, run_url: &str) -> String {
        let mut out = String::new();
        out.push_str("## Root cause\n\n");
        if self.is_empty() {
            out.push_str(&format!(
                "No failing job/step was identified in [run {}]({}) — it may have \
                 failed before any job started (e.g. a workflow/setup error). Open \
                 the run to investigate.\n",
                self.run_id, run_url
            ));
            return out;
        }
        out.push_str(&format!(
            "Failing jobs and steps in [run {}]({}):\n\n",
            self.run_id, run_url
        ));
        for job in &self.failed_jobs {
            if job.failed_steps.is_empty() {
                out.push_str(&format!(
                    "- job `{}` concluded `{}` (no individual step reported failing)\n",
                    job.name, job.conclusion
                ));
            } else {
                for step in &job.failed_steps {
                    out.push_str(&format!("- job `{}` \u{2192} step `{}`\n", job.name, step));
                }
            }
        }
        out
    }
}

/// One row of `gh run view <id> --json jobs` → `jobs[]`.
#[derive(Clone, Debug, Deserialize)]
struct RawJob {
    name: String,
    /// `""`/absent until the job completes.
    #[serde(default)]
    conclusion: String,
    #[serde(default)]
    steps: Vec<RawStep>,
}

/// One row of `jobs[].steps[]`.
#[derive(Clone, Debug, Deserialize)]
struct RawStep {
    name: String,
    #[serde(default)]
    conclusion: String,
}

/// The `{ "jobs": [...] }` envelope `gh run view --json jobs` emits. `jobs` is
/// **required**: `gh run view --json jobs` always includes it, so a response
/// missing the key is an unexpected/malformed shape and is surfaced as a parse
/// error (→ "diagnosis unavailable") rather than being silently read as an
/// empty, falsely-clean diagnosis. A present-but-empty `"jobs": []` remains the
/// legitimate "run failed before any job started" case.
#[derive(Clone, Debug, Deserialize)]
struct RawJobsEnvelope {
    jobs: Vec<RawJob>,
}

/// A conclusion string is a genuine failure worth surfacing as root cause. The
/// same set the sweep treats as actionable ([`RunConclusion::is_actionable_failure`]);
/// keeping them aligned means the diagnosis names exactly the jobs/steps whose
/// failure made the workflow actionable, and non-failures (`cancelled`,
/// `skipped`, `success`, `neutral`, …) are never mistaken for the root cause.
fn is_failing(conclusion: &str) -> bool {
    !conclusion.is_empty() && RunConclusion::parse(conclusion).is_actionable_failure()
}

/// Parse `gh run view <id> --json jobs` output into a [`RunDiagnosis`]: keep
/// only jobs that failed, and within each the steps that failed. Pure — this is
/// the testable core of diagnosis and never touches the network.
pub fn parse_run_diagnosis(run_id: u64, json: &[u8]) -> SimardResult<RunDiagnosis> {
    let env: RawJobsEnvelope =
        serde_json::from_slice(json).map_err(|e| SimardError::CiHealthGhCommandFailed {
            reason: format!("failed to parse `gh run view --json jobs` JSON: {e}"),
        })?;
    let failed_jobs = env
        .jobs
        .into_iter()
        .filter(|job| is_failing(&job.conclusion))
        .map(|job| FailedJob {
            name: job.name,
            conclusion: job.conclusion,
            failed_steps: job
                .steps
                .into_iter()
                .filter(|s| is_failing(&s.conclusion))
                .map(|s| s.name)
                .collect(),
        })
        .collect();
    Ok(RunDiagnosis {
        run_id,
        failed_jobs,
    })
}

/// Abstract the single `gh` read root-cause diagnosis needs, so the steward's
/// issue-body enrichment is testable with a fake.
pub trait RunDiagnostics {
    /// Diagnose the failing `run_id` in `repo`. Fail-loud: a `gh` error
    /// propagates so the caller can record diagnosis as *unavailable* (rather
    /// than emitting an empty, falsely-clean Root-cause block).
    fn diagnose(&self, repo: &str, run_id: u64) -> SimardResult<RunDiagnosis>;
}

/// Production [`RunDiagnostics`] that shells out to `gh run view --json jobs`.
#[derive(Default)]
pub struct RealGhRunDiagnostics;

impl RealGhRunDiagnostics {
    pub fn new() -> Self {
        Self
    }
}

impl RunDiagnostics for RealGhRunDiagnostics {
    fn diagnose(&self, repo: &str, run_id: u64) -> SimardResult<RunDiagnosis> {
        let id = run_id.to_string();
        let output = std::process::Command::new("gh")
            .args(["run", "view", &id, "-R", repo, "--json", "jobs"])
            .output()
            .map_err(|e| SimardError::CiHealthGhCommandFailed {
                reason: format!("failed to spawn `gh run view {id} -R {repo} --json jobs`: {e}"),
            })?;
        if !output.status.success() {
            return Err(SimardError::CiHealthGhCommandFailed {
                reason: format!(
                    "`gh run view {id} -R {repo} --json jobs` exited {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        parse_run_diagnosis(run_id, &output.stdout)
    }
}
