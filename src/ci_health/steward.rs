//! Route a CI-health [`FleetReport`]'s actionable failures into the
//! deduplicated GitHub-issue pipeline — the "dedupe to one issue/PR per
//! distinct failure" half of the standing CI-health stewardship goal.
//!
//! The CI-health sweep ([`build_report`](super::build_report)) *detects* every
//! active default-branch workflow that genuinely failed. This module *routes*
//! those failures into tracked, deduplicated issues, reusing the stewardship
//! dedup contract (`stewardship-signature:` body front-matter + `gh issue list
//! --search`) so an already-tracked broken workflow is never re-filed.
//!
//! Design:
//! - **Distinct-failure identity.** A distinct CI failure is one broken
//!   *workflow on a repo* — keyed by `<repo> :: <workflow>`, independent of the
//!   volatile run id/url and of which failing conclusion (`failure` /
//!   `timed_out` / `startup_failure`) it happens to carry this sweep. That
//!   yields exactly one issue per broken workflow, matching the goal's "one
//!   issue/PR per distinct failure."
//! - **Target repo is the failing repo itself.** Unlike orchestrator-failure
//!   routing, a CI failure's repo is already known (it is a governed repo), so
//!   no routing matrix is consulted.
//! - **Reuse, don't fork.** Signature hashing, issue search, and the
//!   `stewardship-signature:` body contract come from [`crate::stewardship`];
//!   this module only adapts the CI failure into that contract.
//!
//! See `docs/reference/ci-health-sweep.md`.

use std::collections::{BTreeMap, HashSet};
use std::process::Command;

use crate::error::{SimardError, SimardResult};
use crate::stewardship::{GhClient, GhIssue, StewardshipOutcome, failure_signature, find_existing};

use super::classify::RepoReport;
use super::diagnose::RunDiagnostics;
use super::{ActionableFailure, FleetReport};

/// Stable `failure_kind` component for every CI-health-originated signature, so
/// these never collide with an orchestrator-failure signature that happened to
/// hash the same normalized text.
const CI_FAILURE_KIND: &str = "ci_workflow_failure";

/// Human-facing issue title for a broken workflow. Dedup keys on the body
/// signature (not the title), so a descriptive title is safe.
fn issue_title(failure: &ActionableFailure) -> String {
    format!(
        "[ci-health] {} failing on {}",
        failure.workflow, failure.repo
    )
}

/// The distinct-failure signature for a `<repo> :: <workflow>` identity — the
/// stable key shared by *filing* a broken workflow and *resolving* it once it is
/// green again. Keyed only on repo+workflow (see module docs): the volatile run
/// id/url and the specific failing conclusion are deliberately excluded so the
/// same workflow hashes identically across sweeps, across a `failure`/`timed_out`
/// flap, and across the failure→green transition.
pub fn ci_signature_for(repo: &str, workflow: &str) -> String {
    let identity = format!("{repo} :: {workflow}");
    failure_signature(CI_FAILURE_KIND, &identity)
}

/// The distinct-failure signature for a CI actionable failure. Thin wrapper over
/// [`ci_signature_for`] so a failure and its later green result hash identically.
pub fn ci_failure_signature(failure: &ActionableFailure) -> String {
    ci_signature_for(&failure.repo, &failure.workflow)
}

/// Issue body embedding the dedup front-matter (matching the stewardship
/// contract) plus CI-health specifics for triage. `root_cause` is the rendered
/// Root-cause block (see [`diagnose_block`]); it is embedded verbatim so an
/// unavailable diagnosis is *visible in the issue* rather than silently absent.
fn issue_body(failure: &ActionableFailure, signature: &str, root_cause: &str) -> String {
    let run_url = failure.run_url.as_deref().unwrap_or("unknown");
    format!(
        "filed-by: simard-stewardship\n\
         stewardship-signature: {sig}\n\
         ci-health-repo: {repo}\n\
         ci-health-workflow: {workflow}\n\
         default-branch: {branch}\n\
         latest-conclusion: {conclusion}\n\
         latest-run: {run_url}\n\
         \n\
         ## CI-health actionable failure\n\
         \n\
         The `{workflow}` workflow's latest run on `{repo}`@`{branch}` \
         concluded `{conclusion}`.\n\
         \n\
         {root_cause}\
         \n\
         Filed by Simard's CI-health steward. This issue tracks the broken \
         workflow until its default-branch CI is green again; re-sweeps with \
         `simard ci-health --file-issues` dedupe against the \
         `stewardship-signature` above instead of filing a new issue.\n",
        sig = signature,
        repo = failure.repo,
        workflow = failure.workflow,
        branch = failure.default_branch,
        conclusion = failure.conclusion,
        run_url = run_url,
        root_cause = root_cause,
    )
}

