//! TDD (RED) test: Observe vs Decide phase weights produce a *different*
//! ranked-recall ordering of the same **episode** set (issue #2395, parity with
//! the fact-side [`super::tests_phase_recall`] for #2329).
//!
//! Episodes are scored without a confidence/importance term (those are facts-only),
//! so the phase divergence is driven here through the **usage** and
//! **text-relevance** signals, which the Observe and Decide presets weight
//! differently:
//!
//! * `E_text` — high text relevance to the query, never reinforced (usage 0).
//! * `E_usage` — low text relevance, but reinforced repeatedly so its usage
//!   signal is strong.
//!
//! The **Observe** preset (recency/usage-leaning: text 0.8, usage 0.4) ranks the
//! heavily-used `E_usage` first; the **Decide** preset (relevance-leaning:
//! text 1.0, usage 0.3) ranks the text-relevant `E_text` first. Both freshly
//! created/accessed episodes share an ~equal recency term, so it cancels and the
//! ordering turns on the usage-vs-text trade-off — which is exactly what the
//! per-phase weights tune.
//!
//! Recall runs with `record_access = false`, so the first (Observe) recall does
//! not mutate usage/recency and skew the second (Decide) recall — both see the
//! identical episode state.

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory, MemoryKind};

use super::OodaPhase;
use super::phase_weights::weights_for_phase;

#[test]
fn observe_and_decide_weights_change_episode_recall_ordering() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory DB");

    // E_text: high text relevance (jaccard 1.0 with the query), usage 0.
    let _e_text = mem
        .store_episode("alpha beta gamma delta", "test-src", None)
        .expect("store E_text");
    // E_usage: low text relevance (jaccard 0.1), but heavily reinforced so its
    // usage signal dominates a usage-leaning phase.
    let e_usage = mem
        .store_episode("alpha foo bar baz qux quux corge", "test-src", None)
        .expect("store E_usage");
    for _ in 0..10 {
        mem.reinforce_access(&e_usage, MemoryKind::Episode)
            .expect("reinforce E_usage");
    }

    let query = "alpha beta gamma delta";

    let observed = mem
        .recall_episodes_ranked(query, 10, weights_for_phase(OodaPhase::Observe))
        .expect("observe recall");
    let decided = mem
        .recall_episodes_ranked(query, 10, weights_for_phase(OodaPhase::Decide))
        .expect("decide recall");

    assert_eq!(observed.len(), 2, "both episodes recalled under Observe");
    assert_eq!(decided.len(), 2, "both episodes recalled under Decide");

    // Observe is usage/recency-leaning: the heavily-used episode wins.
    assert_eq!(
        observed[0].node_id, e_usage,
        "Observe must rank the heavily-used episode first"
    );
    // Decide is relevance-leaning: the text-relevant episode wins.
    assert_ne!(
        decided[0].node_id, e_usage,
        "Decide must rank the text-relevant episode first"
    );

    // The crux: the SAME episode set is ordered differently per phase.
    let observe_order: Vec<&str> = observed.iter().map(|e| e.node_id.as_str()).collect();
    let decide_order: Vec<&str> = decided.iter().map(|e| e.node_id.as_str()).collect();
    assert_ne!(
        observe_order, decide_order,
        "Observe and Decide weights must yield different episode orderings"
    );
}
