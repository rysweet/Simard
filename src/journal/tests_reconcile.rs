//! Tests: past-day merged-PR reconciliation (issue #4225).
//!
//! The journal freezes a day's entry once the day passes, so a day that shipped
//! PRs after its final tick — or that froze before the #4140 merged-PR wiring
//! landed — reports `merged: 0` forever. These tests pin the reconciliation
//! pass that folds each recent past day's *real* merges into its frozen entry:
//! it upgrades open rows to `merged`, appends unseen merges, stays idempotent,
//! never downgrades or erases, never touches today, skips absent days, and
//! degrades honestly on a `gh` blip. No network — every source is a fake.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};

use super::test_support::{FakeMemory, pr};
use crate::error::{SimardError, SimardResult};
use crate::journal::reconcile::{
    DEFAULT_RECONCILE_LOOKBACK_DAYS, MAX_RECONCILE_LOOKBACK_DAYS, MergedPrSource, reconcile_entry,
    reconcile_lookback_days_from, reconcile_recent_days,
};
use crate::journal::store::{get_entry_by_date, save_entry};
use crate::journal::types::{JournalEntry, PrSummary};

/// A fixed "reconciliation instant" newer than any frozen entry's `generated_at`.
fn recon_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 17, 12, 0, 0)
        .single()
        .expect("valid ts")
}

/// A frozen entry's (older) generation time.
fn frozen_at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 10, 23, 30, 0)
        .single()
        .expect("valid ts")
}

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).expect("valid date")
}

/// Build a frozen [`JournalEntry`] for `date` with the given PR rows.
fn entry(date: NaiveDate, prs: Vec<PrSummary>, quiet: bool) -> JournalEntry {
    JournalEntry {
        date,
        generated_at: frozen_at(),
        narrative: format!("## {date}\n\nA day happened."),
        draft: "draft".to_string(),
        prs,
        quiet_day: quiet,
    }
}

/// A merged row exactly as the merged-only seam yields it.
fn merged_row(number: u64, summary: &str) -> PrSummary {
    pr(number, summary, "merged")
}

/// A canned merged-only source keyed by date, with a per-date failure switch.
#[derive(Default)]
struct MapMergedSource {
    by_date: HashMap<NaiveDate, Vec<PrSummary>>,
    fail_on: HashMap<NaiveDate, bool>,
}

impl MapMergedSource {
    fn merges(mut self, date: NaiveDate, rows: Vec<PrSummary>) -> Self {
        self.by_date.insert(date, rows);
        self
    }
    fn failing(mut self, date: NaiveDate) -> Self {
        self.fail_on.insert(date, true);
        self
    }
}

impl MergedPrSource for MapMergedSource {
    fn merged_prs_for_date(&self, date: NaiveDate) -> SimardResult<Vec<PrSummary>> {
        if self.fail_on.get(&date).copied().unwrap_or(false) {
            return Err(SimardError::MergeAuthorityGhCommandFailed {
                reason: "gh merged list exploded".into(),
            });
        }
        Ok(self.by_date.get(&date).cloned().unwrap_or_default())
    }
}

// ── Pure `reconcile_entry` ──────────────────────────────────────────────────

#[test]
fn upgrades_a_still_open_row_to_merged() {
    let day = d(2026, 7, 15);
    let e = entry(
        day,
        vec![pr(
            10,
            "add sign-in",
            "still open — automated checks still running",
        )],
        false,
    );
    let updated =
        reconcile_entry(&e, &[merged_row(10, "add sign-in")], recon_now()).expect("changed");

    assert_eq!(updated.prs.len(), 1, "no duplicate row is created");
    assert_eq!(updated.prs[0].outcome, "merged");
    assert_eq!(
        updated.prs[0].plain_summary, "add sign-in",
        "the frozen summary is preserved"
    );
    assert_eq!(updated.merged_pr_count(), 1);
    assert!(
        updated.generated_at > e.generated_at,
        "generation time advances"
    );
}