/// Build the Root-cause Markdown block for a failure, best-effort. Filing the
/// tracking issue is the correctness-critical act, so a diagnosis that cannot
/// be fetched must never abort it; instead the block records *why* it is
/// unavailable (no silent degradation). Returns a block ending in a newline.
fn diagnose_block(failure: &ActionableFailure, diag: &dyn RunDiagnostics) -> String {
    // When the specific run URL was not captured, fall back to the repo's
    // Actions page so the block always offers a real place to investigate.
    let run_url = failure
        .run_url
        .clone()
        .unwrap_or_else(|| format!("https://github.com/{}/actions", failure.repo));
    let Some(run_id) = failure.run_id else {
        return format!(
            "## Root cause\n\nDiagnosis unavailable: the failing run's id was not captured \
             this sweep. Investigate the workflow's runs: {run_url}\n"
        );
    };
    match diag.diagnose(&failure.repo, run_id) {
        Ok(diagnosis) => diagnosis.render(&run_url),
        Err(e) => format!(
            "## Root cause\n\nDiagnosis unavailable: could not read the failing run's jobs \
             ({e}). Open the run to investigate: {run_url}\n"
        ),
    }
}

/// A distinct actionable failure (or a whole repo, during resolution) that the
/// sweep could **not** reconcile because its token lacks write access to that
/// repo — a cross-repo `issues:write` denial, as when a governed *sibling*
/// repo is red but `STEWARD_GH_TOKEN` is absent so the sweep falls back to the
/// repo-scoped default `GITHUB_TOKEN`.
///
/// Recorded and reported — never silently dropped (the sibling's red CI still
/// appears in the sweep report and this skip is printed loudly) — so a single
/// unwritable repo no longer aborts the entire sweep. Aborting used to turn
/// Simard's own scheduled `ci-health` run red on *any* sibling failure, which
/// the next sweep then re-detected as a fresh actionable failure: the exact
/// self-referential loop the `--exit-zero` scheduled-sweep contract exists to
/// avoid. Fail-*safe*, not fail-open: the failure is surfaced, just not by
/// crashing the steward.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnauthorizedSkip {
    pub repo: String,
    /// The failing workflow whose tracking issue could not be filed, or `None`
    /// for a repo-level resolution skip (listing/closing that repo's issues).
    pub workflow: Option<String>,
    /// The underlying `gh` error text, so the skip names *why* it was skipped.
    pub reason: String,
}

/// Result of a `--file-issues` filing pass: one [`StewardshipOutcome`] per
/// distinct actionable failure that could be reconciled, plus any failures
/// skipped because the failing repo was not writable (see [`UnauthorizedSkip`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssueFilingReport {
    pub outcomes: Vec<StewardshipOutcome>,
    pub skipped_unauthorized: Vec<UnauthorizedSkip>,
}

