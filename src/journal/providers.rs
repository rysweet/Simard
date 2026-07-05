//! Injectable provider seams for journal generation (issue #2606).
//!
//! Every input the daily generator needs from the outside world is behind a
//! small trait so the whole pipeline runs offline in tests with fakes (no
//! network, no wall clock): the [`JournalClock`] fixes "today", the
//! [`EpisodeSource`] supplies the day's episodic memories (the primary
//! narrative source), and the [`PrListSource`] supplies the day's code-change
//! proposals. Augmentations the background thread gathers from other subsystems
//! (goals, deploys, Overseer activity, memory growth, notable events) are
//! passed in via [`DayExtras`].
//!
//! In production the [`SystemClock`] reads the UTC calendar day and adapters
//! wrap cognitive memory / the PR-readiness view behind [`EpisodeSource`] /
//! [`PrListSource`]; those adapters (and the background cognitive thread in
//! [`crate::journal::thread`] that the OODA daemon runs to drive
//! [`generate_and_store`]) consume exactly these seams.

use chrono::{NaiveDate, Utc};

use crate::error::SimardResult;
use crate::journal::generate::JournalGenerator;
use crate::journal::types::{DayContext, JournalEntry, MemoryGrowth, PrSummary};
use crate::memory_cognitive::CognitiveEpisode;

/// Supplies the current calendar day (UTC). Injected so tests are deterministic.
pub trait JournalClock: Send + Sync {
    /// The day the journal should treat as "today".
    fn today(&self) -> NaiveDate;
}

/// Production clock: the real UTC calendar day.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl JournalClock for SystemClock {
    fn today(&self) -> NaiveDate {
        Utc::now().date_naive()
    }
}

/// Supplies the episodic memories that belong to a given day — the primary
/// narrative source. The production adapter filters cognitive-memory episodes
/// to the day's UTC window; tests inject canned episodes.
pub trait EpisodeSource: Send + Sync {
    /// The day's episodes (may be empty on a quiet day).
    fn episodes_for_date(&self, date: NaiveDate) -> SimardResult<Vec<CognitiveEpisode>>;
}

/// Supplies the day's code-change proposals (pull requests) in plain language.
/// The production adapter wraps the existing PR-readiness / merge-judge view;
/// tests inject a fixed list.
pub trait PrListSource: Send + Sync {
    /// The day's code-change proposals (may be empty).
    fn prs_for_date(&self, date: NaiveDate) -> SimardResult<Vec<PrSummary>>;
}

/// The best-effort augmentations gathered from other subsystems for a day.
///
/// Each field is optional/allowed-empty; whatever is present enriches the
/// narrative and whatever is absent is simply omitted (honest degradation).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DayExtras {
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

/// Assemble the [`DayContext`] for `date` from the injected sources plus
/// `extras`. Episodics are pulled first (the primary source); the code-change
/// proposals and augmentations layer on top.
pub fn assemble_day_context(
    date: NaiveDate,
    episodes: &dyn EpisodeSource,
    prs: &dyn PrListSource,
    extras: DayExtras,
) -> SimardResult<DayContext> {
    Ok(DayContext {
        date,
        episodes: episodes.episodes_for_date(date)?,
        prs: prs.prs_for_date(date)?,
        goals: extras.goals,
        deploys: extras.deploys,
        overseer_events: extras.overseer_events,
        memory_growth: extras.memory_growth,
        notable: extras.notable,
    })
}

/// End-to-end: assemble the day, generate the reviewed entry, and persist it.
///
/// This is the single unit the background journal thread calls once per day
/// (and again as the day rolls forward — the store's caller-key dedup makes the
/// repeat idempotent). It persists through a borrowed
/// [`CognitiveMemoryOps`](crate::cognitive_memory::CognitiveMemoryOps) handle
/// (the thread runs against `ThreadContext::memory`, which it only holds by
/// reference) via [`store::save_entry`](crate::journal::store::save_entry).
/// Returns the entry that was stored.
pub fn generate_and_store(
    date: NaiveDate,
    episodes: &dyn EpisodeSource,
    prs: &dyn PrListSource,
    extras: DayExtras,
    generator: &JournalGenerator,
    mem: &dyn crate::cognitive_memory::CognitiveMemoryOps,
) -> SimardResult<JournalEntry> {
    let day = assemble_day_context(date, episodes, prs, extras)?;
    let entry = generator.generate(&day);
    crate::journal::store::save_entry(mem, &entry)?;
    Ok(entry)
}
