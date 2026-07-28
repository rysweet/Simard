//! Auto-generated documentation-PR **reconciliation** (goal_hygiene).
//!
//! An automated doc-update flow opens a fresh `"Update documentation with N
//! changed files"` PR per doc-drift event without deduping, rebasing, or
//! auto-closing superseded ones — so stale, CONFLICTING, draft auto-doc PRs
//! accumulate and rot unmerged. This module enforces a **single-open** invariant
//! for those PRs: keep the newest (canonical) auto-doc PR and close every other
//! candidate, tagging each close as a superseded duplicate or a stale
//! CONFLICTING draft.
//!
//! The design is additive and split into a **pure** decision core
//! ([`reconcile_doc_prs`]) with no I/O and a thin IO-guarded executor
//! ([`run_doc_pr_reconcile`]) that lists open PRs and closes the superseded ones
//! by number via the additive [`PrGhClient::close_pr`]. A composite,
//! fail-closed identity gate ([`is_auto_doc_pr`]) positively identifies an
//! auto-doc PR only when the title marker, the known auto-generation author, the
//! draft flag, and the auto-doc label ALL hold — so a human PR (or one with an
//! empty/absent author) is never a candidate.
//!
//! See `docs/reference/auto-doc-pr-reconciliation-api.md` for the full contract.

use crate::error::SimardResult;
use crate::overseer::config::{DEFAULT_OVERSEER_AUTHOR_LOGIN, SIMARD_ENGINEER_PR_LABEL};
use crate::stewardship::merge_authority::{OpenPrSummary, PrGhClient};

/// Title-prefix a candidate PR's title must start with. A durable cross-system
/// string; changing it would silently disable reconciliation, so it is a stable
/// contract.
pub const AUTO_DOC_PR_TITLE_MARKER: &str = "Update documentation with";

/// The exact author login a candidate's `pr.author` must equal. A compile-time
/// constant so the gate stays pure (no env/I/O). An empty/absent author can
/// never equal it, so a human PR fails closed. The auto-doc PRs are opened under
/// the overseer bot identity.
pub const AUTO_DOC_PR_AUTHOR: &str = DEFAULT_OVERSEER_AUTHOR_LOGIN;

/// The label a candidate must carry. Simard's own autonomous PRs (including the
/// auto-doc drafts) are tagged with this durable label.
pub const AUTO_DOC_PR_LABEL: &str = SIMARD_ENGINEER_PR_LABEL;

/// The `gh pr list` fetch window for reconciliation. Wide enough to see the full
/// backlog of accumulated auto-doc drafts in one pass so the canonical selection
/// is over ALL candidates, not a truncated window.
const DOC_PR_LIST_LIMIT: u32 = 200;

/// Maximum closes executed per cycle — a bounded batch so a large accumulated
/// backlog is drained over several cycles rather than in one unbounded storm of
/// `gh pr close` mutations. The canonical PR is always preserved regardless.
const MAX_CLOSES_PER_CYCLE: usize = 25;

/// Why a duplicate auto-doc PR is being closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    /// An older duplicate, superseded by the canonical (newest) auto-doc PR.
    SupersededDuplicate,
    /// A candidate whose `mergeable` state is `CONFLICTING` — a rotted draft.
    StaleConflictingDraft,
}

/// One queued close: the PR number, why it is closed, and the comment to post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocPrClose {
    pub number: u32,
    pub reason: CloseReason,
    /// Comment posted on close, e.g. `"superseded by #<canonical>"`.
    pub comment: String,
}

/// The pure decision: which single auto-doc PR to keep and which to close.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocPrReconcileDecision {
    /// The PR kept open (the single-open invariant's survivor), if any candidate
    /// exists.
    pub canonical: Option<u32>,
    /// PRs to close, each with the reason it was superseded/auto-closed. The
    /// canonical PR is NEVER present here.
    pub to_close: Vec<DocPrClose>,
}

/// Structured outcome of an executed reconciliation pass, for the overseer
/// journal/audit.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DocPrReconcileReport {
    /// The surviving canonical auto-doc PR, if any.
    pub canonical: Option<u32>,
    /// PR numbers actually closed this cycle.
    pub closed: Vec<u32>,
    /// Count of open PRs that were NOT auto-doc candidates (ignored).
    pub skipped: usize,
    /// Per-close failures (number + reason); the pass continues past them so one
    /// flaky close never blocks the rest.
    pub errors: Vec<String>,
}

/// The `mergeable` state string GitHub reports for a PR with merge conflicts.
const CONFLICTING_MERGEABLE: &str = "CONFLICTING";

