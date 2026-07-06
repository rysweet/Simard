//! src/journal/tests_report_structure.rs
//!
//! Tests (issue #2606): the journal reads and is formatted as a professional,
//! third-person **narrative engineering & research report** — an Overview
//! paragraph, `##`-headed sections, timestamped chronological remembered
//! moments, and a verbose prepared-context summary — **not** a first-person
//! "Dear diary". Also covers the `episode_time_label` helper and the
//! prompt-first `JournalGenerator::for_repo` constructor's honest offline
//! fallback.
//!
//! These specify the TARGET behaviour and fail against the pre-fix #2618 build,
//! which uses a first-person diary drafter, carries no verbose prepared-context,
//! and renders episodes without timestamps.

use std::path::Path;

use super::test_support::{FixedEpisodes, FixedPrs, day, episode, episode_at, pr};
use crate::journal::generate::JournalGenerator;
use crate::journal::providers::{DayExtras, assemble_day_context, episode_time_label};
use crate::journal::types::{DayContext, MemoryGrowth};

/// The final report is third-person and carries NO diary voice anywhere — not in
/// the reviewed narrative and not even in the raw draft (the drafter itself is a
/// report writer, not a diarist).
#[test]
fn narrative_is_a_third_person_report_not_a_diary() {
    let mut ctx = DayContext::new(day());
    ctx.episodes = vec![
        episode("reviewed the overnight engineering work"),
        episode("started a measurement study of recall quality"),
    ];
    ctx.prs = vec![pr(
        12,
        "made the dashboard load faster",
        "still open — ready to combine into the main code",
    )];

    let entry = JournalGenerator::default_pipeline().generate(&ctx);
    let lower = entry.narrative.to_lowercase();

    assert!(
        !lower.contains("dear diary"),
        "no 'Dear diary' framing: {}",
        entry.narrative
    );
    assert!(
        !entry.narrative.contains("I, Simard"),
        "no first-person diary voice: {}",
        entry.narrative
    );
    assert!(
        !lower.contains("stayed with me"),
        "no confessional diary phrasing: {}",
        entry.narrative
    );
    assert!(
        !entry.draft.to_lowercase().contains("dear diary"),
        "the drafter must not use a diary voice: {}",
        entry.draft
    );
}

/// The report has a deliberate structure: an Overview paragraph plus at least one
/// `##`-headed section, and the final narrative keeps that structure.
#[test]
fn report_has_overview_and_sectioned_headings() {
    let mut ctx = DayContext::new(day());
    ctx.episodes = vec![episode("kept the system healthy")];
    ctx.goals = vec!["improve how Simard recalls its own past work".to_string()];
    ctx.deploys = vec!["shipped a faster dashboard to the live system".to_string()];
    ctx.overseer_events =
        vec!["the steward flagged a stale change and nudged it forward".to_string()];
    ctx.notable = vec!["a measurement study was started".to_string()];

    let entry = JournalGenerator::default_pipeline().generate(&ctx);

    assert!(
        entry.draft.contains("Overview"),
        "report leads with an Overview: {}",
        entry.draft
    );
    assert!(
        entry.draft.contains("## "),
        "report is split into '##' sections: {}",
        entry.draft
    );
    assert!(
        entry.narrative.contains("Overview"),
        "narrative keeps the Overview: {}",
        entry.narrative
    );
}

/// Remembered moments render WITH a timestamp and in chronological order
/// (oldest-to-newest), even when supplied newest-first.
#[test]
fn episodes_are_timestamped_and_chronological() {
    let mut ctx = DayContext::new(day());
    // Supplied newest-first to prove the drafter re-sorts oldest-to-newest.
    ctx.episodes = vec![
        episode_at("a later moment of the day", 1_700_003_600),
        episode_at("an earlier moment of the day", 1_700_000_000),
    ];

    let entry = JournalGenerator::default_pipeline().generate(&ctx);

    // Each moment shows when it occurred (an epoch magnitude => a UTC label).
    assert!(
        entry.draft.contains("UTC"),
        "remembered moments carry a timestamp label: {}",
        entry.draft
    );

    // Oldest-to-newest: the earlier moment appears before the later one.
    let earlier = entry
        .narrative
        .find("earlier moment")
        .expect("earlier moment present in the narrative");
    let later = entry
        .narrative
        .find("later moment")
        .expect("later moment present in the narrative");
    assert!(
        earlier < later,
        "moments must read oldest-to-newest: {}",
        entry.narrative
    );
}

