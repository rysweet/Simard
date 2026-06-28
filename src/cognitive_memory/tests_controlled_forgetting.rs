//! TDD (RED) tests for PR-3 (issue #2434): controlled forgetting of *live*
//! facts (flip the no-op `RetentionPolicy` knobs).
//!
//! Written **before** the production change; FAILS until PR-3 lands. The
//! retention machinery already exists (`prune_superseded` /
//! `prune_semantic_memory` with `RetentionPolicy`), but its policy knobs are
//! no-ops over live facts. #2434 adds a bounded, safe hygiene pass. The
//! symbols these tests pin (to be added by the implementation):
//!
//!   * `CognitiveMemoryOps::forget_low_value_facts(dry_run: bool)
//!      -> SimardResult<ForgetReport>` — a hygiene pass invoking
//!     `prune_semantic_memory` with a REAL `min_importance_to_keep`
//!     (`FORGET_MIN_IMPORTANCE`) over LIVE facts. Default impl is a safe no-op
//!     (`Ok(ForgetReport::default())`); only `LibraryCognitiveMemory` forgets.
//!     Mandatory safety (A5): dry-run first, never include a provenance-bearing
//!     or above-threshold fact in the delete set, and only run live when
//!     candidates > 0.
//!   * `pub struct ForgetReport { dry_run, live_before, live_after, candidates,
//!      archived, deleted }` (+ `Default`) in `crate::cognitive_memory`.
//!   * `pub const FORGET_MIN_IMPORTANCE: f64 = 0.1` in `crate::cognitive_memory`.
//!
//! It must also be wired into the consolidation cadence
//! (`memory_consolidation::consolidation_persistence`, alongside
//! `prune_superseded`) — pinned by `consolidation_persistence_runs_controlled_forgetting`.

use super::{CognitiveMemoryOps, FORGET_MIN_IMPORTANCE, ForgetReport, LibraryCognitiveMemory};
use crate::memory_consolidation::consolidation_persistence;
use crate::session::SessionId;

fn test_mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory DB should create")
}

fn live_concepts(mem: &LibraryCognitiveMemory) -> Vec<String> {
    mem.search_facts("*", 256, 0.0)
        .expect("search all facts")
        .into_iter()
        .map(|f| f.concept)
        .collect()
}

/// The configured threshold is conservative but non-zero: > 0 so it actually
/// forgets, yet small enough to catch only genuinely low-value facts.
#[test]
fn forget_min_importance_is_conservative_and_nonzero() {
    // Bound through a local so clippy doesn't const-fold the comparisons into
    // `assertions_on_constants` — the test asserts the published value, not a
    // literal, and must keep firing if the constant is ever retuned.
    let threshold = FORGET_MIN_IMPORTANCE;
    assert!(
        threshold > 0.0,
        "a zero threshold never forgets a live fact (the pre-#2434 no-op)"
    );
    assert!(threshold <= 0.2, "threshold must stay conservative");
    assert!((threshold - 0.1).abs() < 1e-9);
}

/// Core behavior: a genuinely low-value live fact is forgotten; a high-value one
/// survives a fixed recall query before/after.
#[test]
fn forget_low_value_facts_drops_low_keeps_high() {
    let mem = test_mem();
    mem.store_fact("kafka", "important durable kafka fact", 0.95, &[], "src")
        .expect("store high");
    mem.store_fact("scratch", "throwaway low value note", 0.05, &[], "src")
        .expect("store low");

    let before = live_concepts(&mem);
    assert!(before.contains(&"kafka".to_string()));
    assert!(before.contains(&"scratch".to_string()));

    let report = mem
        .forget_low_value_facts(false)
        .expect("controlled forgetting");
    assert!(!report.dry_run, "live run reported as non-dry");
    assert!(
        report.deleted + report.archived >= 1,
        "at least one low-value fact must be forgotten"
    );

    let after = live_concepts(&mem);
    assert!(
        after.contains(&"kafka".to_string()),
        "high-value fact must survive controlled forgetting"
    );
    assert!(
        !after.contains(&"scratch".to_string()),
        "low-value fact must be forgotten"
    );
    assert!(after.len() < before.len(), "live fact count must shrink");
}