/// True only when EVERY signal marks `pr` an auto-generated doc-drift PR. Fails
/// closed: any missing signal — including an empty/absent author — returns
/// `false`, so a human PR is never a reconciliation candidate.
pub fn is_auto_doc_pr(pr: &OpenPrSummary) -> bool {
    pr.title.starts_with(AUTO_DOC_PR_TITLE_MARKER)
        && !pr.author.is_empty()
        && pr.author == AUTO_DOC_PR_AUTHOR
        && pr.is_draft == Some(true)
        && pr.labels.iter().any(|l| l == AUTO_DOC_PR_LABEL)
}

/// Pure: given the current open-PR listing for one repo, decide which single
/// auto-doc PR to keep (canonical) and which to close (with a reason). Performs
/// NO I/O. Non-auto-doc PRs are ignored entirely.
///
/// The canonical PR is the newest (highest number) auto-doc candidate and is
/// NEVER placed in the close set, so the decision can never close every
/// candidate. Zero or one candidate ⇒ the invariant already holds (no closes).
pub fn reconcile_doc_prs(open_prs: &[OpenPrSummary]) -> DocPrReconcileDecision {
    let candidates: Vec<&OpenPrSummary> = open_prs.iter().filter(|pr| is_auto_doc_pr(pr)).collect();

    let Some(canonical) = candidates.iter().map(|pr| pr.number).max() else {
        return DocPrReconcileDecision {
            canonical: None,
            to_close: Vec::new(),
        };
    };

    let to_close: Vec<DocPrClose> = candidates
        .iter()
        .filter(|pr| pr.number != canonical)
        .map(|pr| {
            let reason = if pr.mergeable == CONFLICTING_MERGEABLE {
                CloseReason::StaleConflictingDraft
            } else {
                CloseReason::SupersededDuplicate
            };
            let comment = match reason {
                CloseReason::SupersededDuplicate => format!(
                    "Auto-closing this superseded auto-generated documentation PR: \
                     it is an older duplicate superseded by the canonical open auto-doc \
                     PR #{canonical}. Enforcing the single-open auto-doc PR invariant \
                     (goal_hygiene)."
                ),
                CloseReason::StaleConflictingDraft => format!(
                    "Auto-closing this stale CONFLICTING auto-generated documentation \
                     draft: it can no longer merge cleanly and is superseded by the \
                     canonical open auto-doc PR #{canonical}. Enforcing the single-open \
                     auto-doc PR invariant (goal_hygiene)."
                ),
            };
            DocPrClose {
                number: pr.number,
                reason,
                comment,
            }
        })
        .collect();

    DocPrReconcileDecision {
        canonical: Some(canonical),
        to_close,
    }
}

/// Apply a reconciliation to one repo: list open PRs, compute the pure decision,
/// then execute the closes by NUMBER via [`PrGhClient::close_pr`]. Bounded and
/// IO-guarded; returns a structured report for the overseer journal/audit.
///
/// **Fail-closed on read error:** a listing failure surfaces the error and
/// performs NO closes that cycle. Per-close failures are collected into
/// [`DocPrReconcileReport::errors`] and do not abort the batch (closing is a
/// hygiene convenience, not a correctness gate). A bounded number of closes
/// ([`MAX_CLOSES_PER_CYCLE`]) is executed per cycle.
pub fn run_doc_pr_reconcile(repo: &str, gh: &dyn PrGhClient) -> SimardResult<DocPrReconcileReport> {
    let open_prs = gh.list_open_prs(repo, DOC_PR_LIST_LIMIT)?;
    let candidate_count = open_prs.iter().filter(|pr| is_auto_doc_pr(pr)).count();
    let skipped = open_prs.len().saturating_sub(candidate_count);

    let decision = reconcile_doc_prs(&open_prs);

    tracing::info!(
        target: "simard::overseer",
        repo = %repo,
        canonical = decision.canonical.map(|n| n as i64).unwrap_or(-1),
        candidates = candidate_count,
        to_close = decision.to_close.len(),
        skipped,
        "auto-doc PR reconciliation: single-open invariant decision computed",
    );

    let mut report = DocPrReconcileReport {
        canonical: decision.canonical,
        closed: Vec::new(),
        skipped,
        errors: Vec::new(),
    };

    for close in decision.to_close.into_iter().take(MAX_CLOSES_PER_CYCLE) {
        match gh.close_pr(repo, close.number, &close.comment) {
            Ok(()) => {
                tracing::info!(
                    target: "simard::overseer",
                    repo = %repo,
                    pr = close.number,
                    reason = ?close.reason,
                    "auto-doc PR reconciliation: closed superseded auto-doc PR",
                );
                report.closed.push(close.number);
            }
            Err(e) => {
                tracing::warn!(
                    target: "simard::overseer",
                    repo = %repo,
                    pr = close.number,
                    reason = ?close.reason,
                    error = %e,
                    "auto-doc PR reconciliation: failed to close a superseded auto-doc PR \
                     — leaving it open, continuing with the rest",
                );
                report.errors.push(format!("close #{}: {e}", close.number));
            }
        }
    }

    Ok(report)
}
