//! TDD (Step 7, RED) — the overseer **verify-and-merge escalation gap** for a
//! deploy-gate-converging PR (issue #4505 / DeployDrift / red-canary).
//!
//! ROOT CAUSE these tests kill: while a red-canary / DeployDrift blocker is
//! active (the running binary is behind merged `main` because the self-deploy
//! canary is red), the overseer keeps failing on the very gate that a green,
//! MERGEABLE, non-draft PR (#4505) would converge — yet that PR is never
//! surfaced/prioritised for escalation. The escalation candidate set treats it
//! as just another ready PR, so #4440/#4398 are escalated while the PR that
//! actually unblocks the deploy sits idle for hours.
//!
//! TARGET contract (references API that does NOT exist yet ⇒ the crate test
//! build FAILS to compile until the feature lands — the RED state):
//!
//!   * `prioritize_gate_converging_prs(authorized, candidates, deploy_drift)` —
//!     a PURE, SET-PRESERVING permutation of the already-authorized `ready_prs`
//!     that, WHEN DeployDrift is active, ranks the PR that converges the active
//!     deploy gate FIRST (so Decide surfaces it as a `VerifyAndMergePr` candidate
//!     alongside #4440/#4398). It is a re-ordering ONLY: it can never add,
//!     remove, or fabricate authorization for any PR — the six-criteria
//!     merge-authority gate still runs downstream unchanged.
//!   * `config::CONVERGES_GATE_PR_LABEL` (`"converges-gate"`) — the EXPLICIT,
//!     whole-string objective anchor that marks a Simard-origin PR as converging
//!     the deploy gate. A label (not a title/branch heuristic and not a per-PR
//!     head-SHA match, which `PrSnapshot` does not carry) is required so the
//!     ranking can never be spoofed by free text.
//!
//! Pure-function tests: no fakes, no network — the whole point of keeping the
//! ranking a deterministic rail beside `project_ready_prs`.

use crate::overseer::capabilities::{DeployDriftObservation, PrDisposition, PrRef, ReasonedPr};
use crate::overseer::config::{CONVERGES_GATE_PR_LABEL, SIMARD_ENGINEER_PR_LABEL};
use crate::overseer::{ProjectionCandidate, prioritize_gate_converging_prs};
use crate::stewardship::PrSnapshot;
use crate::stewardship::merge_authority::CheckRollupEntry;

// ─────────────────────────── builders ──────────────────────────────────────

/// A green, MERGEABLE, non-draft engineer snapshot carrying `labels`.
fn green_snapshot(labels: Vec<String>) -> PrSnapshot {
    PrSnapshot {
        body: String::new(),
        mergeable: "MERGEABLE".to_string(),
        review_decision: "APPROVED".to_string(),
        checks: vec![CheckRollupEntry {
            name: "ci".to_string(),
            state: "SUCCESS".to_string(),
        }],
        base_ref_name: "main".to_string(),
        labels,
    }
}

fn candidate(repo: &str, pr: u32, labels: Vec<String>) -> ProjectionCandidate {
    ProjectionCandidate {
        reasoned: ReasonedPr {
            repo: repo.to_string(),
            pr,
            disposition: PrDisposition::ReadyForMerge,
            rationale: "r".to_string(),
            duplicate_of: None,
        },
        author_login: "rysweet".to_string(),
        head_ref: format!("engineer/{pr}-abcdef"),
        is_draft: Some(false),
        snapshot: green_snapshot(labels),
    }
}

/// An engineer PR that ALSO carries the `converges-gate` anchor.
fn gate_converging_candidate(repo: &str, pr: u32) -> ProjectionCandidate {
    candidate(
        repo,
        pr,
        vec![
            SIMARD_ENGINEER_PR_LABEL.to_string(),
            CONVERGES_GATE_PR_LABEL.to_string(),
        ],
    )
}

/// An ordinary engineer PR (Simard-origin) that does NOT converge the gate.
fn ordinary_candidate(repo: &str, pr: u32) -> ProjectionCandidate {
    candidate(repo, pr, vec![SIMARD_ENGINEER_PR_LABEL.to_string()])
}

fn pr_ref(repo: &str, pr: u32) -> PrRef {
    PrRef {
        repo: repo.to_string(),
        pr,
    }
}

fn active_drift() -> DeployDriftObservation {
    DeployDriftObservation {
        target_commit: "deadbeefcafe".to_string(),
        behind_commits: 1,
    }
}

/// Multiset equality independent of order — the "set-preserving permutation"
/// invariant (no PR added, removed, or duplicated).
fn same_set(a: &[PrRef], b: &[PrRef]) -> bool {
    let mut a: Vec<_> = a.to_vec();
    let mut b: Vec<_> = b.to_vec();
    a.sort_by(|x, y| (x.repo.as_str(), x.pr).cmp(&(y.repo.as_str(), y.pr)));
    b.sort_by(|x, y| (x.repo.as_str(), x.pr).cmp(&(y.repo.as_str(), y.pr)));
    a == b
}

