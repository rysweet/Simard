//! TDD (Step 7) — FAILING tests for the P1 objective-merge-judge fallback.
//!
//! These tests are written **before** the implementation and specify the
//! contract for the P1 fix ("green, mergeable, non-in-flight rysweet PRs are
//! selected but never merged"). Root cause: `build_merge_judge()` falls back to
//! [`RefusingMergeJudge`] (always `Verdict::NotReady`) whenever no LLM/recipe
//! provider is wired, so every delivery-ready PR is refused and re-escalated.
//!
//! They are RED until the following are implemented (see the design spec):
//!   * `crate::stewardship::objective_merge_judge::ObjectiveMergeJudge`
//!   * `crate::stewardship::merge_judge::MergeJudgeKind::Objective`
//!   * `crate::stewardship::merge_judge::resolve_merge_judge_kind`
//!   * `crate::stewardship::merge_authority::PrSnapshot::author_login`
//!   * `crate::overseer::config::merge_objective_fallback_enabled_from`
//!   * `crate::overseer::config::merge_trusted_authors_from`
//!
//! Security invariants under test (see design `security_considerations`):
//!   * default OFF — env unset => `RefusingMergeJudge`, never the objective tier;
//!   * trust is keyed on the AUTHENTICATED `author.login` (exact equality), not
//!     on a spoofable body/title/trailer;
//!   * the overseer bot identity is excluded from the trusted-author allowlist
//!     (no self-merge loop);
//!   * the objective tier only replaces the JUDGMENT half — the objective gates
//!     (CI-green, mergeable, base/repo allowlists) still run downstream.
//!
//! Wire-in (added by the implementation step):
//! `#[cfg(test)] mod tests_objective_merge_judge;` in `src/stewardship/mod.rs`.

use crate::stewardship::merge_authority::{PrSnapshot, parse_pr_view_json};
use crate::stewardship::merge_judge::{
    MergeJudge, MergeJudgeKind, RefusingMergeJudge, Verdict, resolve_merge_judge_kind,
};
use crate::stewardship::objective_merge_judge::ObjectiveMergeJudge;

use crate::overseer::config::{
    SIMARD_MERGE_OBJECTIVE_FALLBACK_ENV, SIMARD_MERGE_TRUSTED_AUTHORS_ENV,
    merge_objective_fallback_enabled_from, merge_trusted_authors_from,
};

/// Test env resolver: fixed key/value pairs, `None` for anything else. Mirrors
/// the `fn env(pairs)` helper the existing `overseer::config` tests use so the
/// hardened `_from(lookup)` seam is exercised without touching the real
/// process environment.
fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move |k: &str| map.get(k).cloned()
}

