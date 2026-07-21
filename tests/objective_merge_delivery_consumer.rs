//! Outside-in consumer test (Step 13, #4389): exercises the delivery-stall fix
//! exactly as an OPERATOR would — through the public `simard` library boundary,
//! with no knowledge of internals.
//!
//! Scenario 1 (basic user-facing behaviour): an operator opts into the
//! objective merge fallback via `SIMARD_MERGE_OBJECTIVE_FALLBACK` + a
//! trusted-author allowlist; a green PR by a trusted author is now judged
//! `Ready` (previously always `NotReady`, which is the exact stall this PR
//! fixes), while an untrusted author and the overseer bot are refused.
//!
//! Scenario 2 (integration / edge cases): the hardened env parsing plus the
//! P2/P3 decision-layer functions an operator/consumer calls — self-deploy
//! per-SHA dedupe + head-advance (with an argv-injection guard) and done-gate
//! slug convergence.

use simard::goal_curation::completion_gate::{
    DoneGatePr, converge_done_gate_prs, sanitize_goal_slug,
};
use simard::overseer::config::{
    SIMARD_MERGE_OBJECTIVE_FALLBACK_ENV, SIMARD_MERGE_TRUSTED_AUTHORS_ENV,
    merge_objective_fallback_enabled_from, merge_trusted_authors_from,
};
use simard::self_deploy::head_advance::{
    DeployHeadState, DeployResult, UnitLoadState, classify_unit_load, is_valid_deploy_sha,
    needs_head_advance, should_deploy_target_sha, should_reconcile_unit,
};
use simard::stewardship::merge_authority::PrSnapshot;
use simard::stewardship::merge_judge::{
    MergeJudge, MergeJudgeKind, Verdict, resolve_merge_judge_kind,
};
use simard::stewardship::objective_merge_judge::ObjectiveMergeJudge;

/// Deterministic env resolver so the consumer path is exercised without
/// mutating the real process environment.
fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move |k: &str| map.get(k).cloned()
}

/// A green, mergeable PR snapshot authored by `author` (only `author_login`
/// drives the objective judge; the objective CI/mergeable gates run separately
/// downstream).
fn green_pr_by(author: &str) -> PrSnapshot {
    PrSnapshot {
        mergeable: "MERGEABLE".to_string(),
        author_login: author.to_string(),
        ..Default::default()
    }
}

// ── Scenario 1: basic user-facing behaviour ────────────────────────────────

#[test]
fn scenario1_operator_optin_lands_green_trusted_author_pr() {
    // Operator config: opt in + allowlist rysweet.
    let lookup = env(&[
        (SIMARD_MERGE_OBJECTIVE_FALLBACK_ENV, "1"),
        (SIMARD_MERGE_TRUSTED_AUTHORS_ENV, "rysweet"),
    ]);

    let optin = merge_objective_fallback_enabled_from(&lookup);
    let trusted = merge_trusted_authors_from(&lookup);
    assert!(optin, "operator opt-in flag must be read as enabled");
    assert_eq!(trusted, vec!["rysweet".to_string()]);

    // With no recipe/LLM provider wired but opt-in on, the resolver must pick
    // the Objective tier instead of the fail-closed Refusing default.
    let kind = resolve_merge_judge_kind(
        /* recipe_available */ false, /* llm_available */ false, optin,
    );
    assert_eq!(kind, MergeJudgeKind::Objective);

    // The green PR by the trusted author is now judged Ready — the delivery
    // stall (#4389) is fixed.
    let judge = ObjectiveMergeJudge::new(trusted, "simard-overseer[bot]".to_string());
    let verdict = judge
        .judge(4389, "rysweet/Simard", &green_pr_by("rysweet"))
        .expect("judge must not error")
        .verdict;
    assert_eq!(
        verdict,
        Verdict::Ready,
        "green PR by a trusted author must be Ready under the objective fallback"
    );
}

#[test]
fn scenario1_untrusted_author_and_bot_are_still_refused() {
    let judge = ObjectiveMergeJudge::new(
        vec!["rysweet".to_string()],
        "simard-overseer[bot]".to_string(),
    );

    // Untrusted human → NotReady.
    let untrusted = judge
        .judge(1, "rysweet/Simard", &green_pr_by("someone-else"))
        .unwrap()
        .verdict;
    assert_eq!(untrusted, Verdict::NotReady);

    // The overseer bot is excluded even if on the allowlist → no self-merge loop.
    let self_judge = ObjectiveMergeJudge::new(
        vec!["simard-overseer[bot]".to_string()],
        "simard-overseer[bot]".to_string(),
    );
    let bot = self_judge
        .judge(2, "rysweet/Simard", &green_pr_by("simard-overseer[bot]"))
        .unwrap()
        .verdict;
    assert_eq!(bot, Verdict::NotReady, "bot self-merge must be refused");

    // Default (no opt-in) stays fail-closed on Refusing.
    let default_kind = resolve_merge_judge_kind(false, false, false);
    assert_eq!(default_kind, MergeJudgeKind::Refusing);
}

