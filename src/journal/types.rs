//! Journal data types (issue #2606).
//!
//! A [`JournalEntry`] is Simard's diary-like narrative for a single day: a
//! layperson-readable, jargon-free story of what Simard (and its steward, the
//! Overseer) did, plus a plain-language table of the day's code-change
//! proposals. Entries are built largely from **episodic** memories and are
//! persisted in cognitive memory (see [`crate::journal::store`]).
//!
//! The transient [`DayContext`] is the assembled, injectable input to
//! generation — episodics are the primary/required source, augmented by the
//! day's code-change proposals, goals, live-system updates, Overseer activity,
//! memory growth, and notable events.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::memory_cognitive::CognitiveEpisode;

/// A plain-language summary of one code-change proposal (pull request) worked
/// on during the day.
///
/// `plain_summary` is the "what changed & why it matters" column a
/// non-engineer can follow; `outcome` is the human-readable disposition
/// (e.g. `"merged"`, `"open"`, `"closed"`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrSummary {
    /// The pull-request number (rendered as `#123`).
    pub number: u64,
    /// Plain-language "what changed & why it matters" — no jargon.
    pub plain_summary: String,
    /// Human-readable outcome: `"merged"`, `"open"`, `"closed"`, ...
    pub outcome: String,
}

/// How much Simard's memory grew over the day, in plain counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryGrowth {
    /// Number of new distilled facts learned during the day.
    pub facts_added: i64,
    /// Number of new remembered moments (episodes) captured during the day.
    pub episodes_added: i64,
}

/// The assembled, injectable input to journal generation for one day.
///
/// [`episodes`](Self::episodes) is the **primary** source — the narrative is
/// built largely from these moment-by-moment memories. Every other field is a
/// best-effort augmentation; a missing augmentation is simply omitted from the
/// narrative (honest degradation), it never fabricates content.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DayContext {
    /// The calendar day (UTC) this context describes.
    pub date: NaiveDate,
    /// Episodic memories for the day — the primary narrative source.
    pub episodes: Vec<CognitiveEpisode>,
    /// The day's code-change proposals (pull requests).
    pub prs: Vec<PrSummary>,
    /// Goals worked toward during the day.
    pub goals: Vec<String>,
    /// Updates shipped to the live system (deploys) during the day.
    pub deploys: Vec<String>,
    /// Steward (Overseer) activity during the day.
    pub overseer_events: Vec<String>,
    /// How much memory grew, if measured.
    pub memory_growth: Option<MemoryGrowth>,
    /// Any other notable events worth calling out.
    pub notable: Vec<String>,
}

impl DayContext {
    /// Create an empty context for `date` (a "quiet day" until populated).
    pub fn new(date: NaiveDate) -> Self {
        Self {
            date,
            ..Self::default()
        }
    }

    /// `true` when nothing happened worth narrating — no episodes, proposals,
    /// goals, deploys, Overseer activity, memory growth, or notable events.
    ///
    /// A quiet day still produces an honest entry (see the drafter) rather than
    /// an empty or fabricated one.
    pub fn is_quiet(&self) -> bool {
        self.episodes.is_empty()
            && self.prs.is_empty()
            && self.goals.is_empty()
            && self.deploys.is_empty()
            && self.overseer_events.is_empty()
            && self.notable.is_empty()
            && self.memory_growth.is_none()
    }
}

/// A single day's journal entry — the durable, persisted record.
///
/// [`narrative`](Self::narrative) is the final, reviewed, jargon-free prose
/// shown to operators. [`draft`](Self::draft) is the pre-review draft, retained
/// for provenance and so tests can assert the review pass actually changed the
/// text. [`prs`](Self::prs) backs the plain-language PR table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// The calendar day (UTC) this entry narrates. Doubles as the storage key.
    pub date: NaiveDate,
    /// When this entry was generated (rolling entries regenerate through the
    /// day, so this is the timestamp of the latest regeneration).
    pub generated_at: DateTime<Utc>,
    /// The final, reviewed, jargon-free narrative prose.
    pub narrative: String,
    /// The pre-review draft, retained for provenance / review-ran assertions.
    pub draft: String,
    /// Plain-language summaries of the day's code-change proposals.
    pub prs: Vec<PrSummary>,
    /// `true` when this was a quiet day (rendered honestly, not fabricated).
    pub quiet_day: bool,
}

impl JournalEntry {
    /// The number of the day's code-change proposals that were merged
    /// (combined into the main code).
    pub fn merged_pr_count(&self) -> usize {
        self.prs
            .iter()
            .filter(|p| p.outcome.eq_ignore_ascii_case("merged"))
            .count()
    }
}
