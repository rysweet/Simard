//! Journal report-tone contract (issue #2711).
//!
//! Simard's daily journal must read as a professional NARRATIVE ENGINEERING &
//! RESEARCH REPORT — third-person, factual, reflective prose — and must NEVER
//! open as a personal diary ("Dear diary", "I, Simard", a confessional voice).
//!
//! This test pins that contract on the two surfaces that own the journal voice:
//!
//!   1. The prompt assets (the preferred, prompt-first production path): the two
//!      journal recipe YAMLs must MANDATE the report voice and FORBID the diary
//!      voice. We assert the PROHIBITION INSTRUCTION is PRESENT — deliberately
//!      NOT that the substring "Dear diary" is absent, because that phrase
//!      legitimately appears *inside* the negative instruction (and inside the
//!      deterministic glossary). Asserting its mere absence would false-fail.
//!
//!   2. The deterministic offline fallback (the honest path when the recipe
//!      runner is unavailable): even when diary phrasing is injected via
//!      untrusted day context, the generated narrative must come out diary-free
//!      and report-framed. This proves the anti-diary guarantee does not rely on
//!      the prompt text alone — the glossary scrubber is a defense-in-depth
//!      backstop.
//!
//! Fully hermetic: it reads the shipped YAMLs from the source tree and runs the
//! deterministic Rust pipeline. No recipe-runner spawn, no network, no real
//! credentials — synthetic fixtures only.

use std::path::PathBuf;

use chrono::NaiveDate;

use simard::journal::{DayContext, JOURNAL_GLOSSARY, JournalGenerator, scrub_jargon};

// ── Part A: the prompt-asset voice contract ─────────────────────────────────

/// Absolute path to a shipped recipe asset in the source tree (mirrors the
/// discovery logic in `tests/recipe_context_file_assets.rs`).
fn recipe_path(filename: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("prompt_assets/simard/recipes")
        .join(filename)
}

fn read_recipe(filename: &str) -> String {
    let path = recipe_path(filename);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("recipe asset {} must be readable: {e}", path.display()))
}

/// Read a recipe asset lowercased with runs of whitespace collapsed to single
/// spaces, so contract phrases match regardless of how the YAML prompt wraps
/// lines (the prompts are hard-wrapped, so `"diary\n      voice"` must still
/// match `"diary voice"`).
fn read_recipe_normalized(filename: &str) -> String {
    read_recipe(filename)
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The narrative (draft) recipe must instruct a professional engineering/
/// research REPORT voice and explicitly forbid the diary voice.
///
/// The prohibition NAMES the banned patterns, so the banned strings legitimately
/// appear in this asset — we therefore assert the negative INSTRUCTION is
/// PRESENT, never that the banned phrase is absent (that would false-fail).
#[test]
fn narrative_recipe_mandates_report_and_forbids_diary() {
    let lower = read_recipe_normalized("journal-narrative.yaml");

    assert!(
        lower.contains("report"),
        "journal-narrative.yaml must frame the journal as a REPORT"
    );
    assert!(
        lower.contains("third-person"),
        "journal-narrative.yaml must mandate third-person prose"
    );

    // Explicitly forbids the diary voice.
    assert!(
        lower.contains("never use a diary voice") || lower.contains("not a personal diary"),
        "journal-narrative.yaml must explicitly FORBID the diary voice"
    );
    assert!(
        lower.contains("dear diary"),
        "journal-narrative.yaml must call out \"Dear diary\" as a banned opener"
    );

    // The grounding constraint must survive any tone edit (indirect
    // prompt-injection guard: untrusted PR/fact text must never become fabricated
    // narrative events).
    assert!(
        lower.contains("use only what the input contains"),
        "journal-narrative.yaml must preserve the anti-fabrication grounding constraint"
    );
}

/// The plain-language (de-jargon) recipe must keep the report framing and must
/// not re-introduce a diary voice while rewriting.
#[test]
fn plain_language_recipe_keeps_report_and_forbids_diary() {
    let lower = read_recipe_normalized("journal-plain-language.yaml");

    assert!(
        lower.contains("report"),
        "journal-plain-language.yaml must keep the REPORT framing"
    );
    assert!(
        lower.contains("third-person"),
        "journal-plain-language.yaml must keep the third-person voice"
    );
    assert!(
        lower.contains("do not introduce a diary voice") || lower.contains("diary voice"),
        "journal-plain-language.yaml must forbid re-introducing a diary voice"
    );
    assert!(
        lower.contains("dear diary"),
        "journal-plain-language.yaml must name \"Dear diary\" as a banned opener"
    );
}

// ── Part B: the deterministic-pipeline anti-diary guarantee ─────────────────

fn a_day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 7, 6).expect("valid date")
}

