//! TEST-FIRST (Step 7 TDD) — the overseer's auto-generated documentation-PR
//! **reconciliation** pass that ends the stale-auto-doc-PR churn (goal_hygiene).
//!
//! # The churn these tests kill
//!
//! An automated doc-update flow opens a fresh `"Update documentation with N
//! changed files"` PR per doc-drift event without deduping, rebasing, or
//! auto-closing superseded ones. In the field ~30 stale, CONFLICTING, draft
//! auto-doc PRs accumulated (oldest from 2026-07-22), rotting unmerged.
//!
//! # The contract (what the fix must make true)
//!
//! 1. A composite, **fail-closed** identity gate `is_auto_doc_pr` positively
//!    identifies an auto-doc PR only when EVERY signal holds (title marker +
//!    known auto-generation author + draft + label). A human PR — or one with an
//!    empty/absent author — is never a candidate.
//! 2. A pure `reconcile_doc_prs` keeps the single newest auto-doc PR (canonical)
//!    and queues every other candidate for close, tagged `SupersededDuplicate`
//!    or `StaleConflictingDraft`. The canonical PR is NEVER in the close set, so
//!    the pass can never close every candidate. Non-auto-doc PRs are ignored.
//! 3. The `run_doc_pr_reconcile` executor closes by NUMBER via the additive,
//!    default-no-op `PrGhClient::close_pr`, and is fail-closed on a list error
//!    (no closes that cycle).
//!
//! `reconcile_doc_prs` / `is_auto_doc_pr` are pure (no I/O) and exhaustively
//! unit-tested on fixture PR lists. RED until the `doc_pr_reconcile` module and
//! the `close_pr` trait method exist.

use std::cell::RefCell;

use crate::error::{SimardError, SimardResult};
use crate::overseer::doc_pr_reconcile::{
    AUTO_DOC_PR_AUTHOR, AUTO_DOC_PR_LABEL, AUTO_DOC_PR_TITLE_MARKER, CloseReason, is_auto_doc_pr,
    reconcile_doc_prs, run_doc_pr_reconcile,
};
use crate::stewardship::PrSnapshot;
use crate::stewardship::merge_authority::{OpenPrSummary, PrGhClient};

const REPO: &str = "rysweet/Simard";

// --- fixtures ---------------------------------------------------------------

/// A canonical auto-doc PR that passes EVERY gate signal, parameterised by
/// number and `mergeable` state.
fn auto_doc_pr(number: u32, mergeable: &str) -> OpenPrSummary {
    OpenPrSummary {
        number,
        title: format!("{AUTO_DOC_PR_TITLE_MARKER} {number} changed files"),
        mergeable: mergeable.to_string(),
        author: AUTO_DOC_PR_AUTHOR.to_string(),
        labels: vec![AUTO_DOC_PR_LABEL.to_string()],
        is_draft: Some(true),
        ..Default::default()
    }
}

/// A human PR that must NEVER be reconciled.
fn human_pr(number: u32) -> OpenPrSummary {
    OpenPrSummary {
        number,
        title: format!("{AUTO_DOC_PR_TITLE_MARKER} {number} changed files"), // same title!
        mergeable: "MERGEABLE".to_string(),
        author: "a-human-contributor".to_string(),
        labels: vec![],
        is_draft: Some(false),
        ..Default::default()
    }
}

// --- fake gh client ---------------------------------------------------------

struct FakeDocPrClient {
    open: Vec<OpenPrSummary>,
    list_fails: bool,
    closed: RefCell<Vec<(u32, String)>>,
}

impl FakeDocPrClient {
    fn with(open: Vec<OpenPrSummary>) -> Self {
        Self {
            open,
            list_fails: false,
            closed: RefCell::new(Vec::new()),
        }
    }
    fn failing() -> Self {
        Self {
            open: Vec::new(),
            list_fails: true,
            closed: RefCell::new(Vec::new()),
        }
    }
    fn closed_numbers(&self) -> Vec<u32> {
        self.closed.borrow().iter().map(|(n, _)| *n).collect()
    }
}

