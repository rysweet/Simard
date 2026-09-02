//! Past-day merged-PR reconciliation (issue #4225).
//!
//! A journal entry only (re)generates while it is *today*: once the day passes
//! it **freezes** (see [`crate::journal::thread::run_journal_tick_with_prs`],
//! which builds only `clock.today()`). Two things follow from that freeze:
//!
//! 1. Entries generated **before** the day's merged-PR wiring (#4140) shipped
//!    froze with `merged_pr_count() == 0` and are never revisited.
//! 2. Even post-#4140, any PR that merges **after** a day's final tick is
//!    missed, so every day silently loses its tail-of-day merges.
//!
//! The dashboard read layer faithfully reports the frozen count, so a day that
//! shipped ten PRs can show `merged: 0` forever. This module closes that gap
//! with a **reconciliation** pass that, on each journal tick, revisits the last
//! few *past* days and folds their real merges into the stored (frozen) entry —
//! upgrading a "still open" row to `merged` and appending any merged PR the
//! entry never saw.
//!
//! ## Merged-only seam
//!
//! Reconciliation flows through [`MergedPrSource`], a deliberately
//! **merged-only** seam that yields *only* the PRs that landed on a date. This
//! is a hard safety property, not an accident: a past-day backfill must never
//! be able to graft *today's* still-open PRs onto a historical entry, so the
//! seam simply cannot express an open PR. The production adapter
//! [`GhMergedPrSource`] wraps the same
//! `gh pr list --state merged --search "merged:<date>"` service the Journal tab
//! already uses (#4140).
//!
//! ## Safety invariants
//!
//! [`reconcile_entry`] is pure and honours these invariants:
//!
//! * **Idempotent** — an entry that already reflects every merge is left
//!   untouched (`None`), so repeated ticks do not churn the store.
//! * **Never downgrades or erases** — an existing row is only ever *upgraded*
//!   from open to `merged`; no row is removed and no `merged` row is reverted.
//! * **Additive** — a merged PR the frozen entry never saw is appended.
//!
//! The driver [`reconcile_recent_days`] adds the rest: it **never touches
//! today** (that is the live tick's job), **skips absent days** (no entry ⇒
//! nothing to reconcile), and **degrades honestly** — a `gh` blip for one day
//! is logged and skipped rather than failing the whole pass or erasing data.

use chrono::{DateTime, Duration, NaiveDate, Utc};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::journal::pr_source::merged_pr_to_summary;
use crate::journal::store::{get_entry_by_date, save_entry};
use crate::journal::types::{JournalEntry, PrSummary};
use crate::stewardship::merge_authority::PrGhClient;

/// The canonical outcome token a merged row carries — the exact string
/// [`JournalEntry::merged_pr_count`](crate::journal::types::JournalEntry::merged_pr_count)
/// counts. Kept in step with [`crate::journal::pr_source::merged_pr_to_summary`].
const MERGED_OUTCOME: &str = "merged";

/// Env var overriding how many past days a reconciliation pass revisits.
pub const JOURNAL_RECONCILE_DAYS_ENV: &str = "SIMARD_JOURNAL_RECONCILE_DAYS";
/// Default number of past days revisited per pass. A week comfortably covers
/// entries that froze before #4140 landed plus every day's tail-of-day merges,
/// while staying cheap next to the OODA cycle.
pub const DEFAULT_RECONCILE_LOOKBACK_DAYS: u32 = 7;
/// Ceiling so a misconfigured env cannot make one tick fan out into an
/// unbounded number of `gh` calls.
pub const MAX_RECONCILE_LOOKBACK_DAYS: u32 = 30;

/// How many past days the reconciliation pass revisits: [`JOURNAL_RECONCILE_DAYS_ENV`]
/// if set and parseable, else [`DEFAULT_RECONCILE_LOOKBACK_DAYS`], clamped to
/// [`MAX_RECONCILE_LOOKBACK_DAYS`]. A `0` disables reconciliation (the loop runs
/// zero iterations) without any special-casing.
#[must_use]
pub fn reconcile_lookback_days() -> u32 {
    reconcile_lookback_days_from(std::env::var(JOURNAL_RECONCILE_DAYS_ENV).ok().as_deref())
}