#[test]
fn appends_a_merged_pr_the_frozen_entry_never_saw() {
    let day = d(2026, 7, 15);
    let e = entry(
        day,
        vec![pr(10, "add sign-in", "still open — not ready yet")],
        false,
    );
    let updated =
        reconcile_entry(&e, &[merged_row(20, "fix a crash")], recon_now()).expect("changed");

    assert_eq!(updated.prs.len(), 2, "the unseen merge is appended");
    // The pre-existing open row is left exactly as it was.
    let open = updated.prs.iter().find(|p| p.number == 10).expect("row 10");
    assert_eq!(open.outcome, "still open — not ready yet");
    let added = updated.prs.iter().find(|p| p.number == 20).expect("row 20");
    assert_eq!(added.outcome, "merged");
    assert_eq!(updated.merged_pr_count(), 1);
}

#[test]
fn is_idempotent_when_the_entry_already_reflects_the_merge() {
    let day = d(2026, 7, 15);
    let e = entry(day, vec![pr(10, "add sign-in", "merged")], false);
    assert!(
        reconcile_entry(&e, &[merged_row(10, "add sign-in")], recon_now()).is_none(),
        "an already-merged row is a no-op"
    );
}

#[test]
fn empty_merge_list_is_a_no_op() {
    let day = d(2026, 7, 15);
    let e = entry(
        day,
        vec![pr(10, "add sign-in", "still open — not ready yet")],
        false,
    );
    assert!(reconcile_entry(&e, &[], recon_now()).is_none());
}

#[test]
fn never_downgrades_or_erases_existing_rows() {
    let day = d(2026, 7, 15);
    let e = entry(
        day,
        vec![
            pr(10, "already landed", "merged"),
            pr(
                11,
                "still cooking",
                "still open — automated checks still running",
            ),
        ],
        false,
    );
    // A different day's merge arrives; the two existing rows must survive intact.
    let updated =
        reconcile_entry(&e, &[merged_row(12, "another change")], recon_now()).expect("changed");

    assert_eq!(updated.prs.len(), 3);
    let landed = updated.prs.iter().find(|p| p.number == 10).expect("row 10");
    assert_eq!(landed.outcome, "merged", "merged row is never downgraded");
    let cooking = updated.prs.iter().find(|p| p.number == 11).expect("row 11");
    assert_eq!(
        cooking.outcome, "still open — automated checks still running",
        "open row untouched by an unrelated merge"
    );
    assert_eq!(updated.merged_pr_count(), 2);
}

#[test]
fn folding_a_merge_flips_a_quiet_day_to_not_quiet() {
    let day = d(2026, 7, 15);
    let e = entry(day, vec![], true);
    let updated =
        reconcile_entry(&e, &[merged_row(5, "a shipped change")], recon_now()).expect("changed");

    assert!(!updated.quiet_day, "a day that shipped code is not quiet");
    assert_eq!(updated.merged_pr_count(), 1);
}

#[test]
fn a_non_merged_row_in_the_input_is_ignored() {
    // Defense-in-depth: the seam is merged-only, but a stray open row must not
    // be folded in even if one somehow appears.
    let day = d(2026, 7, 15);
    let e = entry(day, vec![], true);
    let stray = pr(9, "not landed", "still open — not ready yet");
    assert!(
        reconcile_entry(&e, &[stray], recon_now()).is_none(),
        "a non-merged input row changes nothing"
    );
}

