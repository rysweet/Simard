//! TDD (Step 7) — FAILING tests for the P3 done-gate convergence fix.
//!
//! P3: a completed goal accumulates MULTIPLE competing "done-gate" PRs for the
//! same goal slug (retry churn without delivery) — e.g. the coin-benchmark
//! goal's 3 competing CLEAN done-gate PRs plus 5 stale CONFLICTING branches.
//! The fix converges a goal slug onto a SINGLE done-gate PR and prunes the
//! duplicates via LOGIC (never hand-closing PRs).
//!
//! These tests specify the pure convergence + slug-sanitisation contract. RED
//! until the following are implemented in `goal_curation::completion_gate` (and
//! re-exported at `crate::goal_curation`):
//!   * `sanitize_goal_slug`
//!   * `DoneGatePr`, `SlugConvergence`, `converge_done_gate_prs`
//!
//! Security invariants (see design `security_considerations`):
//!   * supersede ONLY bot-authored PRs whose slug EXACTLY matches — never a
//!     human's PR and never an unrelated goal;
//!   * sanitise the slug to `[a-z0-9-]` before it is used in a branch / argv /
//!     path (no `..`, path-sep, or shell metacharacters).
//!
//! Wire-in: `#[cfg(test)] mod tests_done_gate_dedup;` in
//! `src/goal_curation/mod.rs`.

use crate::goal_curation::completion_gate::{
    DoneGatePr, SlugConvergence, converge_done_gate_prs, sanitize_goal_slug,
};

const BOT: &str = "rysweet";
const SLUG: &str = "build-a-local-coin-benchmark-harness-09e65e35";

fn pr(number: u32, author: &str, slug: &str, mergeable: &str, created_at: &str) -> DoneGatePr {
    DoneGatePr {
        number,
        author: author.to_string(),
        slug: slug.to_string(),
        mergeable: mergeable.to_string(),
        created_at: created_at.to_string(),
    }
}

fn clean(number: u32, created_at: &str) -> DoneGatePr {
    pr(number, BOT, SLUG, "MERGEABLE", created_at)
}

fn dirty(number: u32, created_at: &str) -> DoneGatePr {
    pr(number, BOT, SLUG, "CONFLICTING", created_at)
}

// ════════════════════════════════════════════════════════════════════════════
// 1. sanitize_goal_slug — [a-z0-9-] only, no path/argv metacharacters
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn sanitize_preserves_a_valid_slug() {
    assert_eq!(sanitize_goal_slug(SLUG), SLUG);
}

#[test]
fn sanitize_lowercases() {
    assert_eq!(
        sanitize_goal_slug("Build-A-Local-Coin"),
        "build-a-local-coin"
    );
}

#[test]
fn sanitize_strips_path_and_shell_metacharacters() {
    // `..`, `/`, spaces, and shell metacharacters must be removed so the slug
    // can never traverse a path or inject an argv flag.
    for (raw, _why) in [
        ("../../etc/passwd", "path traversal"),
        ("goal;rm -rf /", "shell metachar"),
        ("goal name with spaces", "spaces"),
        ("goal/../slug", "embedded traversal"),
        ("--flag-like", "argv flag"),
        ("slug$(whoami)", "command substitution"),
    ] {
        let out = sanitize_goal_slug(raw);
        assert!(
            out.bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-')),
            "sanitised {raw:?} => {out:?} must only contain [a-z0-9-]"
        );
        assert!(
            !out.contains(".."),
            "sanitised {raw:?} => {out:?} must not contain '..'"
        );
        assert!(
            !out.starts_with('-'),
            "sanitised {raw:?} => {out:?} must not start with '-'"
        );
    }
}

#[test]
fn sanitize_is_idempotent() {
    let once = sanitize_goal_slug("Goal/../Name!!");
    assert_eq!(sanitize_goal_slug(&once), once);
}

// ════════════════════════════════════════════════════════════════════════════
// 2. converge_done_gate_prs — keep the OLDEST CLEAN, supersede the rest in scope
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn converges_competing_clean_prs_to_the_oldest() {
    // The 3 competing CLEAN done-gate PRs (#4326 oldest, #4329, #4332): keep the
    // oldest, supersede the newer duplicates.
    let prs = vec![
        clean(4332, "2026-07-20T10:00:00Z"),
        clean(4326, "2026-07-18T09:00:00Z"),
        clean(4329, "2026-07-19T09:00:00Z"),
    ];
    let SlugConvergence { keep, supersede } = converge_done_gate_prs(&prs, SLUG, BOT);
    assert_eq!(
        keep,
        Some(4326),
        "the oldest CLEAN PR is the single survivor"
    );
    let mut sup = supersede;
    sup.sort_unstable();
    assert_eq!(
        sup,
        vec![4329, 4332],
        "the newer CLEAN duplicates are superseded"
    );
}

