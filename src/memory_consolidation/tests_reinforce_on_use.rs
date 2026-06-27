//! Test: reinforcement is resurrected at the point of *use* (issue #2395).
//!
//! Preparation recall is a deliberate pure read, so reinforcement does not run
//! while gathering context (proved by [`super::tests_ranked_episodic_prep`]).
//! Instead [`super::reinforce_prepared_context`] — invoked from the OODA
//! `advance.rs` injection point once the recalled facts / procedures / episodes
//! are actually surfaced into the cycle's prompt — bumps each surfaced node's
//! `usage_count` (and stamps `last_accessed_at`) through the
//! [`CognitiveMemoryOps::reinforce_access`] seam.
//!
//! Before #2395 nothing incremented `usage_count` on recall; this test pins that
//! the seam is now driven for the prepared context (a fact and a procedure —
//! the two kinds that surface a `usage_count`), feeding the ranked-recall
//! usage/recency signals on subsequent cycles.

use super::{PreparedContext, reinforce_prepared_context};
use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory, RecallWeightSet};

#[test]
fn reinforce_prepared_context_bumps_surfaced_fact_and_procedure_usage() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory DB");

    mem.store_fact("deploy", "deploy the payment service", 0.9, &[], "src")
        .expect("store fact");
    let steps = vec!["build".to_string(), "ship".to_string()];
    let prereqs: Vec<String> = vec![];
    mem.store_procedure("deploy-service", &steps, &prereqs)
        .expect("store procedure");
    mem.store_episode(
        "deployed the payment service to prod",
        "engineer-cycle",
        None,
    )
    .expect("store episode");

    // Gather exactly what preparation would surface (all pure reads).
    let relevant_facts = mem
        .recall_facts_ranked("deploy", 10, 0.0, RecallWeightSet::default())
        .expect("recall facts");
    let recalled_procedures = mem
        .recall_procedure("deploy-service", 10)
        .expect("recall proc");
    let episodic_recall = mem
        .recall_episodes_ranked("deploy", 10, RecallWeightSet::default())
        .expect("recall episodes");

    assert_eq!(relevant_facts.len(), 1, "fact gathered");
    assert_eq!(recalled_procedures.len(), 1, "procedure gathered");
    assert_eq!(episodic_recall.len(), 1, "episode gathered");
    assert_eq!(
        relevant_facts[0].usage_count, 0,
        "gathering (pure read) must not reinforce the fact"
    );
    assert!(
        relevant_facts[0].last_accessed_at.is_none(),
        "gathering (pure read) must not stamp the fact"
    );
    assert_eq!(
        recalled_procedures[0].usage_count, 0,
        "gathering (pure read) must not reinforce the procedure"
    );

    let ctx = PreparedContext {
        relevant_facts,
        triggered_prospectives: vec![],
        recalled_procedures,
        episodic_recall,
    };

    // Point of use: surface the recalled context into the cycle → reinforce.
    reinforce_prepared_context(&mem, &ctx);

    let fact_after = mem
        .recall_facts_ranked("deploy", 10, 0.0, RecallWeightSet::default())
        .expect("recall fact again");
    assert_eq!(
        fact_after[0].usage_count, 1,
        "using the surfaced fact must reinforce its usage_count"
    );
    assert!(
        fact_after[0].last_accessed_at.is_some(),
        "using the surfaced fact must stamp last_accessed_at"
    );

    let proc_after = mem
        .recall_procedure("deploy-service", 10)
        .expect("recall proc again");
    assert_eq!(
        proc_after[0].usage_count, 1,
        "using the surfaced procedure must reinforce its usage_count"
    );
}
