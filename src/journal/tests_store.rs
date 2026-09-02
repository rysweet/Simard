//! Tests: entries persist in cognitive memory, are retrievable by exact date,
//! roll forward idempotently, and are queryable by date range and free text —
//! surviving a "restart" (a fresh store over the same backend) (issue #2606).

use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};

use super::test_support::{FakeMemory, pr};
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardError;
use crate::journal::store::{JOURNAL_TAG, JournalStore, journal_caller_key};
use crate::journal::types::{JournalEntry, PrSummary};

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

fn entry(date: NaiveDate, narrative: &str, prs: Vec<PrSummary>) -> JournalEntry {
    JournalEntry {
        date,
        generated_at: Utc::now(),
        narrative: narrative.to_string(),
        draft: narrative.to_string(),
        prs,
        quiet_day: false,
    }
}

fn fresh_store() -> (Arc<dyn CognitiveMemoryOps>, JournalStore) {
    let mem: Arc<dyn CognitiveMemoryOps> = Arc::new(FakeMemory::new());
    let store = JournalStore::new(Arc::clone(&mem));
    (mem, store)
}

#[test]
fn save_then_get_by_date_round_trips() {
    let (_mem, store) = fresh_store();
    let e = entry(
        ymd(2026, 7, 5),
        "a good day",
        vec![pr(1, "a fix", "merged")],
    );
    store.save(&e).expect("save");

    let got = store
        .get_by_date(ymd(2026, 7, 5))
        .expect("get")
        .expect("entry present");
    assert_eq!(got.date, e.date);
    assert_eq!(got.narrative, e.narrative);
    assert_eq!(got.prs, e.prs);
}

#[test]
fn missing_date_returns_none() {
    let (_mem, store) = fresh_store();
    assert!(store.get_by_date(ymd(2026, 1, 1)).expect("get").is_none());
}

#[test]
fn regeneration_is_idempotent_rolling_update() {
    let (_mem, store) = fresh_store();
    let d = ymd(2026, 7, 5);
    store.save(&entry(d, "draft one", vec![])).expect("save v1");
    store.save(&entry(d, "draft two", vec![])).expect("save v2");

    // Latest wins; no duplicate day.
    let got = store.get_by_date(d).expect("get").expect("present");
    assert_eq!(got.narrative, "draft two");
    assert_eq!(store.dates().expect("dates").len(), 1);
}

#[test]
fn query_filters_by_date_range_newest_first() {
    let (_mem, store) = fresh_store();
    let d1 = ymd(2026, 7, 3);
    let d2 = ymd(2026, 7, 4);
    let d3 = ymd(2026, 7, 5);
    store.save(&entry(d1, "first", vec![])).expect("save");
    store.save(&entry(d3, "third", vec![])).expect("save");
    store.save(&entry(d2, "second", vec![])).expect("save");

    let all = store.query(None, None).expect("query all");
    let dates: Vec<NaiveDate> = all.iter().map(|e| e.date).collect();
    assert_eq!(dates, vec![d3, d2, d1], "newest day first");

    let ranged = store.query(Some((d1, d2)), None).expect("query range");
    let ranged_dates: Vec<NaiveDate> = ranged.iter().map(|e| e.date).collect();
    assert_eq!(ranged_dates, vec![d2, d1]);
}

#[test]
fn query_matches_free_text_in_narrative_and_prs() {
    let (_mem, store) = fresh_store();
    store
        .save(&entry(
            ymd(2026, 7, 5),
            "the dashboard got much faster today",
            vec![pr(99, "fixed a login crash", "merged")],
        ))
        .expect("save");
    store
        .save(&entry(ymd(2026, 7, 6), "a quiet day", vec![]))
        .expect("save");

    // Matches narrative text.
    let by_narrative = store.query(None, Some("dashboard")).expect("query");
    assert_eq!(by_narrative.len(), 1);
    assert_eq!(by_narrative[0].date, ymd(2026, 7, 5));

    // Matches PR summary text.
    let by_pr = store.query(None, Some("login")).expect("query");
    assert_eq!(by_pr.len(), 1);
    assert_eq!(by_pr[0].date, ymd(2026, 7, 5));

    // Case-insensitive.
    let case = store.query(None, Some("QUIET")).expect("query");
    assert_eq!(case.len(), 1);
    assert_eq!(case[0].date, ymd(2026, 7, 6));
}

#[test]
fn entries_survive_a_restart() {
    let (mem, store) = fresh_store();
    store
        .save(&entry(ymd(2026, 7, 5), "persisted", vec![]))
        .expect("save");

    // A brand-new store over the same (persistent) backend still sees it.
    let restarted = JournalStore::new(Arc::clone(&mem));
    let got = restarted
        .get_by_date(ymd(2026, 7, 5))
        .expect("get")
        .expect("entry survives restart");
    assert_eq!(got.narrative, "persisted");
}