/// Safety (A5): a provenance-bearing fact is protected even when its confidence
/// is below the forgetting threshold; an equally low-confidence fact WITHOUT
/// provenance is forgotten.
#[test]
fn forget_low_value_facts_protects_provenance_bearing() {
    let mem = test_mem();
    let ep = mem
        .store_episode(
            "observed a flaky retry in the deploy step",
            "engineer",
            None,
        )
        .expect("store episode");

    // Low-confidence BUT provenance-bearing → protected.
    mem.store_fact_with_provenance(
        "bug-pattern",
        "flaky retry observed in deploy",
        0.05,
        "src",
        None,
        None,
        &[ep],
    )
    .expect("store provenance fact");
    // Low-confidence, no provenance → forgettable.
    mem.store_fact("noise", "ephemeral low value note", 0.05, &[], "src")
        .expect("store noise");

    let report = mem
        .forget_low_value_facts(false)
        .expect("controlled forgetting");
    assert!(
        report.deleted + report.archived >= 1,
        "the unprotected low-value fact must be forgotten"
    );

    let after = live_concepts(&mem);
    assert!(
        after.contains(&"bug-pattern".to_string()),
        "provenance-bearing fact must NEVER be in the delete set"
    );
    assert!(
        !after.contains(&"noise".to_string()),
        "unprotected low-value fact must be forgotten"
    );
}

/// `dry_run: true` reports candidates but changes nothing (the mandatory
/// preview before any live deletion).
#[test]
fn forget_low_value_facts_dry_run_is_a_pure_preview() {
    let mem = test_mem();
    mem.store_fact("scratch", "throwaway low value note", 0.05, &[], "src")
        .expect("store low");
    mem.store_fact("kafka", "durable", 0.95, &[], "src")
        .expect("store high");

    let before = live_concepts(&mem);
    let report: ForgetReport = mem
        .forget_low_value_facts(true)
        .expect("dry-run forgetting");

    assert!(report.dry_run, "dry-run must be reported as such");
    assert!(
        report.candidates >= 1,
        "dry-run must report the low-value candidate"
    );
    assert_eq!(report.deleted, 0, "dry-run must delete nothing");
    assert_eq!(report.archived, 0, "dry-run must archive nothing");

    let after = live_concepts(&mem);
    assert_eq!(
        before.len(),
        after.len(),
        "dry-run must not change the store"
    );
    assert!(after.contains(&"scratch".to_string()));
}

/// Safe no-op: when every live fact is above the threshold there are no
/// candidates, so nothing is deleted (`archived + deleted == 0`).
#[test]
fn forget_low_value_facts_is_a_safe_noop_when_no_candidates() {
    let mem = test_mem();
    mem.store_fact("a", "durable high-value fact", 0.95, &[], "src")
        .expect("store a");
    mem.store_fact("b", "another durable fact", 0.9, &[], "src")
        .expect("store b");

    let report = mem
        .forget_low_value_facts(false)
        .expect("controlled forgetting");
    assert_eq!(report.candidates, 0, "no low-value candidates exist");
    assert_eq!(
        report.deleted + report.archived,
        0,
        "safe no-op when nothing qualifies for forgetting"
    );
    assert_eq!(
        live_concepts(&mem).len(),
        2,
        "all high-value facts retained"
    );
}

/// Wiring (PR-3 item 3): the consolidation cadence runs controlled forgetting
/// alongside `prune_superseded`, so low-value live facts fade during normal
/// consolidation while high-value ones persist.
#[test]
fn consolidation_persistence_runs_controlled_forgetting() {
    let mem = test_mem();
    mem.store_fact("durable", "high value durable fact", 0.95, &[], "src")
        .expect("store high");
    mem.store_fact("scratch", "throwaway low value note", 0.05, &[], "src")
        .expect("store low");

    let session =
        SessionId::parse("session-01234567-89ab-cdef-0123-456789abcdef").expect("valid session id");
    consolidation_persistence(&session, &mem).expect("consolidation persistence");

    let after = live_concepts(&mem);
    assert!(
        after.contains(&"durable".to_string()),
        "high-value fact must survive the consolidation cadence"
    );
    assert!(
        !after.contains(&"scratch".to_string()),
        "controlled forgetting must be wired into the consolidation cadence"
    );
}
