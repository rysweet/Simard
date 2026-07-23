//! Simard's journal (issue #2606): a layperson-readable **narrative
//! engineering & research report** of each day's activity, built largely from
//! **episodic** memories and stored in cognitive memory.
//!
//! One [`JournalEntry`] per day narrates, in professional third-person prose,
//! what Simard — a single Brain — and its steward, the Overseer, did that day:
//! an Overview paragraph, clearly delineated sections (engineering work,
//! research and findings, key observations), a chronological timestamped list
//! of the moments it remembered, and a plain-language table of every
//! code-change proposal (pull request). It reads as a report a non-engineer can
//! follow — not a personal diary.
//!
//! ## Pipeline
//!
//! 1. **Assemble** a [`DayContext`] from injectable seams
//!    ([`providers`]): the [`JournalClock`] fixes the day, the [`EpisodeSource`]
//!    supplies episodics (primary), the [`PrListSource`] supplies the day's
//!    proposals, and [`DayExtras`] carries the augmentations (including the
//!    prepared-context substance — facts, triggers, procedures).
//! 2. **Draft then review** ([`generate`]): a [`JournalDrafter`] assembles the
//!    report and a **mandatory** [`JournalReviewer`] pass removes/explains
//!    jargon for a layperson; the preferred production path is prompt-first
//!    ([`recipe`]), degrading to the deterministic report drafter + glossary
//!    reviewer. A secret-redaction post-pass always runs last.
//! 3. **Persist** ([`store`]): the reviewed [`JournalEntry`] is saved as a
//!    date-keyed semantic fact — idempotent rolling updates, searchable and
//!    browseable by date, surviving restarts.
//! 4. **Render** ([`render`]): pure, jargon-free, XSS-safe views turn the
//!    report's markdown structure into real headings/lists/tables for both the
//!    dashboard Journal tab and the TUI Journal pane.
//!
//! The dashboard route/tab, the TUI pane widget, and the background cognitive
//! thread that drives [`generate_and_store`] daily are additive wiring layered
//! on top of these seams.

pub mod generate;
pub mod jargon;
pub mod pr_source;
pub mod providers;
pub mod recipe;
pub mod reconcile;
pub mod render;
pub mod store;
pub mod thread;
pub mod types;

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(all(test, unix))]
mod tests_clean_result_channel;
#[cfg(test)]
mod tests_dejargon_teeth;
#[cfg(test)]
mod tests_generate;
#[cfg(test)]
mod tests_jargon;
#[cfg(test)]
mod tests_pr_source;
#[cfg(test)]
mod tests_reconcile;
#[cfg(test)]
mod tests_render;
#[cfg(test)]
mod tests_render_report;
#[cfg(test)]
mod tests_report_structure;
#[cfg(test)]
mod tests_secrets;
#[cfg(test)]
mod tests_store;
#[cfg(test)]
mod tests_thread;

pub use generate::{
    GlossaryReviewer, JournalDrafter, JournalGenerator, JournalReviewer, TemplateDrafter,
};
pub use jargon::{JOURNAL_GLOSSARY, scrub_jargon, scrub_secrets};
pub use pr_source::{
    GhPrListSource, JOURNAL_PR_LIMIT, open_pr_to_summary, plainify_pr_title, pr_readiness_outcome,
};
pub use providers::{
    DayExtras, EpisodeSource, JournalClock, PrListSource, SystemClock, assemble_day_context,
    episode_time_label, generate_and_store,
};
pub use recipe::{RecipeDrafter, RecipeReviewer};
pub use reconcile::{
    DEFAULT_RECONCILE_LOOKBACK_DAYS, GhMergedPrSource, JOURNAL_RECONCILE_DAYS_ENV,
    MAX_RECONCILE_LOOKBACK_DAYS, MergedPrSource, ReconcileReport, reconcile_entry,
    reconcile_lookback_days, reconcile_lookback_days_from, reconcile_recent_days,
};
pub use render::{html_escape, render_entry_html, render_entry_tui_lines};
pub use store::{
    JOURNAL_CONCEPT_PREFIX, JOURNAL_TAG, JournalStore, all_entries, entry_matches,
    get_entry_by_date, journal_caller_key, query_entries, save_entry,
};
pub use thread::{
    journal_enabled, journal_interval_secs, run_journal_tick, run_journal_tick_with_prs,
    run_journal_tick_with_prs_in_repo,
};
pub use types::{DayContext, JournalEntry, MemoryGrowth, PrSummary};