// ─────────────────────────── tests ─────────────────────────────────────────

const REPO: &str = "rysweet/Simard";

#[test]
fn deploy_gate_converging_pr_is_ranked_first_under_active_drift() {
    // #4505 converges the gate; #4440/#4398 are ordinary ready PRs. With
    // DeployDrift active, the converging PR must come FIRST so it is surfaced as
    // an escalate-to-merge candidate alongside the others.
    let authorized = vec![pr_ref(REPO, 4440), pr_ref(REPO, 4398), pr_ref(REPO, 4505)];
    let candidates = vec![
        ordinary_candidate(REPO, 4440),
        ordinary_candidate(REPO, 4398),
        gate_converging_candidate(REPO, 4505),
    ];

    let ranked = prioritize_gate_converging_prs(&authorized, &candidates, Some(&active_drift()));

    assert_eq!(
        ranked.first(),
        Some(&pr_ref(REPO, 4505)),
        "the deploy-gate-converging PR (#4505) must be ranked first under active DeployDrift"
    );
    assert_eq!(
        &ranked[1..],
        &[pr_ref(REPO, 4440), pr_ref(REPO, 4398)],
        "non-converging PRs keep their original relative order (stable partition)"
    );
}

#[test]
fn ranking_is_a_set_preserving_permutation_no_authority_widening() {
    // The ranking is a re-ordering ONLY: identical multiset in, identical multiset
    // out. It can NEVER introduce or drop an authorization.
    let authorized = vec![pr_ref(REPO, 4440), pr_ref(REPO, 4398), pr_ref(REPO, 4505)];
    let candidates = vec![
        ordinary_candidate(REPO, 4440),
        ordinary_candidate(REPO, 4398),
        gate_converging_candidate(REPO, 4505),
    ];

    let ranked = prioritize_gate_converging_prs(&authorized, &candidates, Some(&active_drift()));

    assert_eq!(
        ranked.len(),
        authorized.len(),
        "ranking must not change the number of authorized PRs"
    );
    assert!(
        same_set(&ranked, &authorized),
        "ranking must be a permutation of the authorized set (no widening, no drop)"
    );
}

#[test]
fn no_drift_leaves_order_unchanged_even_with_a_converging_label() {
    // The gate-first promotion is scoped to an ACTIVE blocker. With no
    // DeployDrift, the helper is the identity even if a PR carries the
    // converges-gate label — ranking never re-orders when there is no gate to
    // converge.
    let authorized = vec![pr_ref(REPO, 4440), pr_ref(REPO, 4505)];
    let candidates = vec![
        ordinary_candidate(REPO, 4440),
        gate_converging_candidate(REPO, 4505),
    ];

    let ranked = prioritize_gate_converging_prs(&authorized, &candidates, None);

    assert_eq!(
        ranked, authorized,
        "with no active DeployDrift the ranking is the identity (order preserved)"
    );
}

#[test]
fn without_the_converges_gate_label_no_pr_is_promoted() {
    // Safety: a PR is treated as gate-converging ONLY via the explicit
    // whole-string `converges-gate` label. With drift active but NO candidate
    // carrying the label, nothing is promoted — title/branch/author heuristics
    // alone must never fabricate a gate-converging ranking.
    let authorized = vec![pr_ref(REPO, 4440), pr_ref(REPO, 4398)];
    let candidates = vec![
        ordinary_candidate(REPO, 4440),
        ordinary_candidate(REPO, 4398),
    ];

    let ranked = prioritize_gate_converging_prs(&authorized, &candidates, Some(&active_drift()));

    assert_eq!(
        ranked, authorized,
        "no converges-gate label ⇒ order is unchanged (no heuristic promotion)"
    );
}

#[test]
fn a_converging_candidate_not_in_the_authorized_set_is_never_injected() {
    // The helper reorders ONLY the already-authorized set. A converging candidate
    // that failed the authorization projection (absent from `authorized`) must
    // never be pulled into the result — authorization stays owned by
    // `project_ready_prs`.
    let authorized = vec![pr_ref(REPO, 4440)];
    let candidates = vec![
        ordinary_candidate(REPO, 4440),
        // #4505 converges the gate but is NOT authorized this cycle.
        gate_converging_candidate(REPO, 4505),
    ];

    let ranked = prioritize_gate_converging_prs(&authorized, &candidates, Some(&active_drift()));

    assert_eq!(
        ranked, authorized,
        "an unauthorized converging PR must never be injected by the ranking"
    );
    assert!(
        !ranked.contains(&pr_ref(REPO, 4505)),
        "authorization is owned by project_ready_prs, not by the gate ranking"
    );
}
