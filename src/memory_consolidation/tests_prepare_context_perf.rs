//! TDD orchestration-perf specification for OODA prepare-context (issue #40).
//!
//! ## Where the fix lives vs. what this module guards
//!
//! The ~11-minute-per-cycle pathology is an **engine-side** N+1 neighbor
//! fan-out in `amplihack-memory-lib`'s ranked recall; the substantive fix (a
//! bulk graph-adjacency index) and its RED perf contract live there. Once that
//! merges, Simard bumps its pinned `amplihack-memory` rev and inherits the win.
//!
//! This module locks the **Simard orchestration invariant** that must hold
//! before, during, and after that dependency bump: prepare-context issues a
//! **small, constant number of expensive recall calls that scales with the
//! objective's structure — never with the size of the fact store**. If a future
//! change ever reintroduced a per-fact fan-out at the orchestration layer (e.g.
//! looping ranked recall once per stored fact), it would re-create the same
//! pathology one layer up; these tests fail the moment that happens.
//!
//! It also pins the observability contract: the `"Prepared context: N facts,
//! …"` summary must keep reaching working memory.
//!
//! The assertions are deterministic **call counts**, never wall-clock timings
//! (policy: no wall-clock timeouts on agentic/perf assertions).

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{RecallWeightSet, preparation_memory_operations_with_active_slugs_phased};
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};
use crate::session::SessionId;

fn test_session_id() -> SessionId {
    SessionId::parse("session-01234567-89ab-cdef-0123-456789abcdef").unwrap()
}

/// A `CognitiveMemoryOps` that counts the expensive recall round-trips
/// prepare-context issues and captures every working-memory push.
///
/// `facts_to_return` / `episodes_to_return` let a test vary the *result volume*
/// of each recall independently of the *number of recall calls*, so it can
/// prove result volume never multiplies the call count (the anti-fan-out
/// invariant).
struct CountingMemoryOps {
    facts_to_return: usize,
    episodes_to_return: usize,

    facts_ranked_calls: AtomicUsize,
    episodes_ranked_calls: AtomicUsize,
    search_facts_calls: AtomicUsize,
    check_triggers_calls: AtomicUsize,
    recall_procedure_calls: AtomicUsize,

    working_pushes: Mutex<Vec<(String, String)>>,
}

impl CountingMemoryOps {
    fn new(facts_to_return: usize, episodes_to_return: usize) -> Self {
        Self {
            facts_to_return,
            episodes_to_return,
            facts_ranked_calls: AtomicUsize::new(0),
            episodes_ranked_calls: AtomicUsize::new(0),
            search_facts_calls: AtomicUsize::new(0),
            check_triggers_calls: AtomicUsize::new(0),
            recall_procedure_calls: AtomicUsize::new(0),
            working_pushes: Mutex::new(Vec::new()),
        }
    }

    fn facts_ranked_calls(&self) -> usize {
        self.facts_ranked_calls.load(Ordering::SeqCst)
    }
    fn episodes_ranked_calls(&self) -> usize {
        self.episodes_ranked_calls.load(Ordering::SeqCst)
    }
    fn search_facts_calls(&self) -> usize {
        self.search_facts_calls.load(Ordering::SeqCst)
    }
    fn check_triggers_calls(&self) -> usize {
        self.check_triggers_calls.load(Ordering::SeqCst)
    }
    fn working_pushes(&self) -> Vec<(String, String)> {
        self.working_pushes.lock().unwrap().clone()
    }
}

impl CognitiveMemoryOps for CountingMemoryOps {
    fn record_sensory(&self, _m: &str, _r: &str, _t: u64) -> SimardResult<String> {
        Ok("sen_x".to_string())
    }
    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        Ok(0)
    }
    fn push_working(&self, slot: &str, content: &str, _t: &str, _r: f64) -> SimardResult<String> {
        self.working_pushes
            .lock()
            .unwrap()
            .push((slot.to_string(), content.to_string()));
        Ok("wrk_x".to_string())
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
        Ok("epi_x".to_string())
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
        Ok("sem_x".to_string())
    }
    fn search_facts(&self, _q: &str, _l: u32, _c: f64) -> SimardResult<Vec<CognitiveFact>> {
        // The one-shot goal-fact scan. Return empty so the goal-dedup path is a
        // no-op and the test isolates the ranked-recall call accounting.
        self.search_facts_calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![])
    }
    fn recall_facts_ranked(
        &self,
        _query: &str,
        _limit: u32,
        _min_confidence: f64,
        _weights: RecallWeightSet,
    ) -> SimardResult<Vec<CognitiveFact>> {
        self.facts_ranked_calls.fetch_add(1, Ordering::SeqCst);
        let facts = (0..self.facts_to_return)
            .map(|i| CognitiveFact {
                node_id: format!("sem_{i}"),
                concept: "relevant-topic".to_string(),
                content: format!("ranked fact {i}"),
                confidence: 0.7,
                source_id: "src".to_string(),
                tags: vec![],
                usage_count: 0,
                last_accessed_at: None,
            })
            .collect();
        Ok(facts)
    }
    fn store_procedure(&self, _n: &str, _s: &[String], _p: &[String]) -> SimardResult<String> {
        Ok("prc_x".to_string())
    }
    fn recall_procedure(&self, _q: &str, _l: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        self.recall_procedure_calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![])
    }
    fn store_prospective(&self, _d: &str, _t: &str, _a: &str, _p: i64) -> SimardResult<String> {
        Ok("pro_x".to_string())
    }
    fn check_triggers(&self, _c: &str) -> SimardResult<Vec<CognitiveProspective>> {
        self.check_triggers_calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![])
    }
    fn recall_episodes_ranked(
        &self,
        _query: &str,
        _limit: u32,
        _weights: RecallWeightSet,
    ) -> SimardResult<Vec<CognitiveEpisode>> {
        self.episodes_ranked_calls.fetch_add(1, Ordering::SeqCst);
        let episodes = (0..self.episodes_to_return)
            .map(|i| CognitiveEpisode {
                node_id: format!("epi_{i}"),
                content: format!("ranked episode {i}"),
                // Not a "session-" prefix, so it survives the self-session filter.
                source_label: "external".to_string(),
                temporal_index: i as i64,
                compressed: false,
                created_at: None,
            })
            .collect();
        Ok(episodes)
    }
    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        Ok(CognitiveStatistics::default())
    }
}