/// Pure parse-and-clamp of the raw [`JOURNAL_RECONCILE_DAYS_ENV`] value (or
/// `None` when unset), split out from [`reconcile_lookback_days`] so the parsing
/// and clamping is unit-testable **without mutating the process environment** —
/// `set_var`/`remove_var` are unsound while the rest of the (parallel) test
/// binary may be reading the environment.
#[must_use]
pub fn reconcile_lookback_days_from(raw: Option<&str>) -> u32 {
    raw.and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_RECONCILE_LOOKBACK_DAYS)
        .min(MAX_RECONCILE_LOOKBACK_DAYS)
}

/// Supplies the pull requests that **merged** on a given day, in journal
/// [`PrSummary`] form (each already tagged with the canonical `"merged"`
/// outcome). Deliberately merged-only — see the module docs — so a past-day
/// backfill can never surface an open PR.
pub trait MergedPrSource: Send + Sync {
    /// The PRs that landed on `date` (may be empty). An error signals a
    /// transient fetch failure; the driver degrades honestly and skips the day.
    fn merged_prs_for_date(&self, date: NaiveDate) -> SimardResult<Vec<PrSummary>>;
}

/// Production [`MergedPrSource`] over the `gh pr list --state merged` service.
///
/// Wraps a stewardship [`PrGhClient`] (in production the `gh`-shelling
/// [`RealPrGhClient`](crate::stewardship::RealPrGhClient)) and maps each
/// [`MergedPrSummary`](crate::stewardship::merge_authority::MergedPrSummary) into
/// a journal row via [`merged_pr_to_summary`]. Because it touches the network it
/// is driven off the hot OODA path (the spawned journal tick), never inline.
pub struct GhMergedPrSource<'a> {
    gh: &'a (dyn PrGhClient + Send + Sync),
    repo: &'a str,
    limit: u32,
}

impl<'a> GhMergedPrSource<'a> {
    /// Wrap `gh` for `repo`, fetching up to [`JOURNAL_PR_LIMIT`](crate::journal::JOURNAL_PR_LIMIT)
    /// merged PRs per day.
    #[must_use]
    pub fn new(gh: &'a (dyn PrGhClient + Send + Sync), repo: &'a str) -> Self {
        Self {
            gh,
            repo,
            limit: crate::journal::pr_source::JOURNAL_PR_LIMIT,
        }
    }

    /// Override the `gh pr list` page size (default
    /// [`JOURNAL_PR_LIMIT`](crate::journal::JOURNAL_PR_LIMIT)).
    #[must_use]
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }
}

impl MergedPrSource for GhMergedPrSource<'_> {
    fn merged_prs_for_date(&self, date: NaiveDate) -> SimardResult<Vec<PrSummary>> {
        let merged = self.gh.list_merged_prs(self.repo, date, self.limit)?;
        Ok(merged.iter().map(merged_pr_to_summary).collect())
    }
}

/// Fold the day's real `merged` PRs (`merged`) into a stored, frozen `entry`.
///
/// Returns `Some(updated)` only when the fold actually changed something —
/// `None` when the entry already reflects every merge (idempotent). The update
/// is strictly additive: an existing open row for a merged PR is *upgraded* to
/// `merged` (its frozen `plain_summary` is preserved), a merged PR the entry
/// never saw is *appended*, and nothing is ever removed or downgraded. When a
/// merge is folded into a day previously marked quiet, `quiet_day` flips to
/// `false` (a day that shipped code is not a quiet day). `now` stamps the
/// reconciled entry's `generated_at` so it wins the store's newest-generated
/// tiebreak over any stale duplicate.
///
/// Only rows the seam tags `"merged"` are folded; any other row is ignored as a
/// defensive guard, though the [`MergedPrSource`] contract already promises
/// merged-only input.
#[must_use]
pub fn reconcile_entry(
    entry: &JournalEntry,
    merged: &[PrSummary],
    now: DateTime<Utc>,
) -> Option<JournalEntry> {
    let mut prs = entry.prs.clone();
    let mut changed = false;

    for m in merged {
        // Defensive: the seam is merged-only, but never fold a non-merged row.
        if !m.outcome.eq_ignore_ascii_case(MERGED_OUTCOME) {
            continue;
        }

        // If ANY row for this PR number is already merged, the merge is already
        // reflected — do nothing. This is what keeps the fold idempotent AND
        // guards the `[open #N, merged #N]` shape a single live tick can persist
        // (the production source appends open rows before merged rows, so a PR
        // that merges between those two fetches lands as both). Upgrading the
        // open row in that case would double-count the PR as merged.
        let already_merged = prs
            .iter()
            .any(|p| p.number == m.number && p.outcome.eq_ignore_ascii_case(MERGED_OUTCOME));
        if already_merged {
            continue;
        }

        match prs.iter_mut().find(|p| p.number == m.number) {
            // Upgrade the (open) row to merged; keep the frozen summary. We only
            // reach here when no row for this number is merged yet, so this can
            // never create a second merged row.
            Some(existing) => {
                existing.outcome = MERGED_OUTCOME.to_string();
                changed = true;
            }
            // A landed change the frozen entry never saw — append it.
            None => {
                prs.push(m.clone());
                changed = true;
            }
        }
    }

    if !changed {
        return None;
    }

    // A day that shipped code is not a quiet day; a still-empty table stays as
    // it was. `prs` is only ever grown above, so this only ever clears the flag.
    let quiet_day = entry.quiet_day && prs.is_empty();

    Some(JournalEntry {
        date: entry.date,
        generated_at: now,
        narrative: entry.narrative.clone(),
        draft: entry.draft.clone(),
        prs,
        quiet_day,
    })
}

