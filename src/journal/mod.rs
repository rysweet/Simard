//! Simard's journal (issue #2606): a diary-like, layperson-readable narrative
//! of each day's activity, built largely from **episodic** memories and stored
//! in cognitive memory.
//!
//! One [`JournalEntry`] per day narrates what Simard — a single Brain — and its
//! steward, the Overseer, did that day: the moments it remembered, the goals it
//! worked, the updates it shipped to the live system, how its memory grew, and
//! a plain-language table of every code-change proposal (pull request). It is
//! written in a first-person-steward diary voice a non-engineer can follow.
//!
//! ## Pipeline
//!
//! 1. **Assemble** a [`DayContext`] from injectable seams
//!    ([`providers`]): the [`JournalClock`] fixes the day, the [`EpisodeSource`]
//!    supplies episodics (primary), the [`PrListSource`] supplies the day's
//!    proposals, and [`DayExtras`] carries the augmentations.
//! 2. **Draft then review** ([`generate`]): a [`JournalDrafter`] assembles the
//!    narrative and a **mandatory** [`JournalReviewer`] pass removes/explains
//!    jargon for a layperson.
//! 3. **Persist** ([`store`]): the reviewed [`JournalEntry`] is saved as a
//!    date-keyed semantic fact — idempotent rolling updates, searchable and
//!    browseable by date, surviving restarts.
//! 4. **Render** ([`render`]): pure, jargon-free, XSS-safe views feed both the
//!    dashboard Journal tab and the TUI Journal pane.
//!
//! The dashboard route/tab, the TUI pane widget, and the background cognitive
//! thread that drives [`generate_and_store`] daily are additive wiring layered
//! on top of these seams.

pub mod generate;
pub mod jargon;
pub mod pr_source;
pub mod providers;
pub mod render;
pub mod store;
pub mod thread;
pub mod types;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests_generate;
#[cfg(test)]
mod tests_jargon;
#[cfg(test)]
mod tests_pr_source;
#[cfg(test)]
mod tests_render;
#[cfg(test)]
mod tests_store;
#[cfg(test)]
mod tests_thread;

pub use generate::{
    GlossaryReviewer, JournalDrafter, JournalGenerator, JournalReviewer, TemplateDrafter,
};
pub use jargon::{JOURNAL_GLOSSARY, scrub_jargon};
pub use pr_source::{
    GhPrListSource, JOURNAL_PR_LIMIT, open_pr_to_summary, plainify_pr_title, pr_readiness_outcome,
};
pub use providers::{
    DayExtras, EpisodeSource, JournalClock, PrListSource, SystemClock, assemble_day_context,
    generate_and_store, generate_and_store_ops,
};
pub use render::{html_escape, render_entry_html, render_entry_tui_lines};
pub use store::{
    JOURNAL_CONCEPT_PREFIX, JOURNAL_TAG, JournalStore, all_entries, entry_matches,
    get_entry_by_date, journal_caller_key, query_entries, save_entry,
};
pub use thread::{
    journal_enabled, journal_interval_secs, run_journal_tick, run_journal_tick_with_prs,
};
pub use types::{DayContext, JournalEntry, MemoryGrowth, PrSummary};
