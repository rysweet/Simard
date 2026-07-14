//! Stewardship loop — autonomous failure → guarded GitHub issue for Simard
//! (issue #1167).
//!
//! See `Specs/ProductArchitecture.md` § Stewardship Mode and
//! `docs/concepts/stewardship-mode.md`.
//!
//! Pipeline:
//! 1. Validate the [`OrchestratorRunSummary`] (fail-loud on missing fields).
//! 2. Route `source_module` → [`TargetRepo`] (unmatched → default repo).
//! 3. Validate typed disposition, condition identity, cycle, and provenance.
//! 4. Reserve the durable mutation identity and cycle budget.
//! 5. Replay a journaled completion or file one new issue.

pub mod dedup;
pub mod gh_client;
pub mod merge_authority;
pub mod merge_judge;
pub mod mutation_guard;
pub mod mutation_store;
pub mod recipe_merge_judge;
pub mod routing;
pub mod types;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_extra;
#[cfg(test)]
mod tests_safety;

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
pub use types::{
    ArtifactOrigin, ArtifactProvenance, CycleId, IssueMutationIdentity, IssueMutationLimit,
    IssueMutationOutcome, IssueMutationRequest, LineageId, OrchestratorRunSummary,
    StewardshipDisposition, StewardshipOutcome, TargetRepo,
};

use crate::error::SimardResult;
use crate::stewardship::gh_client::StewardshipGh;
use crate::stewardship::mutation_guard::MutationGuard;

/// Process one orchestrator run summary end-to-end. See the module docstring
/// for the pipeline.
pub(crate) fn process_orchestrator_run(
    run: &OrchestratorRunSummary,
    gh: &dyn StewardshipGh,
    guard: &mut MutationGuard,
) -> SimardResult<StewardshipOutcome> {
    types::validate(run)?;
    if run.disposition == StewardshipDisposition::ObservationOnly {
        return Err(crate::error::SimardError::StewardshipInvalidMutation {
            field: "disposition",
            reason: "observation-only evidence cannot authorize a GitHub issue".to_string(),
        });
    }
    let target = route_failure(&run.source_module)?;
    let repo = target.slug().to_string();
    let signature = run.condition_id.as_str().to_string();
    let title = format!(
        "[stewardship] {kind} in {src}",
        kind = run.failure_kind,
        src = run.source_module
    );
    let body = format!(
        "filed-by: simard-stewardship\n\
         stewardship-signature: {sig}\n\
         stewardship-condition-id: {condition}\n\
         failure-kind: {kind}\n\
         originating-run: {rid}\n\
         failed-step: {step}\n\
         source-module: {src}\n\
         \n\
         ## Error\n\
         {err}\n",
        sig = signature,
        condition = run.condition_id.as_str(),
        kind = run.failure_kind,
        rid = run.run_id,
        step = run.failed_step,
        src = run.source_module,
        err = run.error_text,
    );
    let request = IssueMutationRequest::create(
        &repo,
        run.condition_id.clone(),
        run.provenance.clone(),
        title,
        body,
    )?;
    guard.begin_cycle(run.cycle_id.clone(), IssueMutationLimit::configured()?)?;

    let (issue, filed_new) = match guard.execute(&run.cycle_id, &request, gh)? {
        IssueMutationOutcome::Completed { issue } => (issue, true),
        IssueMutationOutcome::AlreadyCompleted { issue } => (issue, false),
    };
    if filed_new {
        Ok(StewardshipOutcome::FiledNew {
            repo,
            issue_number: issue.number,
            url: issue.url,
            signature,
        })
    } else {
        Ok(StewardshipOutcome::MatchedExisting {
            repo,
            issue_number: issue.number,
            url: issue.url,
            signature,
        })
    }
}
