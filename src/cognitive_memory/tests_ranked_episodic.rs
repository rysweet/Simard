//! TDD (RED) tests for ranked episodic recall + the usage/recency
//! reinforcement seam (issue #2395).
//!
//! Completes the recall-quality upgrade #2329 wired for facts by extending the
//! library's multi-signal ranked recall to **episodes**, and by adding the
//! reinforce-at-use seam the ranker's `usage` / `recency` signals depend on.
//! These pin the new `CognitiveMemoryOps` surface against the lbug-backed
//! `LibraryCognitiveMemory::in_memory()` adapter (and the trait *default*
//! against a non-library mock):
//!
//! * `recall_episodes_ranked(query, limit, weights)` — returns episodes in
//!   **descending score order** (relevance + recency + usage + graph), recovers
//!   compressed consolidation sources via the UNION backfill, and defaults to
//!   `search_episodes_by_keywords` (whitespace-split query) for non-library
//!   backends.
//! * `reinforce_access(node_id, MemoryKind)` — records that a recalled fact /
//!   episode / procedure was used, bumping its `usage_count` and
//!   `last_accessed_at` (now surfaced on `CognitiveFact`).

use super::{CognitiveMemoryOps, LibraryCognitiveMemory, MemoryKind, RecallWeightSet};
use crate::error::SimardResult;
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

fn test_mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory DB should create")
}

// ─── recall_episodes_ranked: ordering ───────────────────────────────────────

/// Invariant #1 (descending recall order): the most text-relevant episode leads
/// even when it is the **older** one. A newest-first keyword scan
/// (`search_episodes_by_keywords`) would have surfaced the diluted recent
/// episode first.
#[test]
fn recall_episodes_ranked_orders_by_relevance_not_recency() {
    let mem = test_mem();
    // E_relevant stored FIRST (older / lower temporal_index): exact keyword match.
    mem.store_episode("payment", "test-src", None)
        .expect("store E_relevant");
    // E_recent stored SECOND (newer): the keyword is diluted across many tokens.
    mem.store_episode(
        "payment alpha beta gamma delta epsilon zeta eta theta",
        "test-src",
        None,
    )
    .expect("store E_recent");

    let ranked = mem
        .recall_episodes_ranked("payment", 10, RecallWeightSet::default())
        .expect("ranked episodic recall");

    assert_eq!(ranked.len(), 2, "both keyword-matching episodes recalled");
    assert_eq!(
        ranked[0].content, "payment",
        "ranked recall leads with the most relevant (older) episode, not the newest"
    );
}

// ─── recall_episodes_ranked: compressed-source UNION backfill ────────────────

/// Invariant #5 (compressed sources preserved): episodes folded into a
/// consolidation summary are flagged `compressed`, which the library's ranked
/// path skips — but Simard's UNION backfill must keep them recallable so a
/// distilled fact/procedure can always be traced back to its source episodes
/// (regression guard for #2298 / distillation).
#[test]
fn recall_episodes_ranked_recovers_compressed_consolidation_source() {
    let mem = test_mem();
    let e1 = mem
        .store_episode("synchronization failed on shard one", "test-src", None)
        .expect("store e1");
    let e2 = mem
        .store_episode("synchronization retry succeeded", "test-src", None)
        .expect("store e2");

    // Consolidate → both source episodes are now flagged `compressed`; the
    // library ranked path skips compressed episodes, so only the UNION backfill
    // can surface them.
    let consolidated = mem.consolidate_episodes(2).expect("consolidate");
    assert!(
        consolidated.is_some(),
        "two episodes consolidate into a summary"
    );

    let recalled = mem
        .recall_episodes_ranked("synchronization", 10, RecallWeightSet::default())
        .expect("ranked episodic recall");

    let ids: std::collections::HashSet<&str> =
        recalled.iter().map(|e| e.node_id.as_str()).collect();
    assert!(
        ids.contains(e1.as_str()),
        "compressed consolidation source e1 stays recallable via the UNION backfill"
    );
    assert!(
        ids.contains(e2.as_str()),
        "compressed consolidation source e2 stays recallable via the UNION backfill"
    );
    assert!(
        recalled.iter().any(|e| e.compressed),
        "the recovered sources are flagged compressed"
    );
}

// ─── recall_episodes_ranked: default back-compat ─────────────────────────────

