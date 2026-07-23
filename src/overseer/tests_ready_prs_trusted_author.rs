//! TDD (Step 7) — FAILING tests for the P1 `project_ready_prs` selection fix.
//!
//! Secondary root cause of P1: the re-narrowing projection in
//! [`crate::overseer::project_ready_prs`] silently DROPS green, mergeable,
//! rysweet-authored PRs that are neither carrying the engineer-PR label nor on
//! an engineer branch (gate #3), and fails closed when the draft state is
//! absent (gate #5). Delivery-ready PRs authored by a TRUSTED author must reach
//! `ready_prs` so the downstream merge chain can act on them.
//!
//! These tests are RED until `project_ready_prs` gains a `trusted_authors`
//! parameter and admits trusted-author PRs at gate #3 while preserving every
//! existing safety gate (anti-recursion author guard, draft exclusion,
//! objective gates).
//!
//! New signature under test:
//! ```ignore
//! pub fn project_ready_prs(
//!     candidates: &[ProjectionCandidate],
//!     base_allowlist: &[String],
//!     overseer_login: &str,
//!     trusted_authors: &[String],
//! ) -> Vec<PrRef>;
//! ```
//!
//! Wire-in (added by the implementation step):
//! `#[cfg(test)] mod tests_ready_prs_trusted_author;` in `src/overseer/mod.rs`.

use crate::overseer::config::{self, DEFAULT_OVERSEER_AUTHOR_LOGIN, SIMARD_ENGINEER_PR_LABEL};
use crate::overseer::{PrDisposition, PrRef, ProjectionCandidate, ReasonedPr, project_ready_prs};
use crate::stewardship::PrSnapshot;
use crate::stewardship::merge_authority::CheckRollupEntry;

fn overseer_login() -> String {
    DEFAULT_OVERSEER_AUTHOR_LOGIN.to_string()
}

fn base_allowlist() -> Vec<String> {
    vec!["main".to_string()]
}

fn trusted() -> Vec<String> {
    vec!["rysweet".to_string()]
}

/// A green, mergeable snapshot with NO engineer label (the operator/human-style
/// PR shape that gate #3 currently drops). `author_login` is the field the P1
/// fix adds to `PrSnapshot`.
fn green_unlabeled_snapshot(author: &str) -> PrSnapshot {
    PrSnapshot {
        body: String::new(),
        mergeable: "MERGEABLE".to_string(),
        review_decision: "APPROVED".to_string(),
        checks: vec![CheckRollupEntry {
            name: "ci".to_string(),
            state: "SUCCESS".to_string(),
        }],
        base_ref_name: "main".to_string(),
        labels: vec![],
        author_login: author.to_string(),
    }
}

