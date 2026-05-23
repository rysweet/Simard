//! Stewardship loop — autonomous failure → issue → backlog routing for Simard
//! (issue #1167).
//!
//! See `Specs/ProductArchitecture.md` § Stewardship Mode and
//! `docs/concepts/stewardship-mode.md`.
//!
//! Pipeline:
//! 1. Validate the [`OrchestratorRunSummary`] (fail-loud on missing fields).
//! 2. Route `source_module` → [`TargetRepo`] (no default).
//! 3. Compute a noise-stripped [`failure_signature`].
//! 4. Search the target repo for an open issue with that signature.
//! 5. If found → [`StewardshipOutcome::MatchedExisting`].
//!    Otherwise → file a new issue → [`StewardshipOutcome::FiledNew`].
//! 6. Enqueue the resulting issue handle onto the [`GoalBoard`] backlog.

pub mod dedup;
pub mod gh_client;
pub mod merge_authority;
pub mod merge_judge;
pub mod recipe_merge_judge;
pub mod routing;
pub mod types;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_extra;

pub use dedup::{failure_signature, find_existing, normalize};
pub use gh_client::{GhClient, GhIssue, RealGhClient};
pub use merge_authority::{
    BASE_ALLOWLIST_ENV, DEFAULT_BASE_ALLOWLIST, MergeOutcome, OpenPrSummary, PrGhClient,
    PrSnapshot, RealPrGhClient, base_allowlist_from_env, evaluate_objective_gates,
    merge_pr_if_merge_ready, merge_pr_if_merge_ready_with_allowlist,
    merge_pr_if_merge_ready_with_judge, parse_pr_list_json,
};
pub use merge_judge::{
    Blocker, JudgeOutcome, LlmMergeJudge, MergeJudge, MergeJudgeKind, RefusingMergeJudge, Verdict,
    build_merge_judge,
};
pub use recipe_merge_judge::RecipeMergeJudge;
pub use routing::route_failure;
pub use types::{OrchestratorRunSummary, StewardshipOutcome, TargetRepo};

use crate::error::SimardResult;
use crate::goal_curation::GoalBoard;
use crate::goal_curation::enqueue_stewardship_issue;

/// Process one orchestrator run summary end-to-end. See the module docstring
/// for the pipeline.
pub fn process_orchestrator_run(
    run: &OrchestratorRunSummary,
    gh: &dyn GhClient,
    board: &mut GoalBoard,
) -> SimardResult<StewardshipOutcome> {
    types::validate(run)?;
    let target = route_failure(&run.source_module)?;
    let repo = target.slug().to_string();
    let signature = failure_signature(&run.failure_kind, &run.error_text);

    let existing = gh.search_issues(&repo, &signature)?;
    if let Some(issue) = find_existing(&existing, &signature) {
        let issue = issue.clone();
        enqueue_stewardship_issue(board, &repo, issue.number, &issue.url, &signature)?;
        return Ok(StewardshipOutcome::MatchedExisting {
            repo,
            issue_number: issue.number,
            url: issue.url,
            signature,
        });
    }

    let title = format!(
        "[stewardship] {kind} in {src}",
        kind = run.failure_kind,
        src = run.source_module
    );
    let body = format!(
        "filed-by: simard-stewardship\n\
         stewardship-signature: {sig}\n\
         originating-run: {rid}\n\
         failed-step: {step}\n\
         source-module: {src}\n\
         \n\
         ## Error\n\
         {err}\n",
        sig = signature,
        rid = run.run_id,
        step = run.failed_step,
        src = run.source_module,
        err = run.error_text,
    );
    let new = gh.create_issue(&repo, &title, &body)?;
    enqueue_stewardship_issue(board, &repo, new.number, &new.url, &signature)?;
    Ok(StewardshipOutcome::FiledNew {
        repo,
        issue_number: new.number,
        url: new.url,
        signature,
    })
}