#[test]
fn corrupt_journal_record_fails_loud() {
    let (mem, store) = fresh_store();
    let d = ymd(2026, 7, 5);
    let key = journal_caller_key(d);
    // Write a journal-keyed fact whose content is not a valid entry.
    mem.store_fact_with_caller_key(&key, &key, "not-json", 1.0, &[JOURNAL_TAG.to_string()], "x")
        .expect("write corrupt fact");

    let err = store.get_by_date(d).expect_err("must fail loud");
    assert!(
        matches!(err, SimardError::InvalidJournalRecord { .. }),
        "corrupt record must surface as InvalidJournalRecord, got: {err:?}"
    );

    // Enumeration stays lenient — a corrupt record is simply skipped.
    let all = store.query(None, None).expect("query tolerates corruption");
    assert!(all.is_empty());
}

/// Build an entry for `date` stamped with an explicit generation time and a
/// specific PR count, so a test can distinguish two same-day duplicates.
fn entry_at(date: NaiveDate, generated_at: DateTime<Utc>, pr_count: usize) -> JournalEntry {
    JournalEntry {
        date,
        generated_at,
        narrative: format!("generated at {generated_at}"),
        draft: String::new(),
        prs: (0..pr_count)
            .map(|i| pr(i as u64, "a change", "open"))
            .collect(),
        quiet_day: false,
    }
}

/// Inject a journal fact directly (bypassing caller-key dedup) so a test can
/// forge the *duplicate-day* corruption the live store exhibited: two distinct
/// `journal:YYYY-MM-DD` facts for the same day.
fn inject_duplicate_fact(mem: &Arc<dyn CognitiveMemoryOps>, entry: &JournalEntry) {
    let key = journal_caller_key(entry.date);
    let content = serde_json::to_string(entry).expect("serialize entry");
    // `store_fact` appends without caller-key replacement, so repeated calls
    // for the same concept accumulate — exactly the state the read layer must
    // now defend against.
    mem.store_fact(
        &key,
        &content,
        1.0,
        &[JOURNAL_TAG.to_string()],
        "journal-generator",
    )
    .expect("inject duplicate fact");
}

#[test]
fn duplicate_day_facts_collapse_to_newest_in_dates_and_search() {
    let (mem, store) = fresh_store();
    let d = ymd(2026, 7, 15);
    let older = entry_at(d, "2026-07-15T21:20:47Z".parse().expect("ts"), 31);
    let newer = entry_at(d, "2026-07-15T22:04:00Z".parse().expect("ts"), 34);
    // Insert out of generation order to prove selection is by timestamp, not
    // enumeration order.
    inject_duplicate_fact(&mem, &newer);
    inject_duplicate_fact(&mem, &older);

    // Date picker: the day appears exactly ONCE (no duplicate), with the
    // newest generation's PR count.
    let dates = store.dates().expect("dates");
    assert_eq!(
        dates,
        vec![d],
        "duplicate day must collapse to a single date"
    );
    let all = store.query(None, None).expect("query all");
    assert_eq!(all.len(), 1, "search must not list the day twice");
    assert_eq!(all[0].prs.len(), 34, "newest generation wins");

    // Single-entry read agrees with the deduped list (newest wins), not the
    // arbitrary first-enumerated duplicate.
    let got = store.get_by_date(d).expect("get").expect("present");
    assert_eq!(
        got.prs.len(),
        34,
        "get_by_date returns the newest generation"
    );
}

#[test]
fn duplicate_facts_across_multiple_days_each_collapse_independently() {
    let (mem, store) = fresh_store();
    let d1 = ymd(2026, 7, 14);
    let d2 = ymd(2026, 7, 15);
    inject_duplicate_fact(
        &mem,
        &entry_at(d1, "2026-07-14T10:00:00Z".parse().unwrap(), 2),
    );
    inject_duplicate_fact(
        &mem,
        &entry_at(d1, "2026-07-14T12:00:00Z".parse().unwrap(), 5),
    );
    inject_duplicate_fact(
        &mem,
        &entry_at(d2, "2026-07-15T09:00:00Z".parse().unwrap(), 1),
    );
    inject_duplicate_fact(
        &mem,
        &entry_at(d2, "2026-07-15T11:00:00Z".parse().unwrap(), 7),
    );

    let all = store.query(None, None).expect("query all");
    let dates: Vec<NaiveDate> = all.iter().map(|e| e.date).collect();
    assert_eq!(dates, vec![d2, d1], "one entry per day, newest day first");
    assert_eq!(all[0].prs.len(), 7, "d2 newest generation wins");
    assert_eq!(all[1].prs.len(), 5, "d1 newest generation wins");

    // Range filtering still applies on top of the dedup.
    let ranged = store.query(Some((d1, d1)), None).expect("query range");
    assert_eq!(ranged.len(), 1);
    assert_eq!(ranged[0].date, d1);
    assert_eq!(ranged[0].prs.len(), 5);
}