/// The outcome of one [`reconcile_recent_days`] pass, for logging.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Past days that had a stored entry and were checked against reality.
    pub days_examined: u32,
    /// Past days whose stored entry was updated (a merge folded in).
    pub days_updated: u32,
    /// Past days skipped because their merged-PR fetch failed (honest
    /// degradation — never an error, never data loss).
    pub days_degraded: u32,
}

/// Revisit the last `lookback_days` **past** days and fold each one's real
/// merges into its stored (frozen) entry via `merged`.
///
/// For each day from `today - 1` back to `today - lookback_days`:
///
/// * **Absent day** (no stored entry) is skipped — there is nothing to
///   reconcile and reconciliation never *creates* an entry.
/// * **Fetch blip** (the merged-PR read errors) is logged and skipped
///   ([`ReconcileReport::days_degraded`]); the pass carries on to the other
///   days and the frozen entry is left exactly as-is.
/// * Otherwise [`reconcile_entry`] folds the day's merges in and, when that
///   changed anything, the upgraded entry is saved under the same day key
///   (idempotent supersede).
///
/// **Today is never touched** — the loop starts at `today - 1`, leaving today to
/// the live [`run_journal_tick_with_prs`](crate::journal::thread::run_journal_tick_with_prs).
/// A `lookback_days` of `0` runs zero iterations (a clean no-op).
///
/// A store read/write error *does* propagate (that is real corruption, not a
/// transient blip); a `gh` fetch error never does.
pub fn reconcile_recent_days(
    mem: &dyn CognitiveMemoryOps,
    merged: &dyn MergedPrSource,
    today: NaiveDate,
    lookback_days: u32,
) -> SimardResult<ReconcileReport> {
    let now = Utc::now();
    let mut report = ReconcileReport::default();

    for delta in 1..=lookback_days {
        let Some(date) = today.checked_sub_signed(Duration::days(i64::from(delta))) else {
            continue;
        };

        // Skip absent days — reconciliation upgrades existing entries, never
        // fabricates one for a day the journal never wrote.
        let Some(entry) = get_entry_by_date(mem, date)? else {
            continue;
        };
        report.days_examined += 1;

        // Honest degradation: a `gh` blip for this day is logged and skipped,
        // never fails the whole pass and never erases the frozen entry.
        let merged_prs = match merged.merged_prs_for_date(date) {
            Ok(rows) => rows,
            Err(e) => {
                report.days_degraded += 1;
                tracing::warn!(
                    target: "simard::journal",
                    error = %e,
                    date = %date,
                    "journal reconciliation merged-PR fetch failed; leaving the day's frozen entry unchanged"
                );
                continue;
            }
        };

        if let Some(updated) = reconcile_entry(&entry, &merged_prs, now) {
            save_entry(mem, &updated)?;
            report.days_updated += 1;
            tracing::info!(
                target: "simard::journal",
                date = %date,
                merged = updated.merged_pr_count(),
                "journal reconciliation folded the day's real merges into its frozen entry"
            );
        }
    }

    Ok(report)
}
