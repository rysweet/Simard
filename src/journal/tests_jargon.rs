//! Tests: the jargon scrubber removes/explains jargon (whole-word), and the
//! mandatory review pass runs on every generation and scrubs the draft
//! (issue #2606).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::test_support::{CountingReviewer, day, episode, pr};
use crate::journal::generate::{JournalGenerator, TemplateDrafter};
use crate::journal::jargon::scrub_jargon;
use crate::journal::types::DayContext;

#[test]
fn scrub_removes_insider_acronyms() {
    let out = scrub_jargon("We finished the OODA loop, then started another OODA cycle.");
    assert!(!out.contains("OODA"), "OODA must be gone: {out}");
    assert!(out.contains("decision cycle"), "OODA is replaced: {out}");
}

#[test]
fn scrub_expands_pull_request_acronym() {
    let out = scrub_jargon("Opened PR 12 today.");
    assert!(
        out.contains("pull request"),
        "PR is expanded to plain words: {out}"
    );
    assert!(!out.contains("PR "), "no bare 'PR' acronym remains: {out}");
}

#[test]
fn scrub_handles_plurals_and_merge() {
    let out = scrub_jargon("We merged 3 PRs.");
    assert!(
        out.contains("pull requests"),
        "plural acronym expanded: {out}"
    );
    assert!(
        out.contains("combined into the main code"),
        "'merged' rewritten in plain language: {out}"
    );
}

#[test]
fn scrub_is_whole_word_only() {
    // Terms embedded inside larger words must NOT be rewritten: `PR` inside
    // `approve`, `CI` inside `social`.
    let out = scrub_jargon("The reviewer will approve the social change.");
    assert_eq!(
        out, "The reviewer will approve the social change.",
        "no whole-word jargon present, so nothing should change: {out}"
    );
}

#[test]
fn scrub_explains_domain_terms() {
    let out = scrub_jargon("An episodic memory and a deploy to the daemon.");
    assert!(
        out.contains("moment-by-moment"),
        "episodic explained: {out}"
    );
    assert!(out.contains("live system"), "deploy explained: {out}");
    assert!(
        out.contains("always-on background service"),
        "daemon explained: {out}"
    );
}

#[test]
fn mandatory_review_pass_runs_once_and_scrubs_jargon() {
    let calls = Arc::new(AtomicUsize::new(0));
    let generator = JournalGenerator::new(
        Box::new(TemplateDrafter),
        Box::new(CountingReviewer::new(Arc::clone(&calls))),
    );

    let mut ctx = DayContext::new(day());
    ctx.episodes = vec![episode("completed the OODA loop for cycle 7")];
    ctx.prs = vec![pr(12, "made the dashboard faster", "merged")];

    let entry = generator.generate(&ctx);

    // The review pass is mandatory — it ran exactly once for this generation.
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the review pass must run exactly once per generation"
    );

    // The raw draft still carries jargon; the reviewed narrative does not.
    assert!(entry.draft.contains("OODA"), "draft retains raw jargon");
    assert!(
        !entry.narrative.contains("OODA"),
        "review removed the OODA jargon: {}",
        entry.narrative
    );
    assert!(
        entry.narrative.contains("code-change proposal"),
        "review explained the PR jargon: {}",
        entry.narrative
    );
    assert_ne!(
        entry.draft, entry.narrative,
        "review must actually rewrite the draft"
    );
}