impl PrGhClient for FakeDocPrClient {
    fn view_pr(&self, _repo: &str, _pr: u32) -> SimardResult<PrSnapshot> {
        unreachable!("reconciliation never views a PR")
    }
    fn squash_merge(&self, _repo: &str, _pr: u32) -> SimardResult<()> {
        unreachable!("reconciliation never merges a PR")
    }
    fn list_open_prs(&self, _repo: &str, _limit: u32) -> SimardResult<Vec<OpenPrSummary>> {
        if self.list_fails {
            return Err(SimardError::StewardshipGhCommandFailed {
                reason: "gh pr list transport failure".to_string(),
            });
        }
        Ok(self.open.clone())
    }
    fn close_pr(&self, _repo: &str, number: u32, comment: &str) -> SimardResult<()> {
        self.closed.borrow_mut().push((number, comment.to_string()));
        Ok(())
    }
}

// === is_auto_doc_pr: composite fail-closed gate =============================

#[test]
fn a_fully_qualified_pr_is_an_auto_doc_pr() {
    assert!(
        is_auto_doc_pr(&auto_doc_pr(42, "MERGEABLE")),
        "title marker + auto-gen author + draft + label ⇒ auto-doc PR"
    );
}

#[test]
fn empty_author_fails_closed_as_human() {
    let mut pr = auto_doc_pr(42, "MERGEABLE");
    pr.author = String::new();
    assert!(
        !is_auto_doc_pr(&pr),
        "an empty/absent author must fail closed (treated as human) — never a candidate"
    );
}

#[test]
fn a_human_authored_pr_with_the_same_title_is_not_auto_doc() {
    assert!(
        !is_auto_doc_pr(&human_pr(42)),
        "a human author (even with the same title) is never an auto-doc candidate"
    );
}

#[test]
fn a_non_draft_pr_is_not_auto_doc() {
    let mut none_draft = auto_doc_pr(42, "MERGEABLE");
    none_draft.is_draft = None;
    assert!(
        !is_auto_doc_pr(&none_draft),
        "is_draft None must fail closed (only Some(true) qualifies)"
    );
    let mut ready = auto_doc_pr(42, "MERGEABLE");
    ready.is_draft = Some(false);
    assert!(
        !is_auto_doc_pr(&ready),
        "is_draft Some(false) must fail closed"
    );
}

#[test]
fn a_pr_missing_the_label_is_not_auto_doc() {
    let mut pr = auto_doc_pr(42, "MERGEABLE");
    pr.labels = vec!["some-other-label".to_string()];
    assert!(!is_auto_doc_pr(&pr), "the auto-doc label must be present");
}

#[test]
fn a_pr_with_the_wrong_title_is_not_auto_doc() {
    let mut pr = auto_doc_pr(42, "MERGEABLE");
    pr.title = "Fix a real bug in the OODA loop".to_string();
    assert!(
        !is_auto_doc_pr(&pr),
        "the title must start with the auto-doc marker"
    );
}

// === reconcile_doc_prs: pure decision core ==================================

#[test]
fn zero_candidates_is_a_no_op() {
    let decision = reconcile_doc_prs(&[]);
    assert_eq!(decision.canonical, None);
    assert!(decision.to_close.is_empty());
}

#[test]
fn a_single_candidate_is_canonical_with_no_closes() {
    let decision = reconcile_doc_prs(&[auto_doc_pr(7, "MERGEABLE")]);
    assert_eq!(
        decision.canonical,
        Some(7),
        "the lone auto-doc PR is the survivor"
    );
    assert!(
        decision.to_close.is_empty(),
        "the single-open invariant already holds — nothing to close"
    );
}

#[test]
fn the_newest_candidate_is_canonical_and_the_rest_are_superseded() {
    let decision = reconcile_doc_prs(&[
        auto_doc_pr(10, "MERGEABLE"),
        auto_doc_pr(30, "MERGEABLE"),
        auto_doc_pr(20, "MERGEABLE"),
    ]);
    assert_eq!(
        decision.canonical,
        Some(30),
        "the newest (highest-number) candidate is the keeper"
    );
    let mut closed: Vec<u32> = decision.to_close.iter().map(|c| c.number).collect();
    closed.sort_unstable();
    assert_eq!(
        closed,
        vec![10, 20],
        "every OTHER candidate is queued for close"
    );
    assert!(
        decision.to_close.iter().all(|c| c.number != 30),
        "the canonical PR must NEVER be in the close set"
    );
    assert!(
        decision
            .to_close
            .iter()
            .all(|c| c.reason == CloseReason::SupersededDuplicate),
        "mergeable duplicates are tagged SupersededDuplicate"
    );
}

