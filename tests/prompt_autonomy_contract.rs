//! Durable contract for Simard's operational-autonomy prompt model.
//!
//! Operator (Ryan) directive: "for most operations she should not need
//! outside-party validation." Simard self-promotes well-scoped goals and
//! self-merges / self-validates clean, green, merge-ready work autonomously —
//! without waiting for a human approver — for MOST operations, while a small
//! HIGH-RISK set still surfaces to the operator for sign-off, and the existing
//! quality/safety gates (CI green, merge-judge, base-branch allow-list) are
//! preserved.
//!
//! These tests pin that contract so a future prompt edit cannot silently
//! re-introduce a blanket operator-approval gate or drop the HIGH-RISK
//! boundary. They assert on stable keyword invariants, not full-sentence
//! snapshots, so ordinary rewording does not break them.

use std::fs;
use std::path::PathBuf;

fn prompt(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("prompt_assets/simard")
        .join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read prompt asset {}: {e}", path.display()))
}

fn prompt_lc(name: &str) -> String {
    prompt(name).to_lowercase()
}

fn assert_contains_any(haystack_lc: &str, needles: &[&str], file: &str, what: &str) {
    assert!(
        needles
            .iter()
            .any(|n| haystack_lc.contains(&n.to_lowercase())),
        "{file} must express {what} (expected one of {needles:?})"
    );
}

fn assert_absent(haystack_lc: &str, needle: &str, file: &str) {
    assert!(
        !haystack_lc.contains(&needle.to_lowercase()),
        "{file} must no longer contain the blanket operator-approval gate phrase {needle:?}"
    );
}

#[test]
fn goal_curator_self_promotes_without_blanket_operator_gate() {
    let c = prompt_lc("goal_curator_system.md");
    assert_contains_any(
        &c,
        &["self-promote", "self-promotes"],
        "goal_curator_system.md",
        "autonomous goal self-promotion",
    );
    assert_absent(&c, "do not unilaterally promote", "goal_curator_system.md");
}

#[test]
fn improvement_curator_self_promotes_without_blanket_operator_gate() {
    let c = prompt_lc("improvement_curator_system.md");
    assert_contains_any(
        &c,
        &["self-promote", "self-promotes"],
        "improvement_curator_system.md",
        "autonomous improvement self-promotion",
    );
    assert_absent(
        &c,
        "require operator approval before promotion",
        "improvement_curator_system.md",
    );
}

#[test]
fn engineer_self_merges_routine_work_without_waiting_for_human() {
    let c = prompt_lc("engineer_system.md");
    assert_contains_any(
        &c,
        &[
            "self-merge",
            "self-merges",
            "without waiting for operator approval",
            "without an outside-party",
            "without outside-party validation",
        ],
        "engineer_system.md",
        "autonomous self-merge of routine, merge-ready work",
    );
}

#[test]
fn engineer_keeps_bounded_high_risk_operator_signoff() {
    let c = prompt_lc("engineer_system.md");
    assert!(
        c.contains("high-risk"),
        "engineer_system.md must define a bounded HIGH-RISK operator-sign-off boundary"
    );
    assert_contains_any(
        &c,
        &["force-push", "history rewrite"],
        "engineer_system.md",
        "history-rewrite / force-push as HIGH-RISK",
    );
    assert_contains_any(
        &c,
        &["credential", "secret"],
        "engineer_system.md",
        "secrets/credentials as HIGH-RISK",
    );
    assert!(
        c.contains("simard_git_protected_repos"),
        "engineer_system.md must name SIMARD_GIT_PROTECTED_REPOS as a HIGH-RISK boundary"
    );
}

#[test]
fn objective_self_merges_routine_work_and_keeps_high_risk_boundary() {
    let c = prompt_lc("goal_session_objective.md");
    assert_contains_any(
        &c,
        &[
            "self-merge",
            "self-merges",
            "without waiting for operator approval",
            "without an outside-party",
            "without outside-party validation",
        ],
        "goal_session_objective.md",
        "autonomous self-merge of routine, merge-ready work",
    );
    assert!(
        c.contains("high-risk"),
        "goal_session_objective.md must define a bounded HIGH-RISK operator-sign-off boundary"
    );
    assert_contains_any(
        &c,
        &["force-push", "history rewrite"],
        "goal_session_objective.md",
        "history-rewrite / force-push as HIGH-RISK",
    );
    assert_contains_any(
        &c,
        &["credential", "secret"],
        "goal_session_objective.md",
        "secrets/credentials as HIGH-RISK",
    );
    assert!(
        c.contains("simard_git_protected_repos"),
        "goal_session_objective.md must name SIMARD_GIT_PROTECTED_REPOS as a HIGH-RISK boundary"
    );
}

#[test]
fn merge_ready_contract_clarifies_governed_repos_without_required_reviewers() {
    // For a repo Simard governs that has no required human reviewers /
    // branch-protection-required approvals, "required approvals satisfied" is
    // met once the objective gates + merge-judge pass — she does not block
    // waiting for an external approver.
    let c = prompt_lc("engineer_system.md");
    assert_contains_any(
        &c,
        &[
            "no required human reviewers",
            "no required reviewers",
            "no branch-protection-required",
            "no required branch-protection",
        ],
        "engineer_system.md",
        "merge-ready clarification for governed repos without required human reviewers",
    );
}

#[test]
fn objective_uses_gated_cross_repo_merge_verb() {
    // Cross-repo PRs must land through the gated authority (simard merge-pr
    // --repo <owner/repo>), not a bare `gh pr merge` that skips the gates.
    let c = prompt("goal_session_objective.md");
    assert!(
        c.contains("simard merge-pr"),
        "goal_session_objective.md must route merges through the gated `simard merge-pr` verb"
    );
    assert!(
        c.contains("merge-pr") && c.contains("--repo"),
        "goal_session_objective.md must use `simard merge-pr <PR> --repo <owner/repo>` for cross-repo merges"
    );
}