/// Whether `err` is a GitHub **authorization** failure — the token lacks the
/// scope required for the *target repo*. This is the well-known, expected
/// outcome of the sweep touching a governed sibling repo with only the
/// repo-scoped default `GITHUB_TOKEN` (no `STEWARD_GH_TOKEN`); it is treated as
/// a reported skip rather than a sweep-aborting operational error. Any *other*
/// `gh` failure (a transient outage, a parse error, a missing binary) is NOT an
/// authorization error and still fails loud, preserving no-silent-degradation
/// for genuinely unexpected failures. Matched case-insensitively against the
/// captured `gh` stderr embedded in the error's `reason`.
///
/// **Deliberately conservative — only *explicit permanent permission-denial*
/// phrasings count as authorization skips:**
///   - GITHUB_TOKEN / GitHub App: `Resource not accessible by integration`
///   - under-scoped PAT:          `Resource not accessible by personal access token`
///   - admin-gated action:        `must have admin rights`
///
/// We intentionally do **not** treat a bare `HTTP 403` / `403 Forbidden` as an
/// authorization denial. GitHub returns 403 for genuinely *transient* conditions
/// too — most notably secondary/abuse **rate limits** (`HTTP 403: You have
/// exceeded a secondary rate limit...`), which a fleet sweep firing bursts of
/// `gh issue create`/`list`/`close` is exactly the workload to trip, as well as
/// proxy/policy 403s. Masking those as a permission skip would silently downgrade
/// a real failure to a skip and leave it untracked while the run stays green —
/// the very silent degradation this steward forbids. So an *unrecognized* 403
/// (anything not carrying one of the explicit phrasings above) fails loud.
fn is_authorization_error(err: &SimardError) -> bool {
    let reason = match err {
        SimardError::StewardshipGhCommandFailed { reason }
        | SimardError::CiHealthGhCommandFailed { reason } => reason.as_str(),
        _ => return false,
    };
    let lower = reason.to_ascii_lowercase();
    lower.contains("not accessible by") || lower.contains("must have admin rights")
}

/// File (or match) the deduplicated tracking issue for a single distinct
/// failure. Split out of [`file_issues_for_report`] so its per-failure `gh`
/// errors can be classified ([`is_authorization_error`]) without unwinding the
/// whole sweep.
fn file_one_failure(
    signature: &str,
    failure: &ActionableFailure,
    gh: &dyn GhClient,
    diag: &dyn RunDiagnostics,
) -> SimardResult<StewardshipOutcome> {
    let existing = gh.search_issues(&failure.repo, signature)?;
    if let Some(issue) = find_existing(&existing, signature) {
        return Ok(StewardshipOutcome::MatchedExisting {
            repo: failure.repo.clone(),
            issue_number: issue.number,
            url: issue.url.clone(),
            signature: signature.to_string(),
        });
    }
    let title = issue_title(failure);
    let root_cause = diagnose_block(failure, diag);
    let body = issue_body(failure, signature, &root_cause);
    let new = gh.create_issue(&failure.repo, &title, &body)?;
    Ok(StewardshipOutcome::FiledNew {
        repo: failure.repo.clone(),
        issue_number: new.number,
        url: new.url,
        signature: signature.to_string(),
    })
}

/// File a deduplicated tracking issue for each **distinct** actionable failure
/// in `report`, in the failing repo itself.
///
/// Distinct failures (by [`ci_failure_signature`]) are processed once each: for
/// each, search the repo for an open issue already carrying that signature; if
/// found → [`StewardshipOutcome::MatchedExisting`], else diagnose the failing
/// run's root cause (best-effort, via `diag`) and file a new issue embedding it
/// → [`StewardshipOutcome::FiledNew`]. A diagnosis error never aborts filing:
/// the issue is still filed, recording that the root cause was unavailable (see
/// [`diagnose_block`]).
///
/// Per-failure `gh` errors are classified: an **authorization** denial (the
/// token cannot write the failing repo — a governed sibling without
/// `STEWARD_GH_TOKEN`) is recorded as an [`UnauthorizedSkip`] and the sweep
/// continues to the next distinct failure, so one unwritable repo never starves
/// filing for the rest of the fleet (Simard's own issues included) nor aborts
/// the scheduled run. Any other `gh` error still propagates (fail-loud — never
/// file, or claim green, while the search/create path is genuinely degraded).
///
/// Returns one outcome per reconciled distinct signature (in stable signature
/// order) plus any skips. A green report (no actionable failures) yields an
/// empty report without touching `gh` or `diag`.
pub fn file_issues_for_report(
    report: &FleetReport,
    gh: &dyn GhClient,
    diag: &dyn RunDiagnostics,
) -> SimardResult<IssueFilingReport> {
    // Collapse to one representative failure per distinct signature so two
    // workflows that hash identically (e.g. two workflow files sharing a
    // `name:`) or a repeated signature never file two issues in one sweep.
    let mut distinct: BTreeMap<String, &ActionableFailure> = BTreeMap::new();
    for failure in &report.actionable_failures {
        distinct
            .entry(ci_failure_signature(failure))
            .or_insert(failure);
    }

    let mut result = IssueFilingReport::default();
    for (signature, failure) in distinct {
        match file_one_failure(&signature, failure, gh, diag) {
            Ok(outcome) => result.outcomes.push(outcome),
            Err(e) if is_authorization_error(&e) => {
                result.skipped_unauthorized.push(UnauthorizedSkip {
                    repo: failure.repo.clone(),
                    workflow: Some(failure.workflow.clone()),
                    reason: e.to_string(),
                });
            }
            Err(e) => return Err(e),
        }
    }
    Ok(result)
}

