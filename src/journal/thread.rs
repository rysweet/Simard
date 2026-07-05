//! The background journal thread (issue #2606).
//!
//! Once per interval the daemon runs a journal tick, which builds *today's*
//! [`DayContext`](crate::journal::types::DayContext) from the live
//! cognitive-memory store, generates the reviewed entry, and persists it. It is
//! a **rolling** update: the store keys each entry by its UTC day, so the many
//! ticks across a day supersede one another (idempotent) and the entry grows as
//! the day's remembered moments accumulate.
//!
//! There are two tick entry points, differing only in where the day's
//! code-change proposals come from:
//!
//! * [`run_journal_tick`] uses the offline [`NoNetworkPrs`] source (empty
//!   proposal table). Pure and network-free — used by tests and as a fallback.
//! * [`run_journal_tick_with_prs`] takes an injected [`PrListSource`]. In
//!   production the daemon passes a
//!   [`GhPrListSource`](crate::journal::pr_source::GhPrListSource) that wraps
//!   the `gh pr list` PR-readiness service, so the entry carries the real
//!   plain-language proposal table. That source degrades honestly to an empty
//!   list on a `gh` failure, and the daemon runs the tick on a background
//!   thread so the network fetch never stalls the authoritative OODA cycle.
//!
//! Either way, episodics (the primary source) and the active goals are read
//! straight from the borrowed store.

use chrono::NaiveDate;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::journal::generate::JournalGenerator;
use crate::journal::providers::{DayExtras, EpisodeSource, JournalClock, PrListSource};
use crate::journal::types::{JournalEntry, PrSummary};
use crate::memory_cognitive::CognitiveEpisode;

/// Opt-out switch for the daemon's journal thread. Default-on; set to a falsey
/// value to disable.
pub const JOURNAL_ENABLED_ENV: &str = "SIMARD_JOURNAL_ENABLED";
/// Override for the journal thread cadence, in seconds.
pub const JOURNAL_INTERVAL_ENV: &str = "SIMARD_JOURNAL_INTERVAL_SECS";
/// Default cadence: hourly. Frequent enough that the day's entry stays current
/// as moments accumulate, cheap enough to be invisible next to the OODA cycle.
pub const DEFAULT_JOURNAL_INTERVAL_SECS: u64 = 3600;
/// Lower bound so a misconfigured env cannot busy-loop the tick.
const MIN_JOURNAL_INTERVAL_SECS: u64 = 60;
/// How many recent episodes the in-daemon source pulls as the day's primary
/// narrative material.
const MAX_EPISODES: u32 = 500;

/// Whether the daemon's journal thread is enabled (default-on; opt-out via
/// [`JOURNAL_ENABLED_ENV`]).
#[must_use]
pub fn journal_enabled() -> bool {
    match std::env::var(JOURNAL_ENABLED_ENV) {
        Ok(v) => !matches!(v.trim(), "0" | "false" | "FALSE" | "no" | "off"),
        Err(_) => true,
    }
}

/// The journal thread cadence in seconds ([`JOURNAL_INTERVAL_ENV`] or
/// [`DEFAULT_JOURNAL_INTERVAL_SECS`]), clamped to a sane floor.
#[must_use]
pub fn journal_interval_secs() -> u64 {
    std::env::var(JOURNAL_INTERVAL_ENV)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_JOURNAL_INTERVAL_SECS)
        .max(MIN_JOURNAL_INTERVAL_SECS)
}

/// The day's episodic memories, read from the live store. Best-effort: it pulls
/// the most recent episodes as the day's narrative material rather than
/// precisely windowing by timestamp, so a day with fresh activity always has
/// moments to narrate and a truly idle store yields an honest quiet day.
struct MemoryEpisodeSource<'a> {
    mem: &'a dyn CognitiveMemoryOps,
    limit: u32,
}

impl EpisodeSource for MemoryEpisodeSource<'_> {
    fn episodes_for_date(&self, _date: NaiveDate) -> SimardResult<Vec<CognitiveEpisode>> {
        self.mem.list_all_episodes(self.limit)
    }
}

/// The in-daemon code-change-proposal source. Offline by design: it returns an
/// empty list so the tick never blocks on a network call. The narrative reports
/// this honestly ("No PRs were opened today"); the full plain-language proposal
/// table is produced whenever a richer [`PrListSource`] is injected.
struct NoNetworkPrs;

impl PrListSource for NoNetworkPrs {
    fn prs_for_date(&self, _date: NaiveDate) -> SimardResult<Vec<PrSummary>> {
        Ok(Vec::new())
    }
}

/// Gather best-effort augmentations from the borrowed store. Whatever is present
/// enriches the narrative; whatever is absent is simply omitted (honest
/// degradation). Today this folds in the active goals; the remaining
/// augmentation fields are left for future offline sources.
fn gather_extras(mem: &dyn CognitiveMemoryOps) -> DayExtras {
    let mut extras = DayExtras::default();
    if let Ok(board) = crate::goal_curation::load_goal_board(mem) {
        extras.goals = board
            .active
            .iter()
            .map(|g| g.description.clone())
            .filter(|d| !d.trim().is_empty())
            .collect();
    }
    extras
}

/// Run one rolling journal tick against the borrowed store using the **offline**
/// PR source ([`NoNetworkPrs`]): assemble today's context (episodics primary +
/// best-effort augmentations), generate the reviewed entry, and persist it.
/// `clock` fixes "today" (UTC) so the tick is deterministic under test. Returns
/// the entry that was stored.
///
/// This is the pure, network-free variant. Callers that can supply the day's
/// real code-change proposals (the daemon wraps the `gh pr list` PR-readiness
/// service behind a [`PrListSource`]) use [`run_journal_tick_with_prs`] instead.
pub fn run_journal_tick(
    mem: &dyn CognitiveMemoryOps,
    clock: &dyn JournalClock,
) -> SimardResult<JournalEntry> {
    run_journal_tick_with_prs(mem, clock, &NoNetworkPrs)
}

/// Run one rolling journal tick with an injected [`PrListSource`], so the day's
/// entry carries the real plain-language code-change-proposal table.
///
/// Everything except the proposal source is identical to [`run_journal_tick`]:
/// episodics (the primary source) and the active goals are read from the
/// borrowed store, the entry is drafted and jargon-reviewed, and it is persisted
/// under the UTC day key (idempotent rolling update). `prs` is the only seam
/// that may touch the network — in production the daemon passes a
/// [`GhPrListSource`](crate::journal::pr_source::GhPrListSource) that degrades
/// honestly to an empty table on a `gh` failure, so a network blip never fails
/// the tick.
pub fn run_journal_tick_with_prs(
    mem: &dyn CognitiveMemoryOps,
    clock: &dyn JournalClock,
    prs: &dyn PrListSource,
) -> SimardResult<JournalEntry> {
    let date = clock.today();
    let episodes = MemoryEpisodeSource {
        mem,
        limit: MAX_EPISODES,
    };
    let extras = gather_extras(mem);
    let generator = JournalGenerator::default_pipeline();
    let entry = crate::journal::providers::generate_and_store(
        date, &episodes, prs, extras, &generator, mem,
    )?;
    tracing::info!(
        target: "simard::journal",
        date = %entry.date,
        quiet_day = entry.quiet_day,
        episodes = entry.narrative.len(),
        prs = entry.prs.len(),
        "journal tick generated and stored today's entry"
    );
    Ok(entry)
}