/// A green, mergeable snapshot authored by `author`. `author_login` is the NEW
/// field the P1 fix adds to `PrSnapshot` (hydrated from the existing
/// `gh pr view --json ...,author` call).
fn green_snapshot_by(author: &str) -> PrSnapshot {
    PrSnapshot {
        body: "## CI\ngreen\n## Tests\ncovered\n".to_string(),
        mergeable: "MERGEABLE".to_string(),
        review_decision: "APPROVED".to_string(),
        checks: vec![],
        base_ref_name: "main".to_string(),
        labels: vec![],
        author_login: author.to_string(),
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 1. PrSnapshot gains `author_login`, hydrated from `gh pr view --json ,author`
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn pr_view_json_hydrates_author_login() {
    // The judge-layer trust check needs the AUTHENTICATED author, so the
    // existing `gh pr view` parse must now carry `author.login`.
    let stdout = br#"{
        "body": "b",
        "mergeable": "MERGEABLE",
        "reviewDecision": "APPROVED",
        "statusCheckRollup": [],
        "baseRefName": "main",
        "labels": [],
        "author": { "login": "rysweet" }
    }"#;
    let snap = parse_pr_view_json(stdout).expect("valid gh pr view JSON parses");
    assert_eq!(snap.author_login, "rysweet");
}

#[test]
fn pr_view_json_absent_author_hydrates_empty_fail_closed() {
    // A missing author object must fail closed to an EMPTY login — never a
    // trusted default — so an unknown author can never match the allowlist.
    let stdout = br#"{ "body": "b", "mergeable": "MERGEABLE", "baseRefName": "main" }"#;
    let snap = parse_pr_view_json(stdout).expect("parses with defaults");
    assert_eq!(
        snap.author_login, "",
        "absent author => empty login (fail-closed)"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// 2. ObjectiveMergeJudge — the opt-in non-refusing tier
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn objective_judge_reports_objective_kind_and_is_configured() {
    // Dashboard renders "Judge: configured (objective)" off this signal.
    let judge = ObjectiveMergeJudge::new(
        vec!["rysweet".to_string()],
        "simard-overseer[bot]".to_string(),
    );
    assert_eq!(judge.kind(), MergeJudgeKind::Objective);
    assert!(judge.kind().is_configured());
}

#[test]
fn objective_judge_passes_trusted_author_green_pr() {
    // The whole P1 fix: a green PR from a trusted author gets a READY verdict
    // (instead of the RefusingMergeJudge NotReady that stalls delivery).
    let judge = ObjectiveMergeJudge::new(
        vec!["rysweet".to_string()],
        "simard-overseer[bot]".to_string(),
    );
    let out = judge
        .judge(4389, "rysweet/Simard", &green_snapshot_by("rysweet"))
        .expect("objective judge does not error");
    assert_eq!(out.verdict, Verdict::Ready);
}

#[test]
fn objective_judge_refuses_untrusted_author() {
    // Someone not on the allowlist must NOT get a ready verdict even on a green
    // PR — trust is the whole gate the objective tier replaces.
    let judge = ObjectiveMergeJudge::new(
        vec!["rysweet".to_string()],
        "simard-overseer[bot]".to_string(),
    );
    let out = judge
        .judge(
            4389,
            "rysweet/Simard",
            &green_snapshot_by("some-random-user"),
        )
        .expect("objective judge does not error");
    assert_eq!(out.verdict, Verdict::NotReady);
    assert!(
        !out.blockers.is_empty(),
        "refusal must carry an actionable blocker"
    );
}

#[test]
fn objective_judge_matches_author_case_insensitively_but_exactly() {
    // GitHub logins are case-insensitive; `RySweet` == `rysweet`. But a
    // look-alike (`rysweet-bot`, `notrysweet`) must NOT match.
    let judge = ObjectiveMergeJudge::new(
        vec!["rysweet".to_string()],
        "simard-overseer[bot]".to_string(),
    );
    assert_eq!(
        judge
            .judge(1, "rysweet/Simard", &green_snapshot_by("RySweet"))
            .unwrap()
            .verdict,
        Verdict::Ready
    );
    for imposter in ["rysweet-bot", "notrysweet", "rysweet ", " rysweet"] {
        assert_eq!(
            judge
                .judge(1, "rysweet/Simard", &green_snapshot_by(imposter))
                .unwrap()
                .verdict,
            Verdict::NotReady,
            "look-alike/padded login {imposter:?} must not match the allowlist"
        );
    }
}

#[test]
fn objective_judge_excludes_bot_identity_no_self_merge() {
    // Even if the bot login is somehow present in the allowlist, the judge must
    // never issue Ready for the overseer bot's own PR (anti self-merge loop).
    let bot = "simard-overseer[bot]";
    let judge = ObjectiveMergeJudge::new(
        vec!["rysweet".to_string(), bot.to_string()],
        bot.to_string(),
    );
    let out = judge
        .judge(4389, "rysweet/Simard", &green_snapshot_by(bot))
        .expect("objective judge does not error");
    assert_eq!(
        out.verdict,
        Verdict::NotReady,
        "the bot identity is always excluded from a Ready verdict"
    );
}

#[test]
fn objective_judge_refuses_empty_author_fail_closed() {
    // An empty/unknown author (absent `author` object in the listing) can never
    // be trusted.
    let judge = ObjectiveMergeJudge::new(
        vec!["rysweet".to_string()],
        "simard-overseer[bot]".to_string(),
    );
    assert_eq!(
        judge
            .judge(1, "rysweet/Simard", &green_snapshot_by(""))
            .unwrap()
            .verdict,
        Verdict::NotReady
    );
}

#[test]
fn objective_judge_with_empty_allowlist_refuses_everyone() {
    // An empty trusted-authors list is fully fail-closed — no one is trusted.
    let judge = ObjectiveMergeJudge::new(vec![], "simard-overseer[bot]".to_string());
    assert_eq!(
        judge
            .judge(1, "rysweet/Simard", &green_snapshot_by("rysweet"))
            .unwrap()
            .verdict,
        Verdict::NotReady
    );
}

// ════════════════════════════════════════════════════════════════════════════
// 3. resolve_merge_judge_kind — Recipe > LLM > (Objective iff opt-in) > Refusing
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn judge_resolution_defaults_to_refusing_when_nothing_configured() {
    // Default posture: no recipe, no LLM, fallback OFF => RefusingMergeJudge.
    let kind = resolve_merge_judge_kind(
        /* recipe_available */ false, /* llm_available */ false,
        /* objective_fallback */ false,
    );
    assert_eq!(kind, MergeJudgeKind::Refusing);
    assert!(!kind.is_configured());
}

#[test]
fn judge_resolution_uses_objective_only_when_fallback_opted_in() {
    // With no recipe/LLM but the opt-in fallback ON, the objective tier is used
    // instead of refusing — this is the P1 unblock.
    let kind = resolve_merge_judge_kind(false, false, true);
    assert_eq!(kind, MergeJudgeKind::Objective);
}

#[test]
fn judge_resolution_prefers_recipe_then_llm_over_objective() {
    // Objective is the LAST-resort fallback: a real reviewer always wins.
    assert_eq!(
        resolve_merge_judge_kind(true, false, true),
        MergeJudgeKind::Recipe
    );
    assert_eq!(
        resolve_merge_judge_kind(false, true, true),
        MergeJudgeKind::Llm
    );
    assert_eq!(
        resolve_merge_judge_kind(true, true, true),
        MergeJudgeKind::Recipe
    );
}

// ════════════════════════════════════════════════════════════════════════════
// 4. config: SIMARD_MERGE_OBJECTIVE_FALLBACK / SIMARD_MERGE_TRUSTED_AUTHORS
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn objective_fallback_defaults_off() {
    // Unset => OFF (fail-closed): deploying the code must NOT flip merge policy.
    assert!(!merge_objective_fallback_enabled_from(env(&[])));
}

#[test]
fn objective_fallback_enables_on_truthy_values() {
    for v in ["1", "true", "on", "yes", "TRUE", " on "] {
        assert!(
            merge_objective_fallback_enabled_from(env(&[(SIMARD_MERGE_OBJECTIVE_FALLBACK_ENV, v)])),
            "value {v:?} should enable the objective fallback"
        );
    }
}

#[test]
fn objective_fallback_stays_off_on_falsey_or_noise() {
    for v in ["0", "false", "off", "no", "", "  ", "maybe"] {
        assert!(
            !merge_objective_fallback_enabled_from(env(&[(
                SIMARD_MERGE_OBJECTIVE_FALLBACK_ENV,
                v
            )])),
            "value {v:?} must keep the objective fallback OFF (fail-closed)"
        );
    }
}

#[test]
fn trusted_authors_default_is_rysweet() {
    // Unset => the documented default single trusted author.
    assert_eq!(
        merge_trusted_authors_from(env(&[])),
        vec!["rysweet".to_string()]
    );
}

#[test]
fn trusted_authors_parses_trims_csv() {
    let got = merge_trusted_authors_from(env(&[(
        SIMARD_MERGE_TRUSTED_AUTHORS_ENV,
        " rysweet , second-user ,,third ",
    )]));
    assert_eq!(
        got,
        vec![
            "rysweet".to_string(),
            "second-user".to_string(),
            "third".to_string()
        ],
        "CSV is split, trimmed, empties dropped"
    );
}

#[test]
fn trusted_authors_rejects_logins_with_whitespace_or_slash() {
    // A GitHub login can never contain an internal space or a '/', so such an
    // entry is malformed/injected and must be dropped (defense-in-depth).
    let got = merge_trusted_authors_from(env(&[(
        SIMARD_MERGE_TRUSTED_AUTHORS_ENV,
        "good-user, bad user, owner/repo, ok2",
    )]));
    assert_eq!(
        got,
        vec!["good-user".to_string(), "ok2".to_string()],
        "internal-space and slash-bearing entries are rejected"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// 5. RefusingMergeJudge stays the DEFAULT — regression guard on fail-closed
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn refusing_judge_remains_available_and_not_configured() {
    // The objective tier must not remove or weaken the refusing default; the
    // regression risk is "RefusingMergeJudge ceases to be the default".
    let j = RefusingMergeJudge;
    assert_eq!(j.kind(), MergeJudgeKind::Refusing);
    assert!(!j.kind().is_configured());
    let out = j
        .judge(1, "rysweet/Simard", &green_snapshot_by("rysweet"))
        .unwrap();
    assert_eq!(out.verdict, Verdict::NotReady);
}