// ── Resolution: close a tracking issue once its workflow is green again ──────

/// Outcome of resolving (closing) a CI-health tracking issue whose workflow has
/// returned to green. One per issue actually closed; a green workflow with no
/// open tracking issue produces nothing (it was never broken, or was already
/// resolved), so this vector holds only real state transitions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolutionOutcome {
    pub repo: String,
    pub workflow: String,
    pub issue_number: u64,
    pub url: String,
    pub signature: String,
}

/// The reads+write root-cause *resolution* needs: list a repo's open ci-health
/// tracking issues **once**, and close the ones whose workflow is green again.
/// Factored into its own trait (rather than extended onto [`GhClient`], which
/// many modules implement) so the resolution surface has exactly one production
/// impl and one test fake.
pub trait CiIssueResolver {
    /// List the **open** ci-health tracking issues in `repo` — those carrying the
    /// `ci-health-workflow:` marker this steward files. One `gh` call per repo,
    /// so a healthy fleet is scanned in O(repos) requests rather than
    /// O(green-workflows); resolution then matches signatures locally.
    fn list_open_tracking_issues(&self, repo: &str) -> SimardResult<Vec<GhIssue>>;
    /// Close issue `number` in `repo`, posting `comment` as the closing comment.
    /// Fail-loud: a `gh` error propagates so a degraded close is never mistaken
    /// for a resolved issue.
    fn close_issue(&self, repo: &str, number: u64, comment: &str) -> SimardResult<()>;
}

/// Upper bound on open issues fetched per repo when resolving. The REST
/// issue-list endpoint (`gh issue list`, no `--search`) is paged internally by
/// `gh` up to this many issues; it is sized comfortably above the governed
/// fleet's per-repo open-issue volumes so a real tracking issue is never
/// truncated away, while still bounding work on an unexpectedly huge repo.
const OPEN_ISSUE_LIST_LIMIT: usize = 1000;

/// The unique body marker every CI-health tracking issue carries (see
/// [`issue_body`]). Substring-matched **locally** to select this steward's
/// tracking issues from a repo's open issues — GitHub's tokenizing issue search
/// splits `ci-health-workflow` into separate words and so cannot select them
/// reliably (nor bound the result to them), which is why resolution filters in
/// process rather than via a `--search` qualifier.
const CI_HEALTH_ISSUE_MARKER: &str = "ci-health-workflow:";

/// Whether `body` belongs to a CI-health tracking issue this steward filed.
pub(crate) fn is_ci_health_tracking_issue(body: &str) -> bool {
    body.contains(CI_HEALTH_ISSUE_MARKER)
}

/// Production [`CiIssueResolver`] that shells out to `gh`.
#[derive(Default)]
pub struct RealCiIssueResolver;

impl RealCiIssueResolver {
    pub fn new() -> Self {
        Self
    }
}