#[test]
fn does_not_double_count_when_a_merged_row_for_the_pr_already_exists() {
    // A single live tick can persist `[open #N, merged #N]`: the production
    // source appends OPEN rows before MERGED rows, so a PR that merges between
    // those two fetches lands as both. Reconciliation must NOT upgrade the open
    // row (that would count the PR as merged twice) — the merge is already
    // reflected, so the fold is a no-op.
    let day = d(2026, 7, 15);
    let e = entry(
        day,
        vec![
            pr(
                10,
                "add sign-in",
                "still open — automated checks still running",
            ),
            pr(10, "add sign-in", "merged"),
        ],
        false,
    );
    assert_eq!(
        e.merged_pr_count(),
        1,
        "precondition: exactly one merged row"
    );
    assert!(
        reconcile_entry(&e, &[merged_row(10, "add sign-in")], recon_now()).is_none(),
        "an already-merged PR is never double-counted"
    );
}

// ── `reconcile_lookback_days_from` parsing (pure; no env mutation) ───────────

#[test]
fn lookback_days_defaults_and_clamps() {
    // Pure parse/clamp — no process-environment mutation (unsound under a
    // parallel test binary). This exercises the exact logic
    // `reconcile_lookback_days()` applies to the env value.
    assert_eq!(
        reconcile_lookback_days_from(None),
        DEFAULT_RECONCILE_LOOKBACK_DAYS,
        "unset falls back to the default"
    );
    assert_eq!(reconcile_lookback_days_from(Some("3")), 3);
    assert_eq!(
        reconcile_lookback_days_from(Some("  5 ")),
        5,
        "surrounding whitespace is trimmed"
    );
    assert_eq!(
        reconcile_lookback_days_from(Some("0")),
        0,
        "0 disables reconciliation"
    );
    assert_eq!(
        reconcile_lookback_days_from(Some("999")),
        MAX_RECONCILE_LOOKBACK_DAYS,
        "clamped to the ceiling"
    );
    assert_eq!(
        reconcile_lookback_days_from(Some("not-a-number")),
        DEFAULT_RECONCILE_LOOKBACK_DAYS,
        "garbage falls back to the default"
    );
}

// ── Driver `reconcile_recent_days` ──────────────────────────────────────────

#[test]
fn backfills_a_frozen_past_day_and_persists_the_real_count() {
    let mem = FakeMemory::new();
    let today = d(2026, 7, 17);
    let yesterday = d(2026, 7, 16);

    // A frozen past-day entry that shipped a PR but froze at merged: 0.
    save_entry(
        &mem,
        &entry(
            yesterday,
            vec![pr(10, "add sign-in", "still open — not ready yet")],
            false,
        ),
    )
    .expect("seed frozen entry");

    let src = MapMergedSource::default().merges(yesterday, vec![merged_row(10, "add sign-in")]);
    let report = reconcile_recent_days(&mem, &src, today, 7).expect("pass runs");

    assert_eq!(report.days_examined, 1);
    assert_eq!(report.days_updated, 1);
    assert_eq!(report.days_degraded, 0);

    let stored = get_entry_by_date(&mem, yesterday)
        .expect("read")
        .expect("present");
    assert_eq!(
        stored.merged_pr_count(),
        1,
        "the frozen day now reports its real merge"
    );
    assert_eq!(stored.prs[0].outcome, "merged");
}

#[test]
fn never_touches_today() {
    let mem = FakeMemory::new();
    let today = d(2026, 7, 17);

    // Today's entry is frozen at merged: 0; the source *would* return a merge
    // for today, but reconciliation must leave today entirely to the live tick.
    save_entry(
        &mem,
        &entry(
            today,
            vec![pr(99, "todays work", "still open — not ready yet")],
            false,
        ),
    )
    .expect("seed today");
    let src = MapMergedSource::default().merges(today, vec![merged_row(99, "todays work")]);

    let report = reconcile_recent_days(&mem, &src, today, 7).expect("pass runs");
    assert_eq!(
        report.days_examined, 0,
        "today is outside the past-day window"
    );

    let stored = get_entry_by_date(&mem, today)
        .expect("read")
        .expect("present");
    assert_eq!(stored.merged_pr_count(), 0, "today's entry is untouched");
}

