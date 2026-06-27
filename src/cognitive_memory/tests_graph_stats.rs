//! TDD tests for graph-edge / dedup stats (issue #2331).
//!
//! Pins the contract for the new [`CognitiveMemoryOps::graph_stats`] aggregate
//! against [`LibraryCognitiveMemory`]: the read side of
//! [`store_fact_with_provenance`](CognitiveMemoryOps::store_fact_with_provenance)
//! / [`store_procedure_with_provenance`](CognitiveMemoryOps::store_procedure_with_provenance)
//! / [`store_fact_with_caller_key`](CognitiveMemoryOps::store_fact_with_caller_key)
//! that powers the "edges / connections" section of `simard memory stats`.
//!
//! These target `LibraryCognitiveMemory::in_memory()` directly — the in-memory
//! `GraphStore` implements `query_neighbors`, so the edge round-trip is
//! exercised without the bridge/IPC layer. `graph_stats` is read-only.

use super::{CognitiveMemoryOps, LibraryCognitiveMemory};

fn test_mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory DB should create")
}

/// Core contract: a fact stored with provenance for an episode bumps the
/// aggregate `DERIVES_FROM` edge count and the fact-provenance coverage.
#[test]
fn graph_stats_counts_derives_from_after_provenance_link() {
    let mem = test_mem();

    let ep = mem
        .store_episode(
            "enabled auto-merge then waited for CI",
            "engineer-loop",
            None,
        )
        .expect("store_episode");

    mem.store_fact_with_provenance(
        "pr-pattern",
        "enable auto-merge before final review",
        0.7,
        &format!("distill:{ep}"),
        None,
        None,
        std::slice::from_ref(&ep),
    )
    .expect("store_fact_with_provenance");

    let stats = mem.graph_stats().expect("graph_stats");

    assert!(
        stats.derives_from_edges >= 1,
        "a provenance-linked fact must register a DERIVES_FROM edge; got {stats:?}"
    );
    assert!(
        stats.facts_with_provenance >= 1,
        "the linked fact must count toward facts_with_provenance; got {stats:?}"
    );
    assert!(
        stats.facts_total >= 1,
        "facts_total must include the stored fact; got {stats:?}"
    );
    assert!(
        stats.facts_with_provenance <= stats.facts_total,
        "coverage cannot exceed the total; got {stats:?}"
    );
}

/// Fan-out: a fact distilled from several episodes contributes one
/// `DERIVES_FROM` edge per source episode, but only one toward coverage.
#[test]
fn graph_stats_counts_one_edge_per_source_episode() {
    let mem = test_mem();
    let ep_a = mem.store_episode("alpha", "engineer-loop", None).unwrap();
    let ep_b = mem.store_episode("beta", "engineer-loop", None).unwrap();
    let ep_c = mem.store_episode("gamma", "engineer-loop", None).unwrap();

    mem.store_fact_with_provenance(
        "lesson",
        "three episodes, one lesson",
        0.7,
        "distill:multi",
        None,
        None,
        &[ep_a, ep_b, ep_c],
    )
    .unwrap();

    let stats = mem.graph_stats().unwrap();
    assert_eq!(
        stats.derives_from_edges, 3,
        "one DERIVES_FROM edge per source episode; got {stats:?}"
    );
    assert_eq!(
        stats.facts_with_provenance, 1,
        "a single fact, however many edges; got {stats:?}"
    );
}

/// Procedure provenance is summed independently of fact provenance.
#[test]
fn graph_stats_counts_procedure_derives_from() {
    let mem = test_mem();
    let ep = mem
        .store_episode("ran the merge ritual", "engineer-loop", None)
        .unwrap();

    mem.store_procedure_with_provenance(
        "pr-merge:from-episode",
        &["gh pr create".to_string()],
        &[],
        std::slice::from_ref(&ep),
    )
    .expect("store_procedure_with_provenance");

    let stats = mem.graph_stats().unwrap();
    assert!(
        stats.procedure_derives_from_edges >= 1,
        "a provenance-linked procedure must register a PROCEDURE_DERIVES_FROM \
         edge; got {stats:?}"
    );
}

/// A fresh store has no edges and no facts — every counter is zero.
#[test]
fn graph_stats_zero_on_empty_store() {
    let mem = test_mem();
    let stats = mem.graph_stats().unwrap();
    assert_eq!(stats.derives_from_edges, 0, "{stats:?}");
    assert_eq!(stats.procedure_derives_from_edges, 0, "{stats:?}");
    assert_eq!(stats.facts_with_provenance, 0, "{stats:?}");
    assert_eq!(stats.facts_total, 0, "{stats:?}");
    assert_eq!(stats.snapshot_facts_total, 0, "{stats:?}");
    assert_eq!(stats.distinct_snapshot_caller_keys, 0, "{stats:?}");
}

/// Negative control: plain `store_fact` (no provenance) draws no edges, so the
/// fact counts toward `facts_total` but not `facts_with_provenance`.
#[test]
fn graph_stats_plain_fact_has_no_provenance() {
    let mem = test_mem();
    mem.store_fact("rust", "a fact with no source", 0.7, &[], "manual")
        .unwrap();

    let stats = mem.graph_stats().unwrap();
    assert_eq!(stats.derives_from_edges, 0, "{stats:?}");
    assert_eq!(stats.facts_with_provenance, 0, "{stats:?}");
    assert!(stats.facts_total >= 1, "{stats:?}");
}

/// Snapshot dedup: repeated caller-key snapshot writes under one key collapse to
/// a single distinct caller key. A changed-content write supersedes the prior
/// revision (archived but still a node), so `snapshot_facts_total` reflects the
/// dedup volume while `distinct_snapshot_caller_keys` stays at one — exactly the
/// operator-visible dedup signal.
#[test]
fn graph_stats_snapshot_dedup_groups_by_caller_key() {
    let mem = test_mem();

    mem.store_fact_with_caller_key(
        "goal-board:snapshot",
        "goal-board:snapshot",
        "{\"rev\":1}",
        1.0,
        &["goal-board".to_string()],
        "goal-curator",
    )
    .unwrap();
    // Changed content under the same key -> supersede (new live + archived old).
    mem.store_fact_with_caller_key(
        "goal-board:snapshot",
        "goal-board:snapshot",
        "{\"rev\":2}",
        1.0,
        &["goal-board".to_string()],
        "goal-curator",
    )
    .unwrap();

    let stats = mem.graph_stats().unwrap();
    assert_eq!(
        stats.distinct_snapshot_caller_keys, 1,
        "both writes share one caller key; got {stats:?}"
    );
    assert_eq!(
        stats.snapshot_facts_total, 2,
        "the superseded rev:1 (archived, retained) and the live rev:2 must both \
         be counted, so the dedup volume is visible; got {stats:?}"
    );
    assert!(
        stats.snapshot_facts_total > stats.distinct_snapshot_caller_keys,
        "the operator-visible dedup signal is many revisions collapsing onto one \
         caller key (volume strictly above the distinct-key count); got {stats:?}"
    );
}