#[test]
fn a_conflicting_duplicate_is_tagged_stale_conflicting_draft() {
    let decision = reconcile_doc_prs(&[
        auto_doc_pr(50, "MERGEABLE"), // canonical (newest)
        auto_doc_pr(40, "CONFLICTING"),
        auto_doc_pr(30, "MERGEABLE"),
    ]);
    assert_eq!(decision.canonical, Some(50));
    let conflicting = decision
        .to_close
        .iter()
        .find(|c| c.number == 40)
        .expect("the conflicting duplicate must be queued for close");
    assert_eq!(
        conflicting.reason,
        CloseReason::StaleConflictingDraft,
        "a CONFLICTING duplicate must be auto-closed as a stale conflicting draft"
    );
    let clean = decision
        .to_close
        .iter()
        .find(|c| c.number == 30)
        .expect("the clean older duplicate must be queued for close");
    assert_eq!(clean.reason, CloseReason::SupersededDuplicate);
}

#[test]
fn non_auto_doc_prs_are_ignored_entirely() {
    // A human PR sharing the title, plus a single genuine auto-doc PR: the human
    // PR is neither canonical nor closed.
    let decision = reconcile_doc_prs(&[human_pr(99), auto_doc_pr(7, "MERGEABLE")]);
    assert_eq!(
        decision.canonical,
        Some(7),
        "only the genuine auto-doc PR is a candidate"
    );
    assert!(
        decision.to_close.iter().all(|c| c.number != 99),
        "a human PR must never be closed by reconciliation"
    );
    assert!(decision.to_close.is_empty());
}

// === run_doc_pr_reconcile: executor =========================================

#[test]
fn executor_closes_the_superseded_prs_by_number() {
    let gh = FakeDocPrClient::with(vec![
        auto_doc_pr(10, "MERGEABLE"),
        auto_doc_pr(20, "CONFLICTING"),
        auto_doc_pr(30, "MERGEABLE"),
    ]);
    let report = run_doc_pr_reconcile(REPO, &gh).expect("reconcile succeeds");

    assert_eq!(
        report.canonical,
        Some(30),
        "the newest PR survives the single-open invariant"
    );
    let mut closed = report.closed.clone();
    closed.sort_unstable();
    assert_eq!(closed, vec![10, 20], "the older duplicates are closed");

    let mut executed = gh.closed_numbers();
    executed.sort_unstable();
    assert_eq!(
        executed,
        vec![10, 20],
        "the executor must close exactly the superseded PRs, by number"
    );
    assert!(
        !gh.closed_numbers().contains(&30),
        "the canonical PR is never closed"
    );
}

#[test]
fn executor_is_fail_closed_on_a_list_error() {
    let gh = FakeDocPrClient::failing();
    let result = run_doc_pr_reconcile(REPO, &gh);
    assert!(
        result.is_err(),
        "a listing failure must surface as an error"
    );
    assert!(
        gh.closed_numbers().is_empty(),
        "NO PR may be closed when the open-PR listing failed (fail-closed)"
    );
}

#[test]
fn executor_never_closes_a_human_pr() {
    let gh = FakeDocPrClient::with(vec![
        human_pr(99),
        auto_doc_pr(10, "MERGEABLE"),
        auto_doc_pr(20, "MERGEABLE"),
    ]);
    let report = run_doc_pr_reconcile(REPO, &gh).expect("reconcile succeeds");
    assert_eq!(report.canonical, Some(20));
    assert!(
        !gh.closed_numbers().contains(&99),
        "a human PR must never be closed"
    );
    assert_eq!(
        gh.closed_numbers(),
        vec![10],
        "only the superseded auto-doc duplicate is closed"
    );
}

#[test]
fn close_pr_defaults_to_a_no_op_for_unwired_clients() {
    // The additive trait method must default to a no-op so every existing
    // fake / unwired client performs NO mutation.
    struct MinimalClient;
    impl PrGhClient for MinimalClient {
        fn view_pr(&self, _repo: &str, _pr: u32) -> SimardResult<PrSnapshot> {
            unreachable!()
        }
        fn squash_merge(&self, _repo: &str, _pr: u32) -> SimardResult<()> {
            unreachable!()
        }
    }
    // Must compile (default method present) and succeed without side effects.
    MinimalClient
        .close_pr(REPO, 123, "superseded by #456")
        .expect("the default close_pr is a no-op that returns Ok");
}