/// Invariant #6 (default back-compat): a backend that does NOT override
/// `recall_episodes_ranked` falls back to `search_episodes_by_keywords`, with
/// the `query` split on whitespace into keywords and the `limit` forwarded
/// (`weights` ignored). Pinned against a minimal non-library mock that echoes
/// the keywords it received.
#[test]
fn recall_episodes_ranked_default_delegates_to_keyword_scan() {
    let mock = EchoKeywordsMock;

    let got = mock
        .recall_episodes_ranked("alpha beta gamma", 7, RecallWeightSet::default())
        .expect("default recall_episodes_ranked");

    assert_eq!(got.len(), 1, "mock returns exactly one echo episode");
    assert_eq!(
        got[0].content, "alpha,beta,gamma",
        "default impl must split the query on whitespace and delegate to search_episodes_by_keywords"
    );
    assert_eq!(
        got[0].node_id, "epi-7",
        "default impl must forward the limit unchanged"
    );
}

// ─── reinforce_access: usage/recency reinforcement ───────────────────────────

/// Invariant #7 (reinforcement seam, facts) + fact observability: a stored fact
/// starts un-reinforced (`usage_count == 0`, `last_accessed_at == None`);
/// `reinforce_access(node_id, MemoryKind::Fact)` bumps `usage_count` and stamps
/// `last_accessed_at`, and both counters are surfaced on `CognitiveFact`.
#[test]
fn reinforce_access_increments_fact_usage_and_last_accessed() {
    let mem = test_mem();
    mem.store_fact("deploy", "deploy the payment service", 0.9, &[], "src")
        .expect("store fact");

    let before = mem
        .recall_facts_ranked("deploy", 10, 0.0, RecallWeightSet::default())
        .expect("recall fact");
    assert_eq!(before.len(), 1);
    assert_eq!(
        before[0].usage_count, 0,
        "a freshly stored fact has not been reinforced"
    );
    assert!(
        before[0].last_accessed_at.is_none(),
        "no access has been recorded yet"
    );

    mem.reinforce_access(&before[0].node_id, MemoryKind::Fact)
        .expect("reinforce fact");

    let after = mem
        .recall_facts_ranked("deploy", 10, 0.0, RecallWeightSet::default())
        .expect("recall fact again");
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].usage_count, 1,
        "reinforce_access bumps the fact's usage_count by one"
    );
    assert!(
        after[0].last_accessed_at.is_some(),
        "reinforce_access stamps the fact's last_accessed_at"
    );
}

/// Invariant #7 (reinforcement seam, procedures): a stored procedure starts at
/// `usage_count == 0`; `reinforce_access(node_id, MemoryKind::Procedure)` bumps
/// it — resurrecting the previously-dead procedure reinforcement signal that
/// usage-ordered recall feeds on.
#[test]
fn reinforce_access_increments_procedure_usage_count() {
    let mem = test_mem();
    let steps = vec!["build".to_string(), "ship".to_string()];
    let prereqs: Vec<String> = vec![];
    mem.store_procedure("deploy-service", &steps, &prereqs)
        .expect("store procedure");

    let before = mem
        .recall_procedure("deploy-service", 10)
        .expect("recall procedure");
    assert_eq!(before.len(), 1);
    assert_eq!(
        before[0].usage_count, 0,
        "a freshly stored procedure has usage_count 0"
    );

    mem.reinforce_access(&before[0].node_id, MemoryKind::Procedure)
        .expect("reinforce procedure");

    let after = mem
        .recall_procedure("deploy-service", 10)
        .expect("recall procedure again");
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].usage_count, 1,
        "reinforce_access bumps the procedure's usage_count by one"
    );
}

// ─── minimal non-library mock for the default-delegation test ────────────────

/// A non-library `CognitiveMemoryOps` backend that implements only
/// `search_episodes_by_keywords`, echoing the keywords it received (joined with
/// `,`) and the limit into the returned episode. It does NOT override
/// `recall_episodes_ranked`, so that method exercises the trait default.
struct EchoKeywordsMock;

impl CognitiveMemoryOps for EchoKeywordsMock {
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
        Ok(vec![])
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
    fn search_episodes_by_keywords(
        &self,
        keywords: &[String],
        limit: u32,
    ) -> SimardResult<Vec<CognitiveEpisode>> {
        Ok(vec![CognitiveEpisode {
            node_id: format!("epi-{limit}"),
            content: keywords.join(","),
            source_label: "mock".into(),
            temporal_index: 0,
            compressed: false,
        }])
    }
}
