//! TDD (RED) test: OODA preparation gathers `episodic_recall` via the library's
//! ranked recall (issue #2395), not the flat newest-first
//! `search_episodes_by_keywords` scan — and preparation stays a **pure read**.
//!
//! Mirrors the fact-side [`super::tests_ranked_prep`] (issue #2329) for the
//! episodic stream:
//!
//! * Two episodes both match the objective keyword — one an exact match stored
//!   **earlier** (older), one diluted across many tokens stored **later**
//!   (newer). The flat keyword scan returns newest-first and would surface the
//!   diluted-but-recent episode first; the library's ranked recall (text
//!   relevance dominant under the balanced default weights) surfaces the
//!   exact-match older episode first. Asserting `episodic_recall[0]` is the
//!   older exact match proves preparation is on the ranked path.
//! * Two successive preparation passes over an unchanged store recall episodes
//!   in the same order, and a fact gathered by preparation is **not**
//!   reinforced (`usage_count` stays 0, `last_accessed_at` stays `None`),
//!   proving preparation recall runs with `record_access = false`.

use super::{PreparedContext, preparation_memory_operations_with_active_slugs_phased};
use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory, RecallWeightSet};
use crate::session::SessionId;

fn test_session_id() -> SessionId {
    SessionId::parse("session-01234567-89ab-cdef-0123-456789abcdef").unwrap()
}

#[test]
fn preparation_gathers_episodes_in_ranked_order() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory DB");

    // E_relevant stored FIRST (older): exact keyword match to the objective.
    mem.store_episode("payment", "test-src", None)
        .expect("store E_relevant");
    // E_recent stored SECOND (newer): keyword diluted across many tokens.
    mem.store_episode(
        "payment alpha beta gamma delta epsilon zeta eta theta",
        "test-src",
        None,
    )
    .expect("store E_recent");

    let ctx = preparation_memory_operations_with_active_slugs_phased(
        "payment",
        &test_session_id(),
        &mem,
        None,
        RecallWeightSet::default(),
    )
    .expect("preparation");

    assert_eq!(
        ctx.episodic_recall.len(),
        2,
        "both keyword-matching episodes gathered"
    );
    // Ranked recall (text relevance dominant) leads with the most relevant
    // episode even though it is OLDER; the previous newest-first keyword scan
    // would have surfaced the diluted recent episode first.
    assert_eq!(
        ctx.episodic_recall[0].content, "payment",
        "preparation must gather episodes via ranked recall: the most relevant \
         episode leads, not the newest"
    );
}

#[test]
fn preparation_episodic_recall_is_read_only() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory DB");
    mem.store_episode("deployment", "test-src", None)
        .expect("store e1");
    mem.store_episode("deployment rollback notes", "test-src", None)
        .expect("store e2");
    // A fact on the same objective so we can prove preparation does not reinforce it.
    mem.store_fact("deployment", "deployment runbook", 0.9, &[], "src")
        .expect("store fact");

    let first = preparation_memory_operations_with_active_slugs_phased(
        "deployment",
        &test_session_id(),
        &mem,
        None,
        RecallWeightSet::default(),
    )
    .expect("first preparation");
    let second = preparation_memory_operations_with_active_slugs_phased(
        "deployment",
        &test_session_id(),
        &mem,
        None,
        RecallWeightSet::default(),
    )
    .expect("second preparation");

    let ids = |c: &PreparedContext| {
        c.episodic_recall
            .iter()
            .map(|e| e.node_id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(&first),
        ids(&second),
        "two preparation passes over an unchanged store must recall episodes in \
         the same order (record_access = false)"
    );

    // Preparation gathered the fact via ranked recall but must NOT have
    // reinforced it — recall during preparation is a pure read.
    let fact = mem
        .recall_facts_ranked("deployment", 10, 0.0, RecallWeightSet::default())
        .expect("recall fact");
    assert_eq!(fact.len(), 1);
    assert_eq!(
        fact[0].usage_count, 0,
        "preparation recall must not bump a fact's usage_count"
    );
    assert!(
        fact[0].last_accessed_at.is_none(),
        "preparation recall must not stamp last_accessed_at"
    );
}