/// Diary phrasing injected via untrusted day context must NOT survive into the
/// generated narrative: the deterministic glossary backstop strips it, and the
/// report structure is preserved.
#[test]
fn injected_diary_phrasing_is_scrubbed_from_generated_narrative() {
    let mut day = DayContext::new(a_day());
    // Untrusted, diary-formatted content sneaking in via a recorded "fact".
    day.facts
        .push("Dear diary, today Simard reflected on the reliability work.".to_string());
    day.goals.push("advance the reliability work".to_string());

    assert!(
        !day.is_quiet(),
        "fixture must exercise the active-day report path"
    );

    let entry = JournalGenerator::default_pipeline().generate(&day);

    assert!(
        !entry.narrative.to_lowercase().contains("dear diary"),
        "generated narrative must not contain a diary salutation; got:\n{}",
        entry.narrative
    );
    assert!(
        entry.narrative.contains("## Overview"),
        "narrative must open with the report's Overview section; got:\n{}",
        entry.narrative
    );
    assert!(
        !entry.narrative.contains("I, Simard"),
        "narrative must not use the confessional \"I, Simard\" voice; got:\n{}",
        entry.narrative
    );
}

/// A quiet day still renders an honest REPORT — never a diary — and stays
/// report-framed (`## Overview`) while honestly naming the day as quiet.
#[test]
fn quiet_day_report_is_diary_free_and_report_framed() {
    let day = DayContext::new(a_day());
    assert!(day.is_quiet(), "an empty context is a quiet day");

    let entry = JournalGenerator::default_pipeline().generate(&day);
    let lower = entry.narrative.to_lowercase();

    assert!(
        !lower.contains("dear diary"),
        "quiet-day narrative must be diary-free; got:\n{}",
        entry.narrative
    );
    assert!(
        entry.narrative.contains("## Overview"),
        "quiet-day narrative must still be a report (## Overview); got:\n{}",
        entry.narrative
    );
    assert!(
        lower.contains("quiet"),
        "quiet-day narrative must honestly say the day was quiet; got:\n{}",
        entry.narrative
    );
}

/// Unit pin on the deterministic backstop itself: the salutation maps to nothing
/// (whole-word, case-insensitive).
#[test]
fn scrub_jargon_strips_the_diary_salutation() {
    let scrubbed = scrub_jargon("Dear diary, the fix shipped today.");
    assert!(
        !scrubbed.to_lowercase().contains("dear diary"),
        "scrub_jargon must strip the diary salutation; got: {scrubbed:?}"
    );
}

/// The glossary must retain the anti-diary entry, so the deterministic path can
/// never regress to emitting a diary salutation even if the prompt text changes.
#[test]
fn glossary_retains_the_anti_diary_backstop() {
    assert!(
        JOURNAL_GLOSSARY
            .iter()
            .any(|(term, repl)| term.eq_ignore_ascii_case("dear diary") && repl.is_empty()),
        "JOURNAL_GLOSSARY must map \"dear diary\" -> \"\" (the anti-diary backstop)"
    );
}
