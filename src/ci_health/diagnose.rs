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
//! - **Actual error text, structured too.** Naming *which* job/step failed still
//!   leaves a fixer hunting for *what* broke. So each failing job's GitHub
//!   check-run **failure annotations** (`gh api
//!   repos/{repo}/check-runs/{job_id}/annotations`, kept where
//!   `annotation_level == "failure"`) — which carry the concrete error message
//!   (e.g. `error[E0432]: unresolved import`, `Process completed with exit code
//!   101`) — are embedded into the Root-cause block. A downstream `ci-diagnostic`
//!   fixer (or a human) then sees the real error without opening the run. This is
//!   still structured API data, not scraped logs. It is **bounded** (a capped
//!   number of annotations per job, each truncated) so the issue stays readable,
//!   and **best-effort**: an annotations fetch that fails simply omits them for
//!   that job — it never blocks the diagnosis, which itself never blocks filing.
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
    /// The job's GitHub Actions run id (`gh run view --json jobs` →
    /// `jobs[].databaseId`), which is also the check-run id used to fetch this
    /// job's annotations. `None` when the field was absent (unexpected). Kept so
    /// the diagnosis is self-describing and so `annotations` can be enriched
    /// downstream.
    pub job_id: Option<u64>,
    pub failed_steps: Vec<String>,
    /// The job's **failure-level** check-run annotations, already formatted and
    /// bounded (see [`parse_failure_annotations`]) — the concrete error text
    /// (compiler error, failing assertion, `Process completed with exit code N`,
    /// …). Empty when the job had no failure annotations, or when the annotations
    /// fetch was skipped/failed (best-effort enrichment). The failing job/step
    /// names are shown regardless, so an empty list only means "no extra error
    /// text", never a lost diagnosis.
    pub annotations: Vec<String>,
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
            // The concrete error text for this job (bounded, already formatted),
            // as nested bullets under the job. Empty when GitHub attached no
            // failure annotation or the fetch was skipped/failed — the job/step
            // names above still stand on their own.
            for annotation in &job.annotations {
                out.push_str(&format!("    - {annotation}\n"));
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
    /// The job's run/check id (`databaseId`). Used to fetch this job's
    /// annotations. Absent in older/edge shapes → `None`, which simply skips
    /// annotation enrichment for that job.
    #[serde(rename = "databaseId", default)]
    database_id: Option<u64>,
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
            job_id: job.database_id,
            failed_steps: job
                .steps
                .into_iter()
                .filter(|s| is_failing(&s.conclusion))
                .map(|s| s.name)
                .collect(),
            // Populated separately (a second API call per failing job); parsing
            // the jobs list alone leaves it empty.
            annotations: Vec::new(),
        })
        .collect();
    Ok(RunDiagnosis {
        run_id,
        failed_jobs,
    })
}

// ── Failure annotations: the concrete error text per failing job ─────────────

/// Cap on failure annotations embedded per failing job, so a job that emits many
/// annotations (e.g. one per failing test) cannot bloat the tracking issue. When
/// more exist, the rendered list ends with an explicit "(+N more)" marker so the
/// truncation is visible, never silent.
const MAX_ANNOTATIONS_PER_JOB: usize = 5;

/// Cap on a single annotation message's rendered length (characters). Long
/// messages are truncated with an ellipsis so one verbose annotation cannot
/// dominate the block; the run link in the Root-cause block reaches the full text.
const MAX_ANNOTATION_LEN: usize = 300;

/// One row of `gh api repos/{repo}/check-runs/{id}/annotations`.
#[derive(Clone, Debug, Deserialize)]
struct RawAnnotation {
    #[serde(default)]
    annotation_level: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    start_line: Option<u64>,
}

/// Collapse a possibly multi-line annotation message into a single, length-bounded
/// line so each annotation renders as exactly one bullet. Deterministic.
fn one_line_bounded(message: &str) -> String {
    let collapsed = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > MAX_ANNOTATION_LEN {
        let truncated: String = collapsed.chars().take(MAX_ANNOTATION_LEN).collect();
        format!("{truncated}…")
    } else {
        collapsed
    }
}

