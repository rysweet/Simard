//! TDD test: OODA preparation gathers `relevant_facts` via ranked recall
//! (issue #2329), not a plain confidence-sorted `search_facts`.
//!
//! Two facts both match the objective keyword:
//! * `F_a` — exact text match (jaccard 1.0) but only **0.5** confidence.
//! * `F_b` — diluted text match (jaccard 0.1) but **0.9** confidence.
//!
//! A plain `search_facts` orders purely by confidence and would surface `F_b`
//! first. The library's ranked recall — where text relevance is the dominant
//! signal under the balanced default weights — surfaces `F_a` first. Asserting
//! `relevant_facts[0]` is the lower-confidence-but-more-relevant `F_a` proves
//! preparation is on the ranked path.

use super::{RecallWeightSet, preparation_memory_operations_with_active_slugs_phased};
use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::session::SessionId;

fn test_session_id() -> SessionId {
    SessionId::parse("session-01234567-89ab-cdef-0123-456789abcdef").unwrap()
}

#[test]
fn preparation_gathers_relevant_facts_in_ranked_order() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory DB");

    // F_a: exact match to the objective token, LOW confidence.
    mem.store_fact("refactor", "refactor", 0.5, &[], "src")
        .expect("store F_a");
    // F_b: contains the token but heavily diluted, HIGH confidence.
    mem.store_fact(
        "task",
        "refactor alpha beta gamma delta epsilon zeta eta theta",
        0.9,
        &[],
        "src",
    )
    .expect("store F_b");

    let ctx = preparation_memory_operations_with_active_slugs_phased(
        "refactor",
        &test_session_id(),
        &mem,
        None,
        RecallWeightSet::default(),
    )
    .expect("preparation");

    assert_eq!(
        ctx.relevant_facts.len(),
        2,
        "both keyword-matching facts gathered"
    );
    // Ranked recall (text relevance dominant) ranks the exact-match fact first,
    // even though its confidence is lower — a confidence-sorted `search_facts`
    // would have put the 0.9-confidence fact first.
    assert_eq!(
        ctx.relevant_facts[0].confidence, 0.5,
        "preparation must use ranked recall: the most relevant fact leads, \
         not the most confident"
    );
    assert_eq!(ctx.relevant_facts[0].concept, "refactor");
}
