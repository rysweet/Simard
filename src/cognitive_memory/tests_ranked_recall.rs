//! TDD tests for ranked fact recall + CallerKey snapshot retention/dedup
//! (issue #2329).
//!
//! Pins the three new `CognitiveMemoryOps` methods against the lbug-backed
//! `LibraryCognitiveMemory::in_memory()` adapter (and the trait *default*
//! against a non-library mock):
//!
//! * `recall_facts_ranked(query, limit, min_confidence, weights)` — returns
//!   facts in **descending score order**, excludes superseded revisions, and
//!   defaults to `search_facts` for non-library backends.
//! * `store_fact_with_caller_key(caller_key, …)` — identical content is reused
//!   (no duplicate node), changed content supersedes the prior live fact
//!   (`SUPERSEDES`), leaving exactly one live fact per key.
//! * `prune_superseded()` — reclaims the archived/superseded tail.

use super::{CognitiveFact, CognitiveMemoryOps, LibraryCognitiveMemory, RecallWeightSet};
use crate::error::SimardResult;
use crate::memory_cognitive::{
    CognitiveProcedure, CognitiveProspective, CognitiveStatistics, CognitiveWorkingSlot,
};

fn test_mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory DB should create")
}

/// Count live (non-superseded) facts for a concept by going through ranked
/// recall, which excludes superseded/archived revisions. `query` only affects
/// scoring — every live fact above `min_confidence` is returned regardless of
/// text match — so a broad query surfaces the full live set.
fn live_facts_for_concept(mem: &LibraryCognitiveMemory, concept: &str) -> Vec<CognitiveFact> {
    mem.recall_facts_ranked(concept, 256, 0.0, RecallWeightSet::default())
        .expect("ranked recall")
        .into_iter()
        .filter(|f| f.concept == concept)
        .collect()
}

/// Count *all* physical facts (including archived/superseded) via the wildcard
/// `search_facts` path, which maps to `get_all_facts` and does NOT filter
/// archived revisions.
fn all_physical_facts(mem: &LibraryCognitiveMemory) -> Vec<CognitiveFact> {
    mem.search_facts("*", 256, 0.0).expect("search all facts")
}

// ─── recall_facts_ranked: ordering ──────────────────────────────────────────

/// Invariant #3 (descending recall order): a fact with high text relevance AND
/// high confidence outranks a poorly-matching low-confidence fact.
#[test]
fn recall_facts_ranked_orders_by_descending_score() {
    let mem = test_mem();
    // Strong match + high confidence.
    mem.store_fact("alpha beta", "gamma delta", 0.9, &[], "src")
        .expect("store hi");
    // No token overlap + low confidence.
    mem.store_fact("omega", "sigma", 0.2, &[], "src")
        .expect("store lo");

    let ranked = mem
        .recall_facts_ranked(
            "alpha beta gamma delta",
            10,
            0.0,
            RecallWeightSet::default(),
        )
        .expect("ranked recall");

    assert_eq!(ranked.len(), 2, "both facts returned");
    assert_eq!(
        ranked[0].confidence, 0.9,
        "highest-scored (relevant, confident) fact must come first"
    );
    assert_eq!(
        ranked[1].confidence, 0.2,
        "lowest-scored fact must come last"
    );
}

/// `min_confidence` floors the candidate set, same as `search_facts`.
#[test]
fn recall_facts_ranked_respects_min_confidence() {
    let mem = test_mem();
    mem.store_fact("topic", "high", 0.9, &[], "src")
        .expect("store high");
    mem.store_fact("topic", "low", 0.1, &[], "src")
        .expect("store low");

    let ranked = mem
        .recall_facts_ranked("topic", 10, 0.5, RecallWeightSet::default())
        .expect("ranked recall");

    assert_eq!(ranked.len(), 1, "the 0.1-confidence fact is floored out");
    assert_eq!(ranked[0].confidence, 0.9);
}

