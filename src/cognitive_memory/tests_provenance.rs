//! TDD (RED) tests for fact/procedure provenance wiring (issue #2325).
//!
//! These tests pin the contract for the three new `CognitiveMemoryOps`
//! methods that turn the flat cognitive-memory node store into a
//! connected graph by recording where distilled facts/procedures came
//! from (DERIVES_FROM / PROCEDURE_DERIVES_FROM edges):
//!
//! * `store_fact_with_provenance(concept, content, confidence, source_id,
//!    tags, metadata, source_episode_ids) -> SimardResult<String>`
//! * `store_procedure_with_provenance(name, steps, prerequisites,
//!    source_episode_ids) -> SimardResult<String>`
//! * `episodes_for_fact(fact_id) -> SimardResult<Vec<String>>`
//!
//! All three land on the trait with **defaulted** impls so the other
//! `CognitiveMemoryOps` implementors keep compiling unchanged; only
//! [`LibraryCognitiveMemory`] overrides them against the lbug-backed
//! `GraphStore` (writing/reading the DERIVES_FROM edges).
//!
//! These tests target `LibraryCognitiveMemory::in_memory()` directly —
//! the in-memory `GraphStore` implements `query_neighbors`, so the edge
//! round-trip is exercised without going through the memory/IPC layer.
//!
//! ## Param-order footgun (intentional)
//!
//! The new `store_fact_with_provenance` follows the **library** argument
//! order — `source_id` BEFORE `tags`, and `tags`/`metadata` are
//! `Option`s — which is the *opposite* of the legacy
//! `store_fact(concept, content, confidence, tags, source_id)`. The tests
//! call the new method in library order on purpose; the trait default
//! impl memories the swap for non-library backends.
//!
//! ## Expected red signal
//!
//! Before the wiring lands, none of the three methods exist, so every
//! call below fails to resolve and the crate test build fails to
//! compile. That unresolved-method error IS the intended TDD red signal
//! (same convention as `tests_pr_b_distill` / `distillation_tests`).
//! Once the dependency is bumped to the provenance-capable library rev
//! and the adapter overrides land, these assertions drive GREEN.

use std::collections::HashMap;

use super::{CognitiveMemoryOps, LibraryCognitiveMemory};

fn test_mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory DB should create")
}

/// Core contract (issue #2325, acceptance #4 + #8): a fact stored with
/// provenance for an episode must be **recallable** back to that episode
/// through Simard's own API. This is the round-trip that proves a
/// DERIVES_FROM edge was created and can be traversed.
///
/// The `fact_id` handed to `episodes_for_fact` is exactly the id returned
/// by `store_fact_with_provenance`, so the assertion is a pure
/// write-then-read round-trip and does not couple to the internal
/// node-id format.
#[test]
fn store_fact_with_provenance_links_episode_recallable() {
    let mem = test_mem();

    let ep = mem
        .store_episode(
            "enabled auto-merge then waited for CI",
            "engineer-loop",
            None,
        )
        .expect("store_episode");

    let fact_id = mem
        .store_fact_with_provenance(
            "pr-pattern",
            "enable auto-merge before final review",
            0.7,
            &format!("distill:{ep}"),
            None,                      // tags
            None,                      // metadata
            std::slice::from_ref(&ep), // source_episode_ids
        )
        .expect("store_fact_with_provenance");

    let episodes = mem.episodes_for_fact(&fact_id).expect("episodes_for_fact");

    assert!(
        episodes.contains(&ep),
        "fact {fact_id} must record a DERIVES_FROM edge back to its source \
         episode {ep}; episodes_for_fact returned {episodes:?}"
    );
}

