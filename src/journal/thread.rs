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

use std::path::Path;

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
/// Upper bound on how many prepared-context items (facts / triggers /
/// procedures) the report summarises — enough substance to be useful, bounded
/// so the entry stays readable (issue #2606).
const MAX_CONTEXT_ITEMS: u32 = 8;

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

/// Prefix of the bare working-memory context summary
/// ("Prepared context: N facts, M triggers, ...") that the consolidation path
/// pushes into working memory. It has no place among the journal's remembered
/// moments now that the report presents the prepared context's *substance*
/// (issue #2606), so the episode source drops it.
const CONTEXT_SUMMARY_PREFIX: &str = "Prepared context:";

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
        let mut episodes = self.mem.list_all_episodes(self.limit)?;
        // Drop the bare "Prepared context: N facts, ..." working-memory summary
        // lines; the report now narrates the substance of that context instead.
        episodes.retain(|e| !e.content.trim_start().starts_with(CONTEXT_SUMMARY_PREFIX));
        Ok(episodes)
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
/// enriches the report; whatever is absent is simply omitted (honest
/// degradation). This folds in the active goals plus the **substance** of the
/// prepared context (issue #2606): brief plain-language descriptions of the
/// facts Simard knows, the reminders (triggers) it holds, and the know-how
/// (procedures) it has — so the report summarises *what* they are rather than a
/// bare "N facts, M triggers, K procedures" count line. Each read is best-effort
/// and degrades to empty on error.
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
    if let Ok(facts) = mem.search_facts("", MAX_CONTEXT_ITEMS, 0.0) {
        extras.facts = facts
            .into_iter()
            .map(|f| f.content)
            .filter(|c| !c.trim().is_empty())
            .collect();
    }
    if let Ok(triggers) = mem.check_triggers("") {
        extras.triggers = triggers
            .into_iter()
            .map(|t| {
                if t.description.trim().is_empty() {
                    t.trigger_condition
                } else {
                    t.description
                }
            })
            .filter(|c| !c.trim().is_empty())
            .take(MAX_CONTEXT_ITEMS as usize)
            .collect();
    }
    if let Ok(procedures) = mem.recall_procedure("", MAX_CONTEXT_ITEMS) {
        extras.procedures = procedures
            .into_iter()
            .map(|p| {
                if p.steps.is_empty() {
                    p.name
                } else {
                    format!("{}: {}", p.name, p.steps.join("; "))
                }
            })
            .filter(|c| !c.trim().is_empty())
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
    // Deterministic, offline generator — the network-free variant used by tests
    // and as the honest fallback.
    tick_with_generator(mem, clock, prs, JournalGenerator::default_pipeline())
}

/// Run one rolling journal tick with an injected [`PrListSource`] and the
/// **prompt-first** generator for `repo_root` (issue #2606, guideline G3).
///
/// Identical to [`run_journal_tick_with_prs`] except the report and its
/// plain-language rewrite are produced by the language-model recipe path when
/// the journal recipe assets and runner are available for `repo_root`; it
/// degrades to the deterministic report drafter + glossary reviewer otherwise.
/// This is the entry point the daemon uses in production.
pub fn run_journal_tick_with_prs_in_repo(
    mem: &dyn CognitiveMemoryOps,
    clock: &dyn JournalClock,
    prs: &dyn PrListSource,
    repo_root: &Path,
) -> SimardResult<JournalEntry> {
    tick_with_generator(mem, clock, prs, JournalGenerator::for_repo(repo_root))
}

/// Shared tick core: assemble today's context (episodics primary + best-effort
/// augmentations), generate the reviewed entry with `generator`, and persist it.
fn tick_with_generator(
    mem: &dyn CognitiveMemoryOps,
    clock: &dyn JournalClock,
    prs: &dyn PrListSource,
    generator: JournalGenerator,
) -> SimardResult<JournalEntry> {
    let date = clock.today();
    let episodes = MemoryEpisodeSource {
        mem,
        limit: MAX_EPISODES,
    };
    let extras = gather_extras(mem);
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
