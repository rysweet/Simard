//! End-to-end guard tests for the episodic->procedural skill-reuse loop
//! (issue #2441), exercised through the Simard `CognitiveMemoryOps` surface
//! against `LibraryCognitiveMemory::in_memory()` (the sole real backend).
//!
//! The reuse loop has three already-wired halves:
//!
//!   1. **distill**  — a recurring/successful episode pattern is stored as a
//!      procedure (`store_procedure*`; see `memory_consolidation::distillation`).
//!   2. **recall**   — `recall_procedure(query, limit)` returns matching
//!      procedures ordered by `usage_count` **descending** (the library's
//!      `search_procedures` sorts reused procedures first), so reuse influences
//!      what is surfaced.
//!   3. **reinforce**— `reinforce_access(id, MemoryKind::Procedure)` bumps a
//!      recalled procedure's persisted `usage_count` at apply time
//!      (`memory_consolidation::reinforce_prepared_context`).
//!
//! Existing tests cover each half *in isolation* (`tests_ranked_episodic` proves
//! the `usage_count` increment; the library proves the usage sort). These tests
//! close that coverage gap by asserting the halves COMPOSE end-to-end through the
//! Simard adapter: reinforcing a recalled procedure must change a *later*
//! recall's ORDER — i.e. reuse feeds back into recall ("close the loop, don't
//! just store"). They are regression guards — green while the loop is closed, red
//! if a refactor (an unranked search, a dropped reinforcement, or a library bump
//! that drops the usage sort) silently re-opens it.
//!
//! The genuinely *new* #2441/#2458 behavior — gating distillation on a verified
//! signal (#2441) and promoting recurring failures to lessons (#2458) — lives in
//! [`crate::memory_consolidation::reflection_lessons`] (with memory-backed
//! acceptance tests alongside it).

use super::{CognitiveMemoryOps, LibraryCognitiveMemory, MemoryKind};

fn test_mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory DB should create")
}

fn steps() -> Vec<String> {
    vec!["step one".to_string(), "step two".to_string()]
}

fn prereqs() -> Vec<String> {
    Vec::new()
}

/// Find a recalled procedure's raw node id by name (the id `reinforce_access`
/// consumes for `MemoryKind::Procedure`).
fn node_id_for<'a>(
    recalled: &'a [crate::memory_cognitive::CognitiveProcedure],
    name: &str,
) -> &'a str {
    recalled
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("procedure {name:?} must be present in recall"))
        .node_id
        .as_str()
}

/// Rank-by-reuse: two procedures that match the query `"merge"` with equal token
/// overlap differ ONLY in `usage_count`; the more-reused one must surface first
/// through the Simard adapter's `recall_procedure`. Guards that reuse ordering is
/// observable end-to-end (not just that `usage_count` increments).
#[test]
fn recall_procedure_ranks_more_used_procedure_first() {
    let mem = test_mem();
    mem.store_procedure("merge conflict resolver", &steps(), &prereqs())
        .expect("store a");
    mem.store_procedure("merge branch updater", &steps(), &prereqs())
        .expect("store b");

    // Reuse "merge branch updater" three times; the other stays at usage_count 0.
    let initial = mem.recall_procedure("merge", 10).expect("recall");
    let reused_id = node_id_for(&initial, "merge branch updater").to_string();
    for _ in 0..3 {
        mem.reinforce_access(&reused_id, MemoryKind::Procedure)
            .expect("reinforce reused procedure");
    }

    let ranked = mem
        .recall_procedure("merge", 10)
        .expect("recall after reuse");
    let names: Vec<&str> = ranked.iter().map(|p| p.name.as_str()).collect();
    assert!(
        names.contains(&"merge branch updater") && names.contains(&"merge conflict resolver"),
        "both equally-matching procedures must be recalled, got {names:?}"
    );
    assert_eq!(
        ranked[0].name, "merge branch updater",
        "the more-reused procedure must rank first; got {names:?}"
    );
}

/// Closed-loop end-to-end (#2441): applying (reusing) the currently lowest-ranked
/// matching procedure enough times must lift it to the TOP of a later recall —
/// proving reuse feeds back into recall ordering through the Simard adapter.
#[test]
fn reuse_feeds_back_into_recall_order_closes_the_loop() {
    let mem = test_mem();
    mem.store_procedure("merge conflict resolver", &steps(), &prereqs())
        .expect("store a");
    mem.store_procedure("merge branch updater", &steps(), &prereqs())
        .expect("store b");

    let first = mem.recall_procedure("merge", 10).expect("recall");
    assert!(
        first.len() >= 2,
        "both procedures recalled, got {}",
        first.len()
    );

    // "Apply" whichever procedure is currently ranked LAST, repeatedly.
    let underdog_name = first.last().expect("non-empty").name.clone();
    let underdog_id = first.last().expect("non-empty").node_id.clone();
    for _ in 0..5 {
        mem.reinforce_access(&underdog_id, MemoryKind::Procedure)
            .expect("reinforce applied procedure");
    }

    let second = mem.recall_procedure("merge", 10).expect("recall again");
    assert_eq!(
        second[0].name, underdog_name,
        "a heavily-reused procedure must climb to the top of recall — reuse must \
         feed back into ranking, closing the episodic->procedural loop"
    );
}

/// A freshly distilled procedure (`usage_count == 0`) must remain recallable for
/// its FIRST reuse: a usage-ordered recall must not drop zero-usage matches (they
/// simply sort after reused ones). Guards that a future scoring change can't make
/// brand-new procedures unrecallable and re-open the loop on first use.
#[test]
fn recall_surfaces_zero_usage_procedure_smoothing_guard() {
    let mem = test_mem();
    mem.store_procedure("bootstrap fresh skill", &steps(), &prereqs())
        .expect("store");

    let recalled = mem.recall_procedure("bootstrap", 10).expect("recall");
    assert!(
        recalled
            .iter()
            .any(|p| p.name == "bootstrap fresh skill" && p.usage_count == 0),
        "a usage_count==0 procedure must remain recallable for its first reuse; got {:?}",
        recalled
            .iter()
            .map(|p| (&p.name, p.usage_count))
            .collect::<Vec<_>>()
    );
}