/// Format one failure annotation for embedding: its message, prefixed with a
/// `path:line` locus (in backticks) when GitHub reported one meaningful enough
/// to help. The **message itself is rendered plainly** rather than wrapped in
/// backticks: real error text frequently contains backticks (e.g. Rust's
/// ``error[E0432]: unresolved import `foo` ``), and wrapping such a message in an
/// outer inline-code span would produce broken, nested-backtick markdown. Any
/// inline code already present in the message renders on its own. Deterministic
/// and pure.
fn format_annotation(a: &RawAnnotation) -> String {
    let msg = one_line_bounded(&a.message);
    // A `path` of just `.github` (GitHub's placeholder for a workflow-level
    // annotation) with no useful line adds noise, so only prefix a locus when the
    // path is a real file path. A real path + line is a genuine locus worth
    // showing.
    let has_real_path = !a.path.is_empty() && a.path != ".github";
    match (has_real_path, a.start_line) {
        (true, Some(line)) if line > 0 => format!("`{}:{}`: {}", a.path, line, msg),
        (true, _) => format!("`{}`: {}", a.path, msg),
        _ => msg,
    }
}

/// Parse `gh api repos/{repo}/check-runs/{id}/annotations` output into the
/// **failure-level** annotation lines for a job, formatted and bounded. Pure —
/// the testable core of annotation enrichment; it never touches the network.
///
/// Only `annotation_level == "failure"` annotations are kept: `warning`/`notice`
/// annotations (deprecation notices, lint hints) are not the root cause and would
/// only dilute the block. At most [`MAX_ANNOTATIONS_PER_JOB`] are returned; when
/// more failure annotations exist, a final "(+N more failure annotation(s))" line
/// makes the truncation explicit. A malformed response is an error so a caller can
/// treat it as "annotations unavailable" rather than a false "no error text".
pub fn parse_failure_annotations(json: &[u8]) -> SimardResult<Vec<String>> {
    let raws: Vec<RawAnnotation> =
        serde_json::from_slice(json).map_err(|e| SimardError::CiHealthGhCommandFailed {
            reason: format!("failed to parse check-run annotations JSON: {e}"),
        })?;
    let failures: Vec<&RawAnnotation> = raws
        .iter()
        .filter(|a| a.annotation_level == "failure")
        .collect();
    let mut lines: Vec<String> = failures
        .iter()
        .take(MAX_ANNOTATIONS_PER_JOB)
        .map(|a| format_annotation(a))
        .collect();
    if failures.len() > MAX_ANNOTATIONS_PER_JOB {
        let more = failures.len() - MAX_ANNOTATIONS_PER_JOB;
        lines.push(format!("(+{more} more failure annotation(s))"));
    }
    Ok(lines)
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
        let mut diagnosis = parse_run_diagnosis(run_id, &output.stdout)?;
        // Best-effort enrichment: attach each failing job's failure annotations
        // (the concrete error text). A job whose id is unknown, or whose
        // annotations cannot be fetched/parsed, simply keeps an empty list — the
        // job/step names already stand on their own, so enrichment never fails
        // the (correctness-critical) diagnosis.
        for job in &mut diagnosis.failed_jobs {
            if let Some(job_id) = job.job_id {
                job.annotations = fetch_failure_annotations(repo, job_id).unwrap_or_default();
            }
        }
        Ok(diagnosis)
    }
}

/// Fetch and parse a job's failure annotations via `gh api
/// repos/{repo}/check-runs/{job_id}/annotations`. Returns the formatted,
/// bounded failure lines ([`parse_failure_annotations`]). The `Err` case (spawn
/// failed, non-zero exit, or malformed JSON) lets the caller treat annotations
/// as *unavailable* for that job (best-effort) without aborting the diagnosis.
fn fetch_failure_annotations(repo: &str, job_id: u64) -> SimardResult<Vec<String>> {
    let path = format!("repos/{repo}/check-runs/{job_id}/annotations");
    let output = std::process::Command::new("gh")
        .args(["api", &path])
        .output()
        .map_err(|e| SimardError::CiHealthGhCommandFailed {
            reason: format!("failed to spawn `gh api {path}`: {e}"),
        })?;
    if !output.status.success() {
        return Err(SimardError::CiHealthGhCommandFailed {
            reason: format!(
                "`gh api {path}` exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    parse_failure_annotations(&output.stdout)
}