fn candidate(
    repo: &str,
    pr: u32,
    disposition: PrDisposition,
    author: &str,
    head: &str,
    is_draft: Option<bool>,
    snapshot: PrSnapshot,
) -> ProjectionCandidate {
    ProjectionCandidate {
        reasoned: ReasonedPr {
            repo: repo.to_string(),
            pr,
            disposition,
            rationale: "r".to_string(),
            duplicate_of: None,
        },
        author_login: author.to_string(),
        head_ref: head.to_string(),
        snapshot,
        is_draft,
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Gate #3 — admit a TRUSTED-author non-engineer PR (the core P1 unblock)
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn admits_trusted_author_green_pr_without_engineer_label_or_branch() {
    // #4389-shaped: green, mergeable, rysweet-authored, NON-engineer branch, no
    // simard-autonomous label. Previously dropped by gate #3; must now project.
    let cands = vec![candidate(
        "rysweet/Simard",
        4389,
        PrDisposition::ReadyForMerge,
        "rysweet",
        "feat/issue-4389-nodeoptions",
        Some(false),
        green_unlabeled_snapshot("rysweet"),
    )];
    let ready = project_ready_prs(&cands, &base_allowlist(), &overseer_login(), &trusted());
    assert_eq!(
        ready,
        vec![PrRef {
            repo: "rysweet/Simard".to_string(),
            pr: 4389,
        }],
        "a green trusted-author PR must be projected even without engineer label/branch"
    );
}

#[test]
fn still_refuses_untrusted_author_non_engineer_pr() {
    // A non-trusted author on a non-engineer PR is an operator/human PR — never
    // projected. Trust widening must be scoped to the allowlist only.
    let cands = vec![candidate(
        "rysweet/Simard",
        4389,
        PrDisposition::ReadyForMerge,
        "some-human",
        "feat/human-typed",
        Some(false),
        green_unlabeled_snapshot("some-human"),
    )];
    let ready = project_ready_prs(&cands, &base_allowlist(), &overseer_login(), &trusted());
    assert!(
        ready.is_empty(),
        "an untrusted, unlabeled, non-engineer PR must NOT be projected"
    );
}

#[test]
fn preserves_engineer_label_admission_for_untrusted_author() {
    // The existing engineer-origin admission must still work even when the
    // author is not on the trusted-author allowlist (label proves origin).
    let mut snap = green_unlabeled_snapshot("engineer-bot");
    snap.labels = vec![SIMARD_ENGINEER_PR_LABEL.to_string()];
    let cands = vec![candidate(
        "rysweet/Simard",
        4123,
        PrDisposition::ReadyForMerge,
        "engineer-bot",
        "engineer/4123-abcdef",
        Some(false),
        snap,
    )];
    let ready = project_ready_prs(&cands, &base_allowlist(), &overseer_login(), &trusted());
    assert_eq!(ready.len(), 1, "engineer-label admission must be preserved");
}

// ════════════════════════════════════════════════════════════════════════════
// Safety gates preserved — trust NEVER bypasses recursion/draft/objective gates
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn trusted_admission_never_bypasses_anti_recursion_author_guard() {
    // Even if the overseer bot login were in the trusted list, its own PR must
    // never be projected.
    let bot = overseer_login();
    let trusted_with_bot = vec!["rysweet".to_string(), bot.clone()];
    let cands = vec![candidate(
        "rysweet/Simard",
        4389,
        PrDisposition::ReadyForMerge,
        &bot,
        "feat/x",
        Some(false),
        green_unlabeled_snapshot(&bot),
    )];
    let ready = project_ready_prs(&cands, &base_allowlist(), &bot, &trusted_with_bot);
    assert!(
        ready.is_empty(),
        "anti-recursion guard must win over trusted-author admission"
    );
}

#[test]
fn trusted_admission_never_bypasses_objective_gates() {
    // A trusted author on a RED / CONFLICTING / off-base PR is still refused —
    // the objective tier only replaces the JUDGMENT half.
    for mutate in [
        (|s: &mut PrSnapshot| s.mergeable = "CONFLICTING".to_string()) as fn(&mut PrSnapshot),
        |s: &mut PrSnapshot| {
            s.checks = vec![CheckRollupEntry {
                name: "ci".to_string(),
                state: "FAILURE".to_string(),
            }]
        },
        |s: &mut PrSnapshot| s.base_ref_name = "stale-base".to_string(),
    ] {
        let mut snap = green_unlabeled_snapshot("rysweet");
        mutate(&mut snap);
        let cands = vec![candidate(
            "rysweet/Simard",
            4389,
            PrDisposition::ReadyForMerge,
            "rysweet",
            "feat/x",
            Some(false),
            snap,
        )];
        let ready = project_ready_prs(&cands, &base_allowlist(), &overseer_login(), &trusted());
        assert!(
            ready.is_empty(),
            "objective gates must still exclude non-green/non-mergeable/off-base PRs"
        );
    }
}

#[test]
fn trusted_admission_still_excludes_drafts_fail_closed() {
    // Gate #5: a draft can never merge server-side. `Some(true)` and `None`
    // (unknown/absent draft state) are both excluded even for a trusted author.
    for draft in [Some(true), None] {
        let cands = vec![candidate(
            "rysweet/Simard",
            4389,
            PrDisposition::ReadyForMerge,
            "rysweet",
            "feat/x",
            draft,
            green_unlabeled_snapshot("rysweet"),
        )];
        let ready = project_ready_prs(&cands, &base_allowlist(), &overseer_login(), &trusted());
        assert!(
            ready.is_empty(),
            "draft state {draft:?} must be excluded fail-closed even for a trusted author"
        );
    }
}

#[test]
fn empty_trusted_list_reverts_to_engineer_only_admission() {
    // With no trusted authors configured, behaviour is the pre-P1 engineer-only
    // policy: a non-engineer PR is dropped.
    let cands = vec![candidate(
        "rysweet/Simard",
        4389,
        PrDisposition::ReadyForMerge,
        "rysweet",
        "feat/x",
        Some(false),
        green_unlabeled_snapshot("rysweet"),
    )];
    let ready = project_ready_prs(&cands, &base_allowlist(), &overseer_login(), &[]);
    assert!(
        ready.is_empty(),
        "an empty trusted-author allowlist must not admit non-engineer PRs"
    );
}

#[test]
fn trusted_match_is_case_insensitive_but_exact() {
    // GitHub logins compare case-insensitively; a look-alike must not match.
    let trusted_list = vec!["rysweet".to_string()];
    let admit = |author: &str| {
        let cands = vec![candidate(
            "rysweet/Simard",
            4389,
            PrDisposition::ReadyForMerge,
            author,
            "feat/x",
            Some(false),
            green_unlabeled_snapshot(author),
        )];
        !project_ready_prs(&cands, &base_allowlist(), &overseer_login(), &trusted_list).is_empty()
    };
    assert!(admit("RySweet"), "case-insensitive login match");
    assert!(
        !admit("rysweet-bot"),
        "look-alike login must not be trusted"
    );
    // Sanity: the config-level engineer-label helper is unaffected by this path.
    assert!(config::is_engineer_pr_label(SIMARD_ENGINEER_PR_LABEL));
}
