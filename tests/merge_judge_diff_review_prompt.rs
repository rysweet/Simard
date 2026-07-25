//! TDD (Step 7 — write tests first) for issue #4163: the merge-readiness judge
//! must REVIEW THE ACTUAL CHANGE, not grade the PR body against a rigid
//! six-heading template.
//!
//! Evidence: the Overseer's autonomous self-merge surveys ~28 CI-green +
//! MERGEABLE + reviewed engineer PRs but merges ZERO, because the judge prompt
//! (`prompt_assets/simard/merge_readiness_judge.md` and its recipe mirror
//! `prompt_assets/simard/recipes/merge-readiness-judge.yaml`) returns
//! `not_ready` for every PR whose body does not recite the six literal
//! merge-ready sections. Simard's OODA engineer-loop PRs carry substantive
//! write-ups but not that template, so the judge rejects genuinely-ready work
//! and does NO crusty-old-engineer review of the real diff.
//!
//! The target contract these tests pin (RED against the current template-grader
//! prompt, GREEN once the prompt is rewritten in Step 8):
//!
//!   1. The judge fetches evidence ITSELF via `gh` — the PR DIFF (`gh pr diff`)
//!      and check status (`gh pr checks`) — instead of grading only the body.
//!   2. The judge applies a crusty-old-engineer review to the CHANGE:
//!      correctness, sharp edges / hidden costs, scope creep, blast-radius /
//!      reversibility, tests for NEW behavior, docs for touched surfaces.
//!   3. The rigid "six evidence sections that MUST be present" body-template
//!      mandate is GONE — a substantive-but-non-templated description is
//!      acceptable when the change is sound, in-scope, tested and CI-green.
//!   4. The fail-closed verdict contract is PRESERVED: exactly the tokens
//!      `ready` / `not_ready` / `unclear`, with ambiguity → `unclear`.
//!   5. The file-channel transport is PRESERVED: the recipe reads
//!      `{{pr_body_path}}` (supplementary context), never the raw `{{pr_body}}`.
//!   6. Both the `.md` source and its `.yaml` recipe mirror carry the rewrite
//!      (mirror parity — a `.md` edit must not leave the live recipe stale).
//!
//! Fully hermetic: reads the shipped prompt assets from the source tree only.

use std::path::PathBuf;

fn asset(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("prompt asset {} must be readable: {e}", path.display()))
}

fn judge_md() -> String {
    asset("prompt_assets/simard/merge_readiness_judge.md").to_lowercase()
}

fn judge_yaml() -> String {
    asset("prompt_assets/simard/recipes/merge-readiness-judge.yaml").to_lowercase()
}

/// Assert `haystack` (already lowercased) contains at least one of `needles`.
fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

// ── 1. the judge fetches the actual diff + check status via `gh` ─────────────

#[test]
fn judge_md_instructs_fetching_the_pr_diff() {
    let md = judge_md();
    assert!(
        md.contains("gh pr diff"),
        "the judge prompt (.md) must instruct the agent to fetch the actual PR \
         DIFF via `gh pr diff` — it can no longer grade only the PR body"
    );
}

#[test]
fn judge_yaml_instructs_fetching_the_pr_diff() {
    let yaml = judge_yaml();
    assert!(
        yaml.contains("gh pr diff"),
        "the recipe mirror (.yaml) must instruct the agent to fetch the actual \
         PR DIFF via `gh pr diff`"
    );
}

#[test]
fn judge_reviews_check_status_itself() {
    // The judge reasons about the real change; it may re-read check status via
    // `gh pr checks` (the deterministic gate already confirmed CI-green, but the
    // judge is told how the objective gate was satisfied).
    for (name, body) in [("md", judge_md()), ("yaml", judge_yaml())] {
        assert!(
            body.contains("gh pr checks") || body.contains("check status"),
            "the judge prompt ({name}) must reference the PR check status \
             (`gh pr checks` / check status) it reasons over"
        );
    }
}

// ── 2. crusty-old-engineer review of the CHANGE (not body evidence) ──────────