impl CiIssueResolver for RealCiIssueResolver {
    fn list_open_tracking_issues(&self, repo: &str) -> SimardResult<Vec<GhIssue>> {
        // Use the REST issue-list endpoint (core rate limit, ~5000/hr) rather
        // than the Search API (~30/min secondary limit): resolution runs once
        // per repo, and a green fleet would otherwise burn a search call per
        // green workflow. We over-fetch the repo's open issues (bounded by
        // OPEN_ISSUE_LIST_LIMIT) and filter locally to this steward's tracking
        // issues by their unique body marker — GitHub's tokenizing search cannot
        // select them reliably, so a `--search` pre-filter would both miss real
        // issues past its window on a busy repo and return false positives.
        let limit = OPEN_ISSUE_LIST_LIMIT.to_string();
        let output = Command::new("gh")
            .args([
                "issue",
                "list",
                "-R",
                repo,
                "--state",
                "open",
                "--limit",
                &limit,
                "--json",
                "number,url,title,body",
            ])
            .output()
            .map_err(|e| SimardError::CiHealthGhCommandFailed {
                reason: format!("failed to spawn `gh issue list -R {repo}`: {e}"),
            })?;
        if !output.status.success() {
            return Err(SimardError::CiHealthGhCommandFailed {
                reason: format!(
                    "`gh issue list -R {repo}` exited {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        let all = parse_tracking_issue_list(&output.stdout)?;
        Ok(all
            .into_iter()
            .filter(|issue| is_ci_health_tracking_issue(&issue.body))
            .collect())
    }

    fn close_issue(&self, repo: &str, number: u64, comment: &str) -> SimardResult<()> {
        let num = number.to_string();
        let output = Command::new("gh")
            .args([
                "issue",
                "close",
                &num,
                "-R",
                repo,
                "--comment",
                comment,
                "--reason",
                "completed",
            ])
            .output()
            .map_err(|e| SimardError::CiHealthGhCommandFailed {
                reason: format!("failed to spawn `gh issue close {num} -R {repo}`: {e}"),
            })?;
        if !output.status.success() {
            return Err(SimardError::CiHealthGhCommandFailed {
                reason: format!(
                    "`gh issue close {num} -R {repo}` exited {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        Ok(())
    }
}

/// The closing comment posted when a broken workflow is green again. Deterministic
/// (no time, no I/O) so it is exhaustively testable. Links the green run when its
/// id was captured, else names the default-branch run generically.
fn resolution_comment(repo: &str, workflow: &str, branch: &str, run_id: Option<u64>) -> String {
    let green_evidence = match run_id {
        Some(id) => format!(
            "[latest run](https://github.com/{repo}/actions/runs/{id})",
            repo = repo,
            id = id
        ),
        None => "its latest default-branch run".to_string(),
    };
    format!(
        "\u{2705} Resolved by Simard's CI-health steward.\n\
         \n\
         The `{workflow}` workflow's {green_evidence} on `{repo}`@`{branch}` \
         default branch is now green, so the failure this issue tracked has \
         cleared. Closing per the tracking contract — this issue tracks the \
         broken workflow only *until its default-branch CI is green again*.\n\
         \n\
         If `{workflow}` fails again, the next `simard ci-health --file-issues` \
         sweep files a fresh tracking issue under the same stewardship signature.\n",
        workflow = workflow,
        green_evidence = green_evidence,
        repo = repo,
        branch = branch,
    )
}

/// Parse `gh issue list --json number,url,title,body` output into [`GhIssue`]s.
/// A malformed response is an error (never a silently-empty list that would make
/// resolution wrongly conclude "no tracking issues to close").
fn parse_tracking_issue_list(stdout: &[u8]) -> SimardResult<Vec<GhIssue>> {
    #[derive(serde::Deserialize)]
    struct RawIssue {
        number: u64,
        url: String,
        title: String,
        body: String,
    }
    let raws: Vec<RawIssue> =
        serde_json::from_slice(stdout).map_err(|e| SimardError::CiHealthGhCommandFailed {
            reason: format!("failed to parse `gh issue list` JSON: {e}"),
        })?;
    Ok(raws
        .into_iter()
        .map(|r| GhIssue {
            number: r.number,
            url: r.url,
            title: r.title,
            body: r.body,
        })
        .collect())
}

/// Result of a `--file-issues` resolution pass: the tracking issues actually
/// closed because their workflow went green, plus any repos skipped because the
/// token could not read/close their issues (see [`UnauthorizedSkip`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssueResolutionReport {
    pub closed: Vec<ResolutionOutcome>,
    pub skipped_unauthorized: Vec<UnauthorizedSkip>,
}

/// A single repo's resolution result: the tracking issues actually closed, plus
/// any per-issue **close** authorization skips. Distinct from a repo-level *list*
/// skip (recorded by the caller when the token cannot even read the repo's
/// issues): here the repo *was* listed and some issues may have closed
/// successfully before/after a per-issue write denial, so those closes must be
/// preserved rather than discarded.
#[derive(Default)]
struct RepoResolution {
    outcomes: Vec<ResolutionOutcome>,
    skipped_unauthorized: Vec<UnauthorizedSkip>,
}

/// Close every open `[ci-health]` tracking issue in `repo` whose workflow is now
/// **green**. Split out of [`resolve_issues_for_report`] so a per-repo
/// authorization denial (an unwritable governed sibling) can be classified and
/// recorded as a skip without unwinding resolution for the rest of the fleet.
/// `failing_signatures` are the signatures still carrying a live actionable
/// failure this sweep (they must never be closed off a same-signature green
/// sibling).
///
/// The initial **list** call fails loud on error (propagated via `?`); the
/// caller classifies a list-level authorization denial as a repo-level skip.
/// Each per-issue **close**, however, is classified *in this loop*: an
/// authorization denial on one close is recorded as a per-workflow skip and
/// resolution continues to the repo's other green workflows, so an earlier
/// successful close is never discarded because a *later* close in the same repo
/// was denied. A non-authorization close error still fails loud.
fn resolve_one_repo(
    repo: &RepoReport,
    failing_signatures: &HashSet<String>,
    resolver: &dyn CiIssueResolver,
) -> SimardResult<RepoResolution> {
    let open_issues = resolver.list_open_tracking_issues(&repo.slug)?;
    if open_issues.is_empty() {
        return Ok(RepoResolution::default());
    }
    // Two green workflow files can share a `name:` and so hash to one
    // signature/issue (filing collapses them identically); track which issue
    // numbers were already handled this repo so such a pair closes the shared
    // issue exactly once rather than double-closing it (or double-skipping it).
    let mut handled_this_repo: HashSet<u64> = HashSet::new();
    let mut result = RepoResolution::default();
    for wf in &repo.workflows {
        if wf.verdict != "green" {
            continue;
        }
        let signature = ci_signature_for(&repo.slug, &wf.name);
        // A green workflow whose signature still carries a live failure this
        // sweep (a same-signature sibling is broken) must not close the
        // shared tracking issue — it is still tracking a real failure.
        if failing_signatures.contains(&signature) {
            continue;
        }
        let Some(issue) = find_existing(&open_issues, &signature) else {
            continue;
        };
        if !handled_this_repo.insert(issue.number) {
            continue;
        }
        let comment = resolution_comment(&repo.slug, &wf.name, &repo.default_branch, wf.run_id);
        match resolver.close_issue(&repo.slug, issue.number, &comment) {
            Ok(()) => result.outcomes.push(ResolutionOutcome {
                repo: repo.slug.clone(),
                workflow: wf.name.clone(),
                issue_number: issue.number,
                url: issue.url.clone(),
                signature,
            }),
            Err(e) if is_authorization_error(&e) => {
                result.skipped_unauthorized.push(UnauthorizedSkip {
                    repo: repo.slug.clone(),
                    workflow: Some(wf.name.clone()),
                    reason: e.to_string(),
                });
            }
            Err(e) => return Err(e),
        }
    }
    Ok(result)
}

/// Close every open `[ci-health]` tracking issue whose workflow is now **green**
/// — the "until its default-branch CI is green again" half of the tracking
/// contract that [`file_issues_for_report`] opens.
///
/// For each freshly-collected repo in `report` (cache-served repos carry no
/// workflow list and are skipped — their green issues are resolved on the next
/// full sweep of that repo), the repo's open ci-health tracking issues are
/// listed **once** (O(repos) `gh` calls, not O(green-workflows)), then each
/// **green** workflow is matched against that list locally by its
/// [`ci_signature_for`] signature. A match is closed with a green-evidence
/// comment and reported. A repo with no open tracking issues costs a single list
/// call and no per-workflow work.
///
/// Conservative by construction: only a workflow verdict of exactly `green`
/// resolves an issue. A workflow that is `ignored` (in-progress, disabled, no
/// run, cancelled/skipped) never closes a tracking issue — an in-flight rerun of
/// a previously-broken workflow keeps its issue open until it *concludes* green.
/// A green workflow whose signature still has a **live actionable failure** this
/// sweep (a same-`name:` sibling file is broken, collapsing to the same
/// signature/issue) is also skipped, so a green sibling never closes an issue
/// that is still tracking a real failure — matching filing's "file if any is
/// broken" rule.
///
/// Authorization denials are recorded as [`UnauthorizedSkip`]s and resolution
/// continues, mirroring filing so one unwritable repo never starves resolution
/// for the rest of the fleet nor aborts the scheduled run. A repo-level **list**
/// denial (the token cannot read the repo's issues) is recorded per-repo
/// (`workflow: None`); a per-issue **close** denial is recorded per-workflow
/// (`workflow: Some`) and does not discard closes that already succeeded in the
/// same repo. Any other `gh` error still propagates (fail-loud — never a silent
/// partial resolution). Returns one [`ResolutionOutcome`] per issue actually
/// closed, in repo-then-workflow order, plus any skips.
pub fn resolve_issues_for_report(
    report: &FleetReport,
    resolver: &dyn CiIssueResolver,
) -> SimardResult<IssueResolutionReport> {
    // Signatures that still have a live actionable failure this sweep. Filing
    // keys a tracking issue on `ci_failure_signature` and files whenever *any*
    // workflow with that signature is broken; resolution must therefore refuse
    // to close a signature that is still failing — otherwise a repo with two
    // workflow *files* sharing a `name:` (which collapse to one signature/issue,
    // yet are classified independently), where one file is green and the other
    // is failing, would have its still-broken tracking issue closed by the green
    // one. That is the exact failure filing is (re-)recording this same sweep,
    // so closing it would flap the issue closed/re-opened. Keying on the same
    // `report.actionable_failures` filing uses guarantees parity.
    let failing_signatures: HashSet<String> = report
        .actionable_failures
        .iter()
        .map(ci_failure_signature)
        .collect();

    let mut result = IssueResolutionReport::default();
    for repo in &report.repos {
        if repo.green_from_cache {
            continue;
        }
        // Cheap pre-check: a repo with no green workflow has nothing to resolve,
        // so skip the list call entirely.
        if !repo.workflows.iter().any(|wf| wf.verdict == "green") {
            continue;
        }
        match resolve_one_repo(repo, &failing_signatures, resolver) {
            Ok(mut repo_res) => {
                result.closed.append(&mut repo_res.outcomes);
                // Per-issue close denials within this repo (some issues may still
                // have closed successfully above — those are preserved).
                result
                    .skipped_unauthorized
                    .append(&mut repo_res.skipped_unauthorized);
            }
            Err(e) if is_authorization_error(&e) => {
                // A repo-level *list* denial: the token cannot even read this
                // repo's issues, so nothing could be resolved. Recorded per-repo
                // (workflow: None) to distinguish it from a per-workflow close
                // denial.
                result.skipped_unauthorized.push(UnauthorizedSkip {
                    repo: repo.slug.clone(),
                    workflow: None,
                    reason: e.to_string(),
                });
            }
            Err(e) => return Err(e),
        }
    }
    Ok(result)
}
