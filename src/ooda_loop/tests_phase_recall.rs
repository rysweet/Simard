//! TDD test: Observe vs Decide phase weights produce a *different* ranked-recall
//! ordering of the same fact set (issue #2329, invariant #4).
//!
//! The phase-weight numeric presets are unit-tested in
//! [`super::phase_weights`]; this test closes the loop by feeding those presets
//! into the real lbug-backed ranked recall and asserting the ordering actually
//! diverges:
//!
//! * Two facts are stored — one with high **text relevance** but low confidence
//!   (`F_text`), one with no text overlap but higher confidence (`F_conf`).
//! * The **Observe** preset (recency/relevance-leaning, confidence 0.5) ranks
//!   `F_text` first.
//! * The **Decide** preset (confidence 1.0) ranks `F_conf` first.
//!
//! Recall runs with `record_access = false`, so the first (Observe) recall does
//! not mutate usage/recency and skew the second (Decide) recall — both see the
//! identical fact state.

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};

use super::OodaPhase;
use super::phase_weights::weights_for_phase;

#[test]
fn observe_and_decide_weights_change_recall_ordering() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory DB");

    // F_text: high text relevance to the query (jaccard 3/4), low confidence.
    //   text = "alpha beta gamma"  vs query "alpha beta gamma delta".
    mem.store_fact("alpha", "beta gamma", 0.1, &[], "src")
        .expect("store F_text");
    // F_conf: no token overlap with the query, higher confidence.
    mem.store_fact("omega", "sigma", 0.6, &[], "src")
        .expect("store F_conf");

    let query = "alpha beta gamma delta";

    let observed = mem
        .recall_facts_ranked(query, 10, 0.0, weights_for_phase(OodaPhase::Observe))
        .expect("observe recall");
    let decided = mem
        .recall_facts_ranked(query, 10, 0.0, weights_for_phase(OodaPhase::Decide))
        .expect("decide recall");

    assert_eq!(observed.len(), 2, "both facts recalled under Observe");
    assert_eq!(decided.len(), 2, "both facts recalled under Decide");

    // Observe is relevance/recency-leaning: the text-relevant fact wins.
    assert_eq!(
        observed[0].confidence, 0.1,
        "Observe must rank the text-relevant fact first"
    );
    // Decide is confidence-leaning: the high-confidence fact wins.
    assert_eq!(
        decided[0].confidence, 0.6,
        "Decide must rank the high-confidence fact first"
    );

    // The crux of issue #2329: the SAME fact set is ordered differently per phase.
    let observe_order: Vec<&str> = observed.iter().map(|f| f.concept.as_str()).collect();
    let decide_order: Vec<&str> = decided.iter().map(|f| f.concept.as_str()).collect();
    assert_ne!(
        observe_order, decide_order,
        "Observe and Decide weights must yield different orderings"
    );
}