/// Invariant #5 (default back-compat): a backend that does NOT override
/// `recall_facts_ranked` returns exactly what `search_facts` returns, for any
/// weights (the default impl ignores them).
#[test]
fn recall_facts_ranked_default_delegates_to_search_facts() {
    let mock = SearchOnlyMock {
        facts: vec![fact("c1", "first", 0.8), fact("c2", "second", 0.5)],
    };

    let via_search = mock.search_facts("q", 10, 0.0).unwrap();
    // Even with non-default (Observe-like) weights, the default impl must
    // delegate verbatim to `search_facts` — same facts, same order.
    let weights = RecallWeightSet {
        recency: 1.0,
        ..RecallWeightSet::default()
    };
    let via_ranked = mock.recall_facts_ranked("q", 10, 0.0, weights).unwrap();

    let search_concepts: Vec<&str> = via_search.iter().map(|f| f.concept.as_str()).collect();
    let ranked_concepts: Vec<&str> = via_ranked.iter().map(|f| f.concept.as_str()).collect();
    assert_eq!(
        ranked_concepts, search_concepts,
        "default recall_facts_ranked must mirror search_facts"
    );
}

// ─── store_fact_with_caller_key: dedup / supersede ──────────────────────────

/// Invariant #1 (single live record): re-storing IDENTICAL content under the
/// same caller key REUSES the existing fact — no duplicate node is created.
#[test]
fn caller_key_identical_content_reuses_no_duplicate() {
    let mem = test_mem();
    let key = "goal-board:snapshot";
    mem.store_fact_with_caller_key(key, "goal-board:snapshot", "BOARD_V1", 1.0, &[], "src")
        .expect("first store");
    mem.store_fact_with_caller_key(key, "goal-board:snapshot", "BOARD_V1", 1.0, &[], "src")
        .expect("identical re-store");

    // Reuse means NO new physical node — exactly one fact exists.
    assert_eq!(
        all_physical_facts(&mem).len(),
        1,
        "identical re-store under the same key must not create a duplicate"
    );
    assert_eq!(live_facts_for_concept(&mem, "goal-board:snapshot").len(), 1);
}

/// Invariants #1 + #2 (supersede integrity): re-storing CHANGED content under
/// the same caller key supersedes the prior fact — one live record remains
/// (the new content), the old one is archived (still physically present until
/// pruned, and excluded from ranked recall).
#[test]
fn caller_key_changed_content_supersedes_prior() {
    let mem = test_mem();
    let key = "goal-board:snapshot";
    mem.store_fact_with_caller_key(key, "goal-board:snapshot", "BOARD_V1", 1.0, &[], "src")
        .expect("store v1");
    mem.store_fact_with_caller_key(key, "goal-board:snapshot", "BOARD_V2", 1.0, &[], "src")
        .expect("store v2 (changed)");

    // Exactly one LIVE snapshot, and it is the latest content.
    let live = live_facts_for_concept(&mem, "goal-board:snapshot");
    assert_eq!(live.len(), 1, "supersede must leave one live snapshot");
    assert_eq!(live[0].content, "BOARD_V2", "live fact is the new revision");

    // The old revision is archived but still physically present (search_facts
    // does not filter archived) — proving a supersede, not an in-place delete.
    assert_eq!(
        all_physical_facts(&mem).len(),
        2,
        "old revision is archived (still physical) until pruned"
    );
}

/// Repeated changing snapshots never accumulate LIVE duplicates: after N
/// supersedes there is still exactly one live record.
#[test]
fn caller_key_repeated_snapshots_keep_single_live_record() {
    let mem = test_mem();
    let key = "goal-board:snapshot";
    for v in 1..=5 {
        mem.store_fact_with_caller_key(
            key,
            "goal-board:snapshot",
            &format!("BOARD_V{v}"),
            1.0,
            &[],
            "src",
        )
        .expect("snapshot store");
    }
    let live = live_facts_for_concept(&mem, "goal-board:snapshot");
    assert_eq!(live.len(), 1, "5 changing snapshots => 1 live record");
    assert_eq!(live[0].content, "BOARD_V5");
}

// ─── prune_superseded ───────────────────────────────────────────────────────

