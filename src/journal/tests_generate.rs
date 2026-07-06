//! Tests: two-pass generation assembles a narrative from episodics + a PR
//! table, honours injected clock / episode / PR sources, and renders a quiet
//! day honestly (issue #2606).

use super::test_support::{FixedClock, FixedEpisodes, FixedPrs, day, episode, pr};
use crate::journal::generate::JournalGenerator;
use crate::journal::providers::{DayExtras, JournalClock, assemble_day_context};
use crate::journal::types::{DayContext, MemoryGrowth};

#[test]
fn narrative_is_built_from_episodics_and_prs() {
    let mut ctx = DayContext::new(day());
    ctx.episodes = vec![
        episode("reviewed the overnight work from the engineers"),
        episode("helped a stuck engineer get unblocked"),
    ];
    ctx.prs = vec![
        pr(12, "Made the dashboard load faster", "merged"),
        pr(15, "Fixed a crash on startup", "open"),
    ];

    let entry = JournalGenerator::default_pipeline().generate(&ctx);

    // The narrative leads with the episodic memories (the primary source).
    assert!(
        entry.narrative.contains("reviewed the overnight work"),
        "narrative must include the episodic content: {}",
        entry.narrative
    );
    assert!(
        entry.narrative.contains("helped a stuck engineer"),
        "narrative must include every episode: {}",
        entry.narrative
    );

    // The PR table is carried on the entry for rendering.
    assert_eq!(entry.prs.len(), 2);
    assert_eq!(entry.prs[0].number, 12);
    assert_eq!(entry.merged_pr_count(), 1);
    assert!(!entry.quiet_day);
}

#[test]
fn augmentations_enrich_the_narrative() {
    let mut ctx = DayContext::new(day());
    ctx.episodes = vec![episode("kept the system healthy")];
    ctx.goals = vec!["Ship the journal feature".to_string()];
    ctx.overseer_events = vec!["the Overseer approved a risky change".to_string()];
    ctx.memory_growth = Some(MemoryGrowth {
        facts_added: 4,
        episodes_added: 9,
    });

    let entry = JournalGenerator::default_pipeline().generate(&ctx);

    assert!(entry.narrative.contains("Ship the journal feature"));
    assert!(
        entry.narrative.contains("Overseer"),
        "the steward's activity must appear: {}",
        entry.narrative
    );
    assert!(
        entry.narrative.contains("4 new facts") && entry.narrative.contains("9 new"),
        "memory growth must appear (jargon-scrubbed): {}",
        entry.narrative
    );
}

#[test]
fn honest_degradation_when_no_prs() {
    let mut ctx = DayContext::new(day());
    ctx.episodes = vec![episode("a busy day of thinking")];
    // No PRs, but the day is not quiet (there are episodes).
    let entry = JournalGenerator::default_pipeline().generate(&ctx);
    assert!(!entry.quiet_day);
    assert!(
        entry.narrative.contains("No code-change proposals"),
        "must honestly state there were no proposals (jargon-scrubbed): {}",
        entry.narrative
    );
}

#[test]
fn injected_clock_episodes_and_prs_assemble_a_day_context() {
    let clock = FixedClock(day());
    let episodes = FixedEpisodes(vec![episode("did a thing"), episode("did another thing")]);
    let prs = FixedPrs(vec![pr(1, "a small fix", "merged")]);

    let ctx = assemble_day_context(clock.today(), &episodes, &prs, DayExtras::default())
        .expect("assemble day context");

    assert_eq!(ctx.date, day());
    assert_eq!(ctx.episodes.len(), 2);
    assert_eq!(ctx.prs.len(), 1);
    assert_eq!(ctx.prs[0].number, 1);
}

#[test]
fn quiet_day_renders_honestly() {
    let ctx = DayContext::new(day());
    assert!(ctx.is_quiet());

    let entry = JournalGenerator::default_pipeline().generate(&ctx);

    assert!(entry.quiet_day);
    assert!(entry.prs.is_empty());
    assert!(
        entry.narrative.to_lowercase().contains("quiet"),
        "a quiet day must say so: {}",
        entry.narrative
    );
    // Honest, not fabricated: no invented episode bullet points.
    assert!(!entry.narrative.contains("- "));
}

// ── Readable deterministic fallback (issues #2640/#2692, A3) ───────────────

/// The deterministic report drafter (the offline fallback) must NOT dump raw
/// error-log episodes verbatim into "Remembered moments". Historical episodes
/// that recorded a subprocess failure (e.g. the live journal E2BIG spawn error)
/// would otherwise fill a degraded journal with raw errno/jargon text — the
/// exact "journal full of raw error dumps" the operator reported.
#[test]
fn deterministic_fallback_strips_raw_error_log_episodes() {
    use crate::journal::generate::{JournalDrafter, TemplateDrafter};

    let mut ctx = DayContext::new(day());
    ctx.episodes = vec![
        episode("reviewed the overnight work from the engineers"),
        episode(
            "journal draft recipe failed; using the deterministic report drafter \
             error=base type 'journal' failed during invocation: recipe-runner-rs \
             spawn failed: Argument list too long (os error 7)",
        ),
    ];

    let draft = TemplateDrafter.draft(&ctx);

    // The healthy, human-readable moment survives.
    assert!(
        draft.contains("reviewed the overnight work"),
        "readable remembered moments must remain: {draft}"
    );
    // The raw historical E2BIG error dump must NOT appear anywhere in the report.
    for leak in [
        "Argument list too long",
        "os error 7",
        "recipe-runner-rs",
        "spawn failed",
    ] {
        assert!(
            !draft.contains(leak),
            "the deterministic fallback must strip raw error-log text {leak:?}: {draft}"
        );
    }
}