/// Fan-out: a fact distilled from several episodes must link to **all**
/// of them (plural `source_episode_ids` / `link_fact_to_episodes`).
#[test]
fn store_fact_with_provenance_links_multiple_episodes() {
    let mem = test_mem();

    let ep_a = mem
        .store_episode("episode alpha", "engineer-loop", None)
        .unwrap();
    let ep_b = mem
        .store_episode("episode beta", "engineer-loop", None)
        .unwrap();
    let ep_c = mem
        .store_episode("episode gamma", "engineer-loop", None)
        .unwrap();

    let fact_id = mem
        .store_fact_with_provenance(
            "lesson-learned",
            "three episodes all point at the same lesson",
            0.7,
            "distill:multi",
            None,
            None,
            &[ep_a.clone(), ep_b.clone(), ep_c.clone()],
        )
        .unwrap();

    let episodes: std::collections::HashSet<String> = mem
        .episodes_for_fact(&fact_id)
        .unwrap()
        .into_iter()
        .collect();

    assert!(
        episodes.contains(&ep_a),
        "missing edge to {ep_a}: {episodes:?}"
    );
    assert!(
        episodes.contains(&ep_b),
        "missing edge to {ep_b}: {episodes:?}"
    );
    assert!(
        episodes.contains(&ep_c),
        "missing edge to {ep_c}: {episodes:?}"
    );
}

/// Negative control: a fact stored through the legacy `store_fact`
/// (no provenance) must have **no** DERIVES_FROM edges. This guards
/// against `episodes_for_fact` fabricating links and proves the edge is
/// only created on the provenance path.
#[test]
fn store_fact_without_provenance_has_no_edges() {
    let mem = test_mem();

    // Legacy signature: tags BEFORE source_id, tags is a plain slice.
    let fact_id = mem
        .store_fact(
            "pr-pattern",
            "a fact with no source episodes",
            0.7,
            &["pr-pattern".to_string()],
            "manual:no-provenance",
        )
        .unwrap();

    let episodes = mem.episodes_for_fact(&fact_id).unwrap();
    assert!(
        episodes.is_empty(),
        "a fact stored without provenance must have zero DERIVES_FROM \
         edges; got {episodes:?}"
    );
}

/// `episodes_for_fact` on an unknown id must be empty, not an error —
/// callers tolerate facts that predate provenance wiring.
#[test]
fn episodes_for_unknown_fact_is_empty() {
    let mem = test_mem();
    let episodes = mem
        .episodes_for_fact("sem_does_not_exist")
        .expect("episodes_for_fact must not error on unknown id");
    assert!(episodes.is_empty(), "unknown fact must yield no episodes");
}

/// Invariant preservation (issue #2325, acceptance #3): the provenance
/// write path must behave like base `store_fact` for everything except
/// the edges — the fact stays searchable, and its `source_id`/`tags`
/// round-trip. Empty episode list ⇒ no edges but still a normal fact.
#[test]
fn store_fact_with_provenance_preserves_search_and_source_id() {
    let mem = test_mem();

    let tags = vec!["pr-pattern".to_string(), "automation".to_string()];
    let _fact_id = mem
        .store_fact_with_provenance(
            "pr-pattern",
            "auto-merge keeps the queue moving",
            0.9,
            "session:abc",
            Some(tags.as_slice()),
            None,
            &[], // no source episodes — must still store a normal fact
        )
        .unwrap();

    let hits = mem.search_facts("pr-pattern", 10, 0.0).unwrap();
    let found = hits
        .iter()
        .find(|f| f.concept == "pr-pattern" && f.content == "auto-merge keeps the queue moving")
        .expect("provenance-written fact must be searchable like any other fact");

    assert_eq!(found.source_id, "session:abc", "source_id must round-trip");
    let tag_set: std::collections::HashSet<&str> = found.tags.iter().map(String::as_str).collect();
    assert!(
        tag_set.contains("pr-pattern"),
        "tags must round-trip: {:?}",
        found.tags
    );
    assert!(
        tag_set.contains("automation"),
        "tags must round-trip: {:?}",
        found.tags
    );
}