#[test]
fn judge_applies_crusty_old_engineer_review_of_the_diff() {
    for (name, body) in [("md", judge_md()), ("yaml", judge_yaml())] {
        assert!(
            body.contains("crusty"),
            "the judge prompt ({name}) must apply a crusty-old-engineer review \
             to the change"
        );
        assert!(
            body.contains("correctness"),
            "crusty review ({name}) must assess correctness of the change"
        );
        assert!(
            body.contains("scope creep"),
            "crusty review ({name}) must assess scope creep"
        );
        assert!(
            contains_any(&body, &["reversib", "blast-radius", "blast radius"]),
            "crusty review ({name}) must assess blast-radius / reversibility"
        );
        assert!(
            body.contains("test"),
            "crusty review ({name}) must assess whether NEW behavior has tests"
        );
        assert!(
            contains_any(&body, &["document", "docs"]),
            "crusty review ({name}) must assess docs for touched public surfaces"
        );
    }
}

// ── 3. the rigid six-section body-template mandate is GONE ───────────────────

#[test]
fn judge_no_longer_mandates_six_literal_body_sections() {
    for (name, body) in [("md", judge_md()), ("yaml", judge_yaml())] {
        assert!(
            !body.contains("sections that must be present"),
            "the judge prompt ({name}) must NOT mandate the six literal body \
             sections — a substantive-but-non-templated description is \
             acceptable when the change is sound, in-scope, tested and CI-green"
        );
    }
}

#[test]
fn judge_verdict_is_substance_over_template() {
    // Positive signal: the rewritten prompt reasons about the SUBSTANCE of the
    // change rather than a heading checklist.
    for (name, body) in [("md", judge_md()), ("yaml", judge_yaml())] {
        assert!(
            body.contains("substance") || body.contains("substantive"),
            "the judge prompt ({name}) must judge the substance of the change"
        );
        assert!(
            contains_any(&body, &["in-scope", "in scope"]),
            "the judge prompt ({name}) must weigh whether the change is in-scope"
        );
    }
}

// ── 4. fail-closed verdict contract PRESERVED ────────────────────────────────

#[test]
fn judge_preserves_the_three_verdict_tokens() {
    for (name, body) in [("md", judge_md()), ("yaml", judge_yaml())] {
        for token in ["ready", "not_ready", "unclear"] {
            assert!(
                body.contains(token),
                "the judge prompt ({name}) must preserve the verdict token \
                 {token:?} (fail-closed contract in recipe_merge_judge.rs)"
            );
        }
        assert!(
            body.contains("unclear"),
            "ambiguity/parse-miss must map to `unclear` ({name}) — never \
             ready-without-verdict"
        );
    }
}

// ── 5. file-channel transport PRESERVED (reads pr_body_path, not pr_body) ─────

#[test]
fn recipe_reads_pr_body_path_not_raw_body() {
    let yaml = judge_yaml();
    assert!(
        yaml.contains("{{pr_body_path}}"),
        "the recipe mirror must still read the PR body via the file-channel var \
         {{{{pr_body_path}}}} (issues #2640/#2692)"
    );
    assert!(
        !yaml.contains("{{pr_body}}"),
        "the recipe mirror must NOT interpolate the raw {{{{pr_body}}}} payload \
         — the file channel exists to avoid inlining an unbounded body"
    );
}

// ── 6. mirror parity: both assets carry the diff-review rewrite ──────────────

#[test]
fn md_and_yaml_mirror_the_diff_review_rewrite() {
    let md = judge_md();
    let yaml = judge_yaml();
    // Both must have moved to diff review and neither may retain the rigid
    // six-section mandate — a rewrite of one that leaves the other stale is a
    // parity break (see recipe_mirrors_carry_the_g4_marker for the G4 analogue).
    assert!(
        md.contains("gh pr diff") && yaml.contains("gh pr diff"),
        "BOTH the .md and its .yaml mirror must instruct fetching the diff"
    );
    assert!(
        !md.contains("sections that must be present")
            && !yaml.contains("sections that must be present"),
        "BOTH the .md and its .yaml mirror must drop the six-section mandate"
    );
}
