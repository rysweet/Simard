//! src/journal/tests_dejargon_teeth.rs
//!
//! Tests (issue #2606): the mandatory de-jargon rewrite has **teeth**. Given a
//! draft stuffed with representative jargon, the final `narrative` contains none
//! of a banned-token list (raw code identifiers, insider terms, unexpanded
//! acronyms, and diary phrasing), acronyms are **expanded** (not merely
//! parenthesised), and the review **materially changes** the text
//! (`draft != narrative`).
//!
//! Specifies the TARGET behaviour: in the pre-fix #2618 build these tokens still
//! survive (the review was effectively a no-op) and acronyms were left
//! parenthesised (e.g. "code-change proposal (PR)") rather than expanded.

use super::test_support::{day, episode, pr};
use crate::journal::generate::JournalGenerator;
use crate::journal::jargon::scrub_jargon;
use crate::journal::types::DayContext;

/// A representative (not exhaustive) banned-jargon token list, curated from the
/// observed production output. None of these may survive into the final
/// narrative — raw identifiers, insider terms, unexpanded acronyms, and the
/// diary phrase.
const BANNED_JARGON: &[&str] = &[
    "OODA",
    "episodic",
    "temporal_index",
    "working_memory",
    "daemon",
    "TUI",
    "LLM",
    "Dear diary",
];

/// Case-insensitive membership test. Multi-word phrases match as substrings;
/// single identifier-like tokens match on word boundaries (underscores are part
/// of the word, so `temporal_index` is one token).
fn contains_jargon(haystack: &str, needle: &str) -> bool {
    let hay = haystack.to_lowercase();
    let need = needle.to_lowercase();
    if needle.contains(' ') {
        return hay.contains(&need);
    }
    hay.split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|word| word == need)
}

#[test]
fn banned_jargon_never_survives_into_the_narrative() {
    let mut ctx = DayContext::new(day());
    ctx.episodes = vec![
        episode("Dear diary, the OODA loop finished cycle 7"),
        episode("sorted the episodic memories by temporal_index in working_memory"),
    ];
    ctx.notable = vec!["the daemon logged the TUI and LLM activity".to_string()];
    ctx.prs = vec![pr(
        12,
        "made the dashboard faster",
        "still open — ready to combine into the main code",
    )];

    let entry = JournalGenerator::default_pipeline().generate(&ctx);

    for token in BANNED_JARGON {
        assert!(
            !contains_jargon(&entry.narrative, token),
            "banned jargon {token:?} must not survive the de-jargon review: {}",
            entry.narrative
        );
    }
    assert_ne!(
        entry.draft, entry.narrative,
        "the de-jargon pass must materially rewrite the draft (not a no-op)"
    );
}

#[test]
fn acronyms_are_expanded_not_left_bare() {
    // The strengthened glossary EXPANDS acronyms into plain words rather than
    // leaving them bare or merely parenthesising them.
    let out = scrub_jargon("Opened a PR and waited for CI.");
    assert!(
        out.contains("pull request"),
        "'PR' expands to 'pull request': {out}"
    );
    assert!(
        !contains_jargon(&out, "PR"),
        "no bare 'PR' remains after expansion: {out}"
    );
    assert!(
        out.to_lowercase().contains("automated checks"),
        "'CI' expands to 'the automated checks': {out}"
    );
    assert!(
        !contains_jargon(&out, "CI"),
        "no bare 'CI' remains after expansion: {out}"
    );
}

#[test]
fn raw_identifiers_are_removed() {
    // Raw code identifiers a non-engineer would trip over are removed/explained.
    let out = scrub_jargon("The episodic entry kept its temporal_index in working_memory.");
    assert!(
        !contains_jargon(&out, "temporal_index"),
        "raw identifier 'temporal_index' removed: {out}"
    );
    assert!(
        !contains_jargon(&out, "working_memory"),
        "raw identifier 'working_memory' removed: {out}"
    );
    assert!(
        !contains_jargon(&out, "episodic"),
        "insider term 'episodic' removed/explained: {out}"
    );
}
