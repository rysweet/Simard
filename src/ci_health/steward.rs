//! Bridge from a CI-health [`FleetReport`]'s actionable failures to the
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

use std::collections::BTreeMap;

use crate::error::SimardResult;
use crate::stewardship::{GhClient, StewardshipOutcome, failure_signature, find_existing};

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

/// The distinct-failure signature for a CI actionable failure: keyed on
/// `<repo> :: <workflow>` (see module docs). The volatile run id/url and the
/// specific failing conclusion are deliberately excluded so the same broken
/// workflow hashes identically across sweeps and across a `failure`/`timed_out`
/// flap.
pub fn ci_failure_signature(failure: &ActionableFailure) -> String {
    let identity = format!("{} :: {}", failure.repo, failure.workflow);
    failure_signature(CI_FAILURE_KIND, &identity)
}

/// Issue body embedding the dedup front-matter (matching the stewardship
/// contract) plus CI-health specifics for triage.
fn issue_body(failure: &ActionableFailure, signature: &str) -> String {
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
    )
}

/// File a deduplicated tracking issue for each **distinct** actionable failure
/// in `report`, in the failing repo itself.
///
/// Distinct failures (by [`ci_failure_signature`]) are processed once each: for
/// each, search the repo for an open issue already carrying that signature; if
/// found → [`StewardshipOutcome::MatchedExisting`], else file a new issue →
/// [`StewardshipOutcome::FiledNew`]. A `gh` error on the search propagates and
/// **no** issue is filed for that signature — the same fail-loud rule as
/// orchestrator stewardship (never file while a search is degraded).
///
/// Returns one outcome per distinct signature, in stable signature order. A
/// green report (no actionable failures) yields an empty vector without
/// touching `gh`.
pub fn file_issues_for_report(
    report: &FleetReport,
    gh: &dyn GhClient,
) -> SimardResult<Vec<StewardshipOutcome>> {
    // Collapse to one representative failure per distinct signature so two
    // workflows that hash identically (e.g. two workflow files sharing a
    // `name:`) or a repeated signature never file two issues in one sweep.
    let mut distinct: BTreeMap<String, &ActionableFailure> = BTreeMap::new();
    for failure in &report.actionable_failures {
        distinct
            .entry(ci_failure_signature(failure))
            .or_insert(failure);
    }

    let mut outcomes = Vec::with_capacity(distinct.len());
    for (signature, failure) in distinct {
        let existing = gh.search_issues(&failure.repo, &signature)?;
        if let Some(issue) = find_existing(&existing, &signature) {
            outcomes.push(StewardshipOutcome::MatchedExisting {
                repo: failure.repo.clone(),
                issue_number: issue.number,
                url: issue.url.clone(),
                signature,
            });
            continue;
        }
        let title = issue_title(failure);
        let body = issue_body(failure, &signature);
        let new = gh.create_issue(&failure.repo, &title, &body)?;
        outcomes.push(StewardshipOutcome::FiledNew {
            repo: failure.repo.clone(),
            issue_number: new.number,
            url: new.url,
            signature,
        });
    }
    Ok(outcomes)
}