#[test]
fn prunes_stale_conflicting_branches_alongside_the_keeper() {
    // The 5 stale CONFLICTING branches must be pruned via logic once a CLEAN
    // keeper is chosen — not left to accumulate.
    let prs = vec![
        clean(4326, "2026-07-18T09:00:00Z"),
        dirty(4161, "2026-07-01T09:00:00Z"),
        dirty(4149, "2026-07-02T09:00:00Z"),
        dirty(4134, "2026-07-03T09:00:00Z"),
    ];
    let SlugConvergence { keep, supersede } = converge_done_gate_prs(&prs, SLUG, BOT);
    assert_eq!(keep, Some(4326));
    let mut sup = supersede;
    sup.sort_unstable();
    assert_eq!(
        sup,
        vec![4134, 4149, 4161],
        "stale conflicting in-scope PRs are pruned"
    );
}

#[test]
fn single_clean_pr_is_kept_with_nothing_superseded() {
    let prs = vec![clean(4326, "2026-07-18T09:00:00Z")];
    let out = converge_done_gate_prs(&prs, SLUG, BOT);
    assert_eq!(out.keep, Some(4326));
    assert!(
        out.supersede.is_empty(),
        "a lone done-gate PR has no duplicates to prune"
    );
}

#[test]
fn no_clean_pr_keeps_nothing_and_supersedes_nothing() {
    // Fail-safe: when NO in-scope PR is mergeable, do not destroy the only
    // representatives — leave them for the stale-goal path to handle.
    let prs = vec![
        dirty(4161, "2026-07-01T09:00:00Z"),
        dirty(4149, "2026-07-02T09:00:00Z"),
    ];
    let out = converge_done_gate_prs(&prs, SLUG, BOT);
    assert_eq!(out.keep, None);
    assert!(out.supersede.is_empty());
}

// ════════════════════════════════════════════════════════════════════════════
// 3. Ownership scoping — never touch a human PR or a different goal slug
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn never_supersedes_a_pr_authored_by_a_human() {
    // A same-slug PR by a non-bot author must never be superseded.
    let prs = vec![
        clean(4326, "2026-07-18T09:00:00Z"),
        pr(
            9001,
            "some-human",
            SLUG,
            "MERGEABLE",
            "2026-07-19T09:00:00Z",
        ),
    ];
    let out = converge_done_gate_prs(&prs, SLUG, BOT);
    assert_eq!(out.keep, Some(4326));
    assert!(
        !out.supersede.contains(&9001),
        "a human-authored PR must never be superseded by the bot's convergence"
    );
}

#[test]
fn never_supersedes_a_pr_for_a_different_goal_slug() {
    let prs = vec![
        clean(4326, "2026-07-18T09:00:00Z"),
        pr(
            7777,
            BOT,
            "some-other-goal-deadbeef",
            "CONFLICTING",
            "2026-07-10T09:00:00Z",
        ),
    ];
    let out = converge_done_gate_prs(&prs, SLUG, BOT);
    assert_eq!(out.keep, Some(4326));
    assert!(
        !out.supersede.contains(&7777),
        "an out-of-slug PR must never be superseded"
    );
}

#[test]
fn does_not_pick_a_different_slug_as_keeper_even_if_older() {
    // The keeper must belong to the TARGET slug, not merely be the oldest CLEAN
    // PR overall.
    let prs = vec![
        pr(1000, BOT, "other-goal", "MERGEABLE", "2026-01-01T00:00:00Z"), // older, wrong slug
        clean(4326, "2026-07-18T09:00:00Z"),
    ];
    let out = converge_done_gate_prs(&prs, SLUG, BOT);
    assert_eq!(out.keep, Some(4326), "keeper must be in the target slug");
    assert!(out.supersede.is_empty());
}

#[test]
fn author_match_is_case_insensitive() {
    let prs = vec![
        pr(4326, "RySweet", SLUG, "MERGEABLE", "2026-07-18T09:00:00Z"),
        pr(4329, "RYSWEET", SLUG, "MERGEABLE", "2026-07-19T09:00:00Z"),
    ];
    let out = converge_done_gate_prs(&prs, SLUG, "rysweet");
    assert_eq!(out.keep, Some(4326));
    assert_eq!(out.supersede, vec![4329]);
}

#[test]
fn slug_is_sanitised_before_matching() {
    // A goal slug passed with stray casing/characters still matches PRs whose
    // slug is the sanitised canonical form.
    let prs = vec![clean(4326, "2026-07-18T09:00:00Z")];
    let out = converge_done_gate_prs(&prs, "Build-A-Local-Coin-Benchmark-Harness-09e65e35", BOT);
    assert_eq!(
        out.keep,
        Some(4326),
        "the target slug is sanitised to the canonical form before matching"
    );
}