/// Invariant preservation (issue #2325, acceptance #3): the provenance
/// write must stamp the per-store monotonic sequence (`FACT_SEQ_META_KEY`)
/// exactly like base `store_fact`, so the "max node_id == newest fact"
/// selection used by the goal board / goal store / consolidation stays
/// correct. Without the stamp, two same-concept facts written in the same
/// second would order by random UUID and this assertion would flake.
#[test]
fn store_fact_with_provenance_stamps_monotonic_sequence() {
    let mem = test_mem();
    let ep = mem
        .store_episode("seq-source", "engineer-loop", None)
        .unwrap();

    mem.store_fact_with_provenance(
        "ordering",
        "older fact",
        0.7,
        "distill:older",
        None,
        None,
        std::slice::from_ref(&ep),
    )
    .unwrap();
    mem.store_fact_with_provenance(
        "ordering",
        "newer fact",
        0.7,
        "distill:newer",
        None,
        None,
        std::slice::from_ref(&ep),
    )
    .unwrap();

    let hits = mem.search_facts("ordering", 10, 0.0).unwrap();
    let newest = hits
        .iter()
        .max_by(|a, b| a.node_id.cmp(&b.node_id))
        .expect("at least one ordering fact");

    assert_eq!(
        newest.content,
        "newer fact",
        "the lexicographically-largest node_id must be the most recently \
         stored fact (monotonic seq stamp must be applied on the provenance \
         path too); hits: {:?}",
        hits.iter()
            .map(|f| (&f.node_id, &f.content))
            .collect::<Vec<_>>()
    );
}

/// Metadata supplied by the caller must be preserved alongside the
/// adapter-injected sequence stamp (the provenance write must not clobber
/// caller metadata when it folds in `FACT_SEQ_META_KEY`).
#[test]
fn store_fact_with_provenance_preserves_caller_metadata() {
    let mem = test_mem();
    let ep = mem
        .store_episode("meta-source", "engineer-loop", None)
        .unwrap();

    let mut metadata: HashMap<String, serde_json::Value> = HashMap::new();
    metadata.insert("origin".to_string(), serde_json::Value::from("distiller"));

    let fact_id = mem
        .store_fact_with_provenance(
            "bug-pattern",
            "off-by-one in batch loop",
            0.7,
            "distill:meta",
            None,
            Some(&metadata),
            std::slice::from_ref(&ep),
        )
        .expect("store_fact_with_provenance with metadata");

    // Edge still recorded despite extra metadata.
    let episodes = mem.episodes_for_fact(&fact_id).unwrap();
    assert!(
        episodes.contains(&ep),
        "provenance edge must survive caller-supplied metadata: {episodes:?}"
    );
}

/// Procedure provenance (issue #2325, acceptance #2): a procedure stored
/// with provenance must be stored + recallable, and the call must remain
/// the idempotent upsert-that-reinforces that plain `store_procedure` is
/// (Simard's procedural-idempotency contract, #2298) — storing the same
/// named procedure twice keeps exactly one node and bumps usage_count.
#[test]
fn store_procedure_with_provenance_stores_and_is_idempotent() {
    let mem = test_mem();
    let ep = mem
        .store_episode("ran the merge ritual", "engineer-loop", None)
        .unwrap();

    let steps = vec!["gh pr create".to_string(), "enable automerge".to_string()];
    let prereqs = vec!["clean working tree".to_string()];

    mem.store_procedure_with_provenance(
        "pr-merge:from-episode",
        &steps,
        &prereqs,
        std::slice::from_ref(&ep),
    )
    .expect("first store_procedure_with_provenance");
    // Second identical store must NOT create a second node — idempotent
    // upsert, exactly like base store_procedure.
    mem.store_procedure_with_provenance(
        "pr-merge:from-episode",
        &steps,
        &prereqs,
        std::slice::from_ref(&ep),
    )
    .expect("second store_procedure_with_provenance");

    let hits = mem.recall_procedure("pr-merge:from-episode", 16).unwrap();
    let exact: Vec<_> = hits
        .iter()
        .filter(|p| p.name == "pr-merge:from-episode")
        .collect();
    assert_eq!(
        exact.len(),
        1,
        "storing the same named procedure twice must keep exactly one node; \
         got {} matches",
        exact.len()
    );
    assert!(
        exact[0].usage_count >= 1,
        "the duplicate store must reinforce usage_count (>= 1); got {}",
        exact[0].usage_count
    );
    assert!(
        mem.procedure_exists("pr-merge:from-episode").unwrap(),
        "the provenance-stored procedure must exist by exact name"
    );
}
