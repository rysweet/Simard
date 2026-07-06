//! Tests: the background journal thread's rolling tick assembles today's entry
//! from episodic memory, scrubs jargon, persists it under the day key, and
//! degrades honestly to a quiet day when the store has no episodes (issue
//! #2606). No network, no wall clock — a [`FakeMemory`] backs the store and a
//! [`FixedClock`] fixes the day.

use std::sync::Arc;

use super::test_support::{FakeMemory, FixedClock, day, episode};
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::journal::store::get_entry_by_date;
use crate::journal::thread::{journal_enabled, journal_interval_secs, run_journal_tick};

#[test]
fn tick_generates_and_stores_todays_entry_from_episodics() {
    let mem = FakeMemory::new();
    mem.add_episode(episode(
        "I helped an engineer land a fix for the login page.",
    ));
    mem.add_episode(episode("The Overseer reviewed today's proposals."));
    let clock = FixedClock(day());

    let entry = run_journal_tick(&mem, &clock).expect("tick");

    // Built from the episodics (the primary source).
    assert_eq!(entry.date, day());
    assert!(!entry.quiet_day, "a day with episodes is not quiet");
    assert!(
        entry.narrative.contains("login page"),
        "narrative is built from episodic content: {}",
        entry.narrative
    );
    // The reviewed narrative is jargon-scrubbed relative to the raw draft: the
    // drafter's insider phrase "episodic memories" is explained for a layperson.
    assert!(
        entry.draft.contains("episodic memories"),
        "draft still carries the raw term"
    );
    assert!(
        !entry.narrative.contains("episodic memories"),
        "review pass explained/removed the jargon: {}",
        entry.narrative
    );

    // Persisted under the day key and retrievable (survives a fresh handle).
    let mem_dyn: &dyn CognitiveMemoryOps = &mem;
    let got = get_entry_by_date(mem_dyn, day())
        .expect("get")
        .expect("entry stored");
    assert_eq!(got.narrative, entry.narrative);
}

#[test]
fn tick_rolls_forward_idempotently() {
    let mem = FakeMemory::new();
    mem.add_episode(episode("first moment of the day"));
    let clock = FixedClock(day());

    run_journal_tick(&mem, &clock).expect("tick 1");
    // Later in the day a new moment lands; the entry regenerates in place.
    mem.add_episode(episode("a later moment worth remembering"));
    let second = run_journal_tick(&mem, &clock).expect("tick 2");

    let mem_dyn: &dyn CognitiveMemoryOps = &mem;
    let got = get_entry_by_date(mem_dyn, day())
        .expect("get")
        .expect("present");
    assert_eq!(got.narrative, second.narrative, "latest tick wins");
    assert!(
        got.narrative.contains("a later moment"),
        "rolling entry picked up the new moment"
    );
}

#[test]
fn tick_on_empty_store_is_an_honest_quiet_day() {
    let mem = FakeMemory::new();
    let clock = FixedClock(day());

    let entry = run_journal_tick(&mem, &clock).expect("tick");
    assert!(entry.quiet_day, "no episodes and no PRs => quiet day");
    assert!(
        entry.narrative.to_lowercase().contains("quiet"),
        "quiet day is narrated honestly: {}",
        entry.narrative
    );
}

#[test]
fn tick_drops_bare_prepared_context_summary_episodes() {
    // The consolidation path pushes a bare "Prepared context: N facts, ..."
    // summary into memory; it must NOT surface among the journal's remembered
    // moments (issue #2606) — the report presents the substance instead.
    let mem = FakeMemory::new();
    mem.add_episode(episode(
        "Prepared context: 10 facts, 2 triggers, 5 procedures, 5 episodes",
    ));
    mem.add_episode(episode("reviewed the login page fix"));
    let clock = FixedClock(day());

    let entry = run_journal_tick(&mem, &clock).expect("tick");

    assert!(
        !entry.narrative.contains("Prepared context"),
        "the bare prepared-context count line is dropped: {}",
        entry.narrative
    );
    assert!(
        entry.narrative.contains("login page"),
        "real remembered moments are still narrated: {}",
        entry.narrative
    );
}

#[test]
fn enabled_by_default_and_interval_has_a_floor() {
    // Guard the env-independent contract without mutating process env (which
    // would race other tests): defaults hold when the vars are unset.
    if std::env::var(crate::journal::thread::JOURNAL_ENABLED_ENV).is_err() {
        assert!(journal_enabled(), "journal thread is default-on");
    }
    // The interval never drops below the safety floor.
    assert!(journal_interval_secs() >= 60);
}

#[test]
fn arc_backed_store_shares_the_same_entries() {
    // The thread writes through a borrowed handle; a JournalStore over the same
    // backend sees the write (no parallel datastore).
    let mem = Arc::new(FakeMemory::new());
    mem.add_episode(episode("shared-backend moment"));
    let clock = FixedClock(day());
    run_journal_tick(mem.as_ref(), &clock).expect("tick");

    let store = crate::journal::store::JournalStore::new(mem.clone());
    let got = store.get_by_date(day()).expect("get").expect("present");
    assert!(got.narrative.contains("shared-backend moment"));
}