/// `episode_time_label` formats an epoch-second magnitude as a UTC label and a
/// small monotonic counter as a stable ordinal (so counter-based test fixtures
/// still render sensibly).
#[test]
fn episode_time_label_handles_epoch_and_counter() {
    // 1_700_000_000 => 2023-11-14 22:13:20 UTC.
    let epoch = episode_time_label(1_700_000_000);
    assert!(
        epoch.contains("UTC"),
        "epoch magnitude => UTC label: {epoch}"
    );
    assert!(
        epoch.contains("2023"),
        "epoch is formatted as a real date: {epoch}"
    );

    let counter = episode_time_label(1);
    assert!(
        !counter.contains("UTC"),
        "a tiny counter is not a wall-clock time: {counter}"
    );
    assert!(
        counter.contains("moment 1"),
        "a counter degrades to 'moment N': {counter}"
    );
}

/// The verbose prepared-context summary reports the SUBSTANCE of the day's
/// facts / triggers / procedures, not a bare line of counts.
#[test]
fn prepared_context_is_verbose_not_counts() {
    let episodes = FixedEpisodes(vec![episode("watched the automated checks run")]);
    let prs = FixedPrs(vec![]);
    let extras = DayExtras {
        facts: vec!["the sign-in code now has automated tests".to_string()],
        triggers: vec!["a reminder to review a stale change fired".to_string()],
        procedures: vec!["the steps to safely combine a change into the main code".to_string()],
        ..DayExtras::default()
    };

    let ctx = assemble_day_context(day(), &episodes, &prs, extras).expect("assemble day context");
    // The extras are carried onto the DayContext as first-class material.
    assert_eq!(ctx.facts.len(), 1, "facts carried onto the context");
    assert_eq!(ctx.triggers.len(), 1, "triggers carried onto the context");
    assert_eq!(
        ctx.procedures.len(),
        1,
        "procedures carried onto the context"
    );

    let entry = JournalGenerator::default_pipeline().generate(&ctx);

    assert!(
        entry
            .narrative
            .contains("the sign-in code now has automated tests"),
        "fact substance appears in the report: {}",
        entry.narrative
    );
    assert!(
        entry
            .narrative
            .contains("a reminder to review a stale change fired"),
        "trigger substance appears in the report: {}",
        entry.narrative
    );
    assert!(
        entry
            .narrative
            .contains("safely combine a change into the main code"),
        "procedure substance appears in the report: {}",
        entry.narrative
    );
    assert!(
        !entry.narrative.contains("Prepared context:"),
        "no bare 'Prepared context: N facts' counts line: {}",
        entry.narrative
    );
}

/// `JournalGenerator::for_repo` builds a working generator that — even with no
/// resolvable recipe assets (honest offline fallback) — still produces a
/// structured, non-diary report whose mandatory review pass materially changes
/// the draft.
#[test]
fn for_repo_falls_back_to_a_structured_report() {
    // A repo root with no prompt assets forces the deterministic fallback.
    let generator = JournalGenerator::for_repo(Path::new("/nonexistent-journal-test-repo-root"));

    let mut ctx = DayContext::new(day());
    ctx.episodes = vec![episode("investigated a flaky check")];
    ctx.memory_growth = Some(MemoryGrowth {
        facts_added: 3,
        episodes_added: 5,
    });

    let entry = generator.generate(&ctx);
    assert!(
        !entry.narrative.to_lowercase().contains("dear diary"),
        "the fallback is still a report, not a diary: {}",
        entry.narrative
    );
    assert!(
        entry.draft.contains("Overview"),
        "the fallback still emits a structured report: {}",
        entry.draft
    );
    assert_ne!(
        entry.draft, entry.narrative,
        "the mandatory review pass still runs in the fallback"
    );
}