#[test]
fn skips_absent_days_without_fabricating_an_entry() {
    let mem = FakeMemory::new();
    let today = d(2026, 7, 17);
    let absent = d(2026, 7, 14); // three days back, no stored entry

    // Even though the source has merges for the absent day, no entry is created.
    let src = MapMergedSource::default().merges(absent, vec![merged_row(30, "orphan merge")]);
    let report = reconcile_recent_days(&mem, &src, today, 7).expect("pass runs");

    assert_eq!(report.days_examined, 0, "an absent day is skipped");
    assert_eq!(report.days_updated, 0);
    assert!(
        get_entry_by_date(&mem, absent).expect("read").is_none(),
        "reconciliation never fabricates an entry for a day the journal never wrote"
    );
}

#[test]
fn degrades_honestly_on_a_gh_blip_and_carries_on() {
    let mem = FakeMemory::new();
    let today = d(2026, 7, 17);
    let day_blip = d(2026, 7, 16);
    let day_ok = d(2026, 7, 15);

    save_entry(
        &mem,
        &entry(
            day_blip,
            vec![pr(10, "blip day", "still open — not ready yet")],
            false,
        ),
    )
    .expect("seed blip day");
    save_entry(
        &mem,
        &entry(
            day_ok,
            vec![pr(20, "good day", "still open — not ready yet")],
            false,
        ),
    )
    .expect("seed ok day");

    let src = MapMergedSource::default()
        .failing(day_blip)
        .merges(day_ok, vec![merged_row(20, "good day")]);

    let report =
        reconcile_recent_days(&mem, &src, today, 7).expect("pass does not error on a blip");

    assert_eq!(report.days_examined, 2);
    assert_eq!(
        report.days_degraded, 1,
        "the blip day is counted as degraded, not fatal"
    );
    assert_eq!(
        report.days_updated, 1,
        "the healthy day is still reconciled"
    );

    // The blip day keeps its frozen (unchanged) entry — never erased.
    let blip = get_entry_by_date(&mem, day_blip)
        .expect("read")
        .expect("present");
    assert_eq!(blip.merged_pr_count(), 0);
    assert_eq!(
        blip.prs.len(),
        1,
        "the blip day's frozen row survives intact"
    );

    let ok = get_entry_by_date(&mem, day_ok)
        .expect("read")
        .expect("present");
    assert_eq!(ok.merged_pr_count(), 1);
}

#[test]
fn a_second_pass_is_idempotent() {
    let mem = FakeMemory::new();
    let today = d(2026, 7, 17);
    let yesterday = d(2026, 7, 16);

    save_entry(
        &mem,
        &entry(
            yesterday,
            vec![pr(10, "add sign-in", "still open — not ready yet")],
            false,
        ),
    )
    .expect("seed");
    let src = MapMergedSource::default().merges(yesterday, vec![merged_row(10, "add sign-in")]);

    let first = reconcile_recent_days(&mem, &src, today, 7).expect("first pass");
    assert_eq!(first.days_updated, 1);

    // Running it again changes nothing: the entry already reflects the merge.
    let second = reconcile_recent_days(&mem, &src, today, 7).expect("second pass");
    assert_eq!(second.days_examined, 1);
    assert_eq!(second.days_updated, 0, "the second pass is a no-op");
}

#[test]
fn zero_lookback_disables_the_pass() {
    let mem = FakeMemory::new();
    let today = d(2026, 7, 17);
    let yesterday = d(2026, 7, 16);
    save_entry(
        &mem,
        &entry(
            yesterday,
            vec![pr(10, "x", "still open — not ready yet")],
            false,
        ),
    )
    .expect("seed");
    let src = MapMergedSource::default().merges(yesterday, vec![merged_row(10, "x")]);

    let report = reconcile_recent_days(&mem, &src, today, 0).expect("no-op pass");
    assert_eq!(
        report,
        crate::journal::ReconcileReport::default(),
        "0 lookback does nothing"
    );
}