fn prep(mock: &CountingMemoryOps, objective: &str) -> super::PreparedContext {
    preparation_memory_operations_with_active_slugs_phased(
        objective,
        &test_session_id(),
        mock,
        None,
        RecallWeightSet::default(),
    )
    .expect("preparation must succeed")
}

// ---------------------------------------------------------------------------

#[test]
fn single_fragment_objective_issues_exactly_one_ranked_fact_recall() {
    // A single objective (no "; " fragments) must trigger exactly one ranked
    // fact recall and exactly one ranked episodic recall — even though the fact
    // recall returns a large result set.
    let mock = CountingMemoryOps::new(500, 20);
    let _ = prep(&mock, "improve the rust async runtime throughput");

    assert_eq!(
        mock.facts_ranked_calls(),
        1,
        "a one-fragment objective must issue exactly one ranked fact recall"
    );
    assert_eq!(
        mock.episodes_ranked_calls(),
        1,
        "episodic ranked recall must be issued exactly once per prepare-context"
    );
}

#[test]
fn ranked_recall_call_count_tracks_fragments_not_fact_volume() {
    // The load-bearing orchestration invariant: the number of ranked recall
    // calls is a function of the OBJECTIVE (its "; "-joined fragments), NEVER of
    // how many facts the store holds / a recall returns. Returning 5000 facts
    // must not multiply the number of recall calls.
    let objective = "reduce prepare-context latency; \
                     index graph adjacency in the engine; \
                     preserve ranking parity";

    let few = CountingMemoryOps::new(3, 1);
    let many = CountingMemoryOps::new(5000, 1);

    let _ = prep(&few, objective);
    let _ = prep(&many, objective);

    assert_eq!(
        few.facts_ranked_calls(),
        3,
        "three fragments => three ranked fact recalls"
    );
    assert_eq!(
        many.facts_ranked_calls(),
        few.facts_ranked_calls(),
        "returning 5000 facts must NOT multiply ranked recall calls — \
         prepare-context recall count is O(objective fragments), never \
         O(fact-store size)"
    );
    assert_eq!(
        many.episodes_ranked_calls(),
        1,
        "episodic recall stays a single call regardless of fact volume"
    );
    assert_eq!(
        few.episodes_ranked_calls(),
        many.episodes_ranked_calls(),
        "episodic recall count must be independent of fact volume"
    );
}

#[test]
fn prepare_context_expensive_reads_are_constant_per_objective() {
    // Beyond ranked recall, the other expensive whole-store reads
    // (goal-fact scan, trigger check) must each fire exactly once, independent
    // of fact volume — the total expensive-read budget of a prepare-context is
    // a small constant, not a function of the store size.
    let small = CountingMemoryOps::new(2, 1);
    let large = CountingMemoryOps::new(4000, 1);

    let objective = "single fragment objective about rust async runtime";
    let _ = prep(&small, objective);
    let _ = prep(&large, objective);

    for mock in [&small, &large] {
        assert_eq!(mock.facts_ranked_calls(), 1);
        assert_eq!(mock.episodes_ranked_calls(), 1);
        assert_eq!(mock.search_facts_calls(), 1, "exactly one goal-fact scan");
        assert_eq!(mock.check_triggers_calls(), 1, "exactly one trigger check");
    }
}

#[test]
fn prepare_context_preserves_prepared_context_summary_observability() {
    // The `"Prepared context: N facts, M triggers, P procedures, Q episodes"`
    // summary must keep reaching working memory — this is the operator-facing
    // observability the fix must NOT regress.
    let mock = CountingMemoryOps::new(4, 2);
    let _ = prep(&mock, "improve rust async runtime performance");

    let pushes = mock.working_pushes();
    let summary = pushes
        .iter()
        .find(|(slot, _)| slot == "context-summary")
        .map(|(_, content)| content.clone())
        .expect("a 'context-summary' slot must be pushed to working memory");

    assert!(
        summary.starts_with("Prepared context:"),
        "the 'Prepared context: …' observability line must be preserved, got: {summary:?}"
    );
    assert!(
        summary.contains("facts") && summary.contains("episodes"),
        "the summary must still report facts and episodes counts, got: {summary:?}"
    );
}