/// `prune_superseded` reclaims the archived/superseded tail: after a supersede
/// it reports >= 1 reclaimed and the physical fact count drops back to the
/// single live record.
#[test]
fn prune_superseded_reclaims_archived_tail() {
    let mem = test_mem();
    let key = "goal-board:snapshot";
    mem.store_fact_with_caller_key(key, "goal-board:snapshot", "BOARD_V1", 1.0, &[], "src")
        .expect("store v1");
    mem.store_fact_with_caller_key(key, "goal-board:snapshot", "BOARD_V2", 1.0, &[], "src")
        .expect("store v2");
    assert_eq!(
        all_physical_facts(&mem).len(),
        2,
        "one archived tail exists"
    );

    let reclaimed = mem.prune_superseded().expect("prune");
    assert!(reclaimed >= 1, "at least the superseded tail is reclaimed");

    // Only the live record survives; the live record itself is untouched.
    assert_eq!(
        all_physical_facts(&mem).len(),
        1,
        "superseded tail reclaimed, live record retained"
    );
    let live = live_facts_for_concept(&mem, "goal-board:snapshot");
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].content, "BOARD_V2");
}

/// `prune_superseded` is a no-op (returns 0) when there is nothing superseded —
/// it must never evict live records.
#[test]
fn prune_superseded_no_op_when_nothing_superseded() {
    let mem = test_mem();
    mem.store_fact("topic", "live", 0.9, &[], "src")
        .expect("store");
    let reclaimed = mem.prune_superseded().expect("prune");
    assert_eq!(reclaimed, 0, "nothing to reclaim");
    assert_eq!(all_physical_facts(&mem).len(), 1, "live fact retained");
}

// ─── minimal non-library mock for the default-delegation test ────────────────

fn fact(concept: &str, content: &str, confidence: f64) -> CognitiveFact {
    CognitiveFact {
        node_id: format!("node-{concept}"),
        concept: concept.to_string(),
        content: content.to_string(),
        confidence,
        source_id: "mock".to_string(),
        tags: vec![],
        usage_count: 0,
        last_accessed_at: None,
    }
}

/// A non-library `CognitiveMemoryOps` backend that only implements
/// `search_facts`; every other method is a stub. It does NOT override
/// `recall_facts_ranked` / `store_fact_with_caller_key` / `prune_superseded`,
/// so those exercise the trait defaults.
struct SearchOnlyMock {
    facts: Vec<CognitiveFact>,
}

impl CognitiveMemoryOps for SearchOnlyMock {
    fn record_sensory(&self, _m: &str, _r: &str, _t: u64) -> SimardResult<String> {
        Ok("sen".into())
    }
    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        Ok(0)
    }
    fn push_working(&self, _s: &str, _c: &str, _t: &str, _r: f64) -> SimardResult<String> {
        Ok("wrk".into())
    }
    fn get_working(&self, _t: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        Ok(vec![])
    }
    fn clear_working(&self, _t: &str) -> SimardResult<usize> {
        Ok(0)
    }
    fn store_episode(
        &self,
        _c: &str,
        _s: &str,
        _m: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        Ok("epi".into())
    }
    fn consolidate_episodes(&self, _b: u32) -> SimardResult<Option<String>> {
        Ok(None)
    }
    fn store_fact(
        &self,
        _concept: &str,
        _content: &str,
        _confidence: f64,
        _tags: &[String],
        _source_id: &str,
    ) -> SimardResult<String> {
        Ok("sem".into())
    }
    fn search_facts(&self, _q: &str, _l: u32, _c: f64) -> SimardResult<Vec<CognitiveFact>> {
        Ok(self.facts.clone())
    }
    fn store_procedure(&self, _n: &str, _s: &[String], _p: &[String]) -> SimardResult<String> {
        Ok("prc".into())
    }
    fn recall_procedure(&self, _q: &str, _l: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        Ok(vec![])
    }
    fn store_prospective(&self, _d: &str, _t: &str, _a: &str, _p: i64) -> SimardResult<String> {
        Ok("pro".into())
    }
    fn check_triggers(&self, _c: &str) -> SimardResult<Vec<CognitiveProspective>> {
        Ok(vec![])
    }
    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        Ok(CognitiveStatistics::default())
    }
}