// ── Scenario 2: integration / edge cases + decision layer ───────────────────

#[test]
fn scenario2_hardened_env_parsing_and_precedence() {
    // Default: flag unset → off; allowlist unset → default trusted author.
    let empty = env(&[]);
    assert!(!merge_objective_fallback_enabled_from(&empty));
    assert_eq!(
        merge_trusted_authors_from(&empty),
        vec!["rysweet".to_string()]
    );

    // CSV is trimmed; whitespace / slash-bearing entries are rejected.
    let dirty = env(&[(
        SIMARD_MERGE_TRUSTED_AUTHORS_ENV,
        " rysweet , bad name , org/team ,octocat ",
    )]);
    let parsed = merge_trusted_authors_from(&dirty);
    assert_eq!(parsed, vec!["rysweet".to_string(), "octocat".to_string()]);

    // Real reviewers always win over the objective fallback even when opted in.
    assert_eq!(
        resolve_merge_judge_kind(true, false, true),
        MergeJudgeKind::Recipe
    );
    assert_eq!(
        resolve_merge_judge_kind(false, true, true),
        MergeJudgeKind::Llm
    );
}

#[test]
fn scenario2_self_deploy_head_advance_dedupe_and_argv_guard() {
    let good = "a".repeat(40);
    let other = "b".repeat(40);

    // Argv-injection guard: only full lowercase-hex SHAs are deployable.
    assert!(is_valid_deploy_sha(&good));
    assert!(!is_valid_deploy_sha("--exec=rm -rf /"));
    assert!(!is_valid_deploy_sha(&good.to_uppercase()));

    // A SHA that already SUCCEEDED is deduped (anti-thrash, #4387)...
    let state = DeployHeadState {
        last_deploy_target_sha: Some(good.clone()),
        last_deploy_result: Some(DeployResult::Succeeded),
    };
    assert!(!should_deploy_target_sha(&state, &good));
    // ...but a genuinely new merged head still advances (#4305).
    assert!(should_deploy_target_sha(&state, &other));
    assert!(needs_head_advance(&good, &other));
    assert!(!needs_head_advance(&good, &good));
    // An argv-unsafe merged target never triggers an advance (fail-closed).
    assert!(!needs_head_advance(&good, "not-a-sha"));

    // A missing systemd unit is reconciled; a loaded one is left alone.
    assert_eq!(
        classify_unit_load(false, "Unit simard.service not found."),
        UnitLoadState::NotLoaded
    );
    assert!(should_reconcile_unit(UnitLoadState::NotLoaded));
    assert!(!should_reconcile_unit(classify_unit_load(true, "enabled")));
}

#[test]
fn scenario2_done_gate_slug_convergence() {
    let slug = "coin-benchmark-harness-09e65e35";
    let bot = "simard-overseer[bot]";

    let prs = vec![
        DoneGatePr {
            number: 4332,
            author: bot.to_string(),
            slug: slug.to_string(),
            mergeable: "MERGEABLE".to_string(),
            created_at: "2026-07-20T10:00:00Z".to_string(),
        },
        DoneGatePr {
            number: 4329,
            author: bot.to_string(),
            slug: slug.to_string(),
            mergeable: "MERGEABLE".to_string(),
            created_at: "2026-07-19T10:00:00Z".to_string(), // oldest CLEAN → survivor
        },
        DoneGatePr {
            number: 4326,
            author: bot.to_string(),
            slug: slug.to_string(),
            mergeable: "CONFLICTING".to_string(),
            created_at: "2026-07-18T10:00:00Z".to_string(),
        },
        // A human PR and a different-slug PR must never be touched.
        DoneGatePr {
            number: 999,
            author: "rysweet".to_string(),
            slug: slug.to_string(),
            mergeable: "MERGEABLE".to_string(),
            created_at: "2026-07-01T10:00:00Z".to_string(),
        },
        DoneGatePr {
            number: 888,
            author: bot.to_string(),
            slug: "some-other-goal".to_string(),
            mergeable: "MERGEABLE".to_string(),
            created_at: "2026-07-01T10:00:00Z".to_string(),
        },
    ];

    let converge = converge_done_gate_prs(&prs, slug, bot);
    assert_eq!(
        converge.keep,
        Some(4329),
        "oldest CLEAN in-scope PR survives"
    );
    let mut superseded = converge.supersede.clone();
    superseded.sort_unstable();
    assert_eq!(
        superseded,
        vec![4326, 4332],
        "newer clean + stale conflicting in-scope duplicates are superseded; human/other-slug untouched"
    );

    // Slug sanitisation lowercases and strips shell/path metacharacters and
    // whitespace (only [a-z0-9-] survive) before matching.
    assert_eq!(sanitize_goal_slug("Coin/Bench $lug!"), "coinbenchlug");
    assert_eq!(sanitize_goal_slug("Coin-Bench-42"), "coin-bench-42");
}
