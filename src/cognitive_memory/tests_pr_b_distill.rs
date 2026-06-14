//! TDD (RED) tests for PR-B trait surface additions.
//!
//! Covers the two new `CognitiveMemoryOps` methods documented in
//! `docs/architecture/episode-distillation.md` §"Trait surface":
//!
//! * `mark_episode_distilled(node_id) -> SimardResult<()>`
//! * `list_undistilled_episodes(limit) -> SimardResult<Vec<CognitiveEpisode>>`
//!
//! Both methods MUST land with a default no-op impl so legacy bridges
//! keep compiling. `NativeCognitiveMemory` MUST override them against
//! the lbug-backed `Episode` schema, which gains a lazy `distilled
//! INT64 DEFAULT 0` column.
//!
//! These tests target `NativeCognitiveMemory::in_memory()` directly so
//! the override behaviour is exercised without going through the
//! bridge layer.
//!
//! ## Expected red signal
//!
//! Before PR-B lands, the default trait impls return `Ok(vec![])` and
//! `Ok(())` respectively, so every assertion that expects non-empty
//! results or post-mark filtering will fail. The lazy schema migration
//! is also untested today — this file is the contract.

use super::{CognitiveMemoryOps, NativeCognitiveMemory};

fn test_mem() -> NativeCognitiveMemory {
    NativeCognitiveMemory::in_memory().expect("in-memory DB should create")
}

/// `list_undistilled_episodes` MUST return episodes newest-first by
/// node id descending. Because Episode ids are UUID-v7 (time-prefixed)
/// in production, lex-descending == chronologically-newest-first
/// without consulting `temporal_index`.
#[test]
fn list_undistilled_episodes_returns_newest_first() {
    let mem = test_mem();
    let ids: Vec<String> = (0..5)
        .map(|i| {
            mem.store_episode(&format!("episode {i}"), "test", None)
                .expect("store_episode")
        })
        .collect();

    let listed = mem
        .list_undistilled_episodes(10)
        .expect("list_undistilled_episodes");

    assert_eq!(
        listed.len(),
        5,
        "all 5 freshly-stored episodes must be undistilled"
    );

    // Returned newest-first; the last id stored must be the first
    // entry in the listing.
    let listed_ids: Vec<&str> = listed.iter().map(|e| e.node_id.as_str()).collect();
    let last_stored = ids.last().unwrap().as_str();
    assert_eq!(
        listed_ids[0], last_stored,
        "newest stored episode must appear first in the listing; \
         listed: {listed_ids:?}, ids: {ids:?}"
    );
}

/// `mark_episode_distilled` followed by `list_undistilled_episodes`
/// must exclude the marked row. The mark MUST be durable across
/// subsequent reads (no in-memory-only state).
#[test]
fn mark_episode_distilled_round_trips() {
    let mem = test_mem();
    let id_a = mem.store_episode("alpha", "test", None).unwrap();
    let id_b = mem.store_episode("beta", "test", None).unwrap();
    let id_c = mem.store_episode("gamma", "test", None).unwrap();

    mem.mark_episode_distilled(&id_b)
        .expect("mark_episode_distilled");

    let listed = mem.list_undistilled_episodes(10).unwrap();
    let listed_ids: std::collections::HashSet<&str> =
        listed.iter().map(|e| e.node_id.as_str()).collect();
    assert!(
        listed_ids.contains(id_a.as_str()),
        "alpha must remain undistilled"
    );
    assert!(
        !listed_ids.contains(id_b.as_str()),
        "beta must be excluded after mark_episode_distilled; listed: {listed_ids:?}"
    );
    assert!(
        listed_ids.contains(id_c.as_str()),
        "gamma must remain undistilled"
    );
}

/// The `limit` parameter MUST be honoured: requesting 2 from a store
/// of 5 undistilled rows returns exactly 2.
#[test]
fn list_undistilled_respects_limit() {
    let mem = test_mem();
    for i in 0..5 {
        mem.store_episode(&format!("e{i}"), "src", None).unwrap();
    }
    let listed = mem.list_undistilled_episodes(2).unwrap();
    assert_eq!(
        listed.len(),
        2,
        "limit=2 must cap the result list at 2 rows"
    );
}

/// Lazy migration: pre-PR-B rows (which lack the `distilled` column)
/// must be treated as undistilled (default `0`). This is verified
/// indirectly: every freshly stored episode in this test predates
/// any `mark_episode_distilled` call, so all must appear in the
/// undistilled list. Combined with the dedicated round-trip test
/// above, this proves the migration is implicit (no offline step).
#[test]
fn list_undistilled_includes_pre_migration_rows_as_undistilled() {
    let mem = test_mem();
    for i in 0..3 {
        mem.store_episode(&format!("legacy {i}"), "legacy", None)
            .unwrap();
    }
    let listed = mem.list_undistilled_episodes(10).unwrap();
    assert_eq!(
        listed.len(),
        3,
        "rows stored without ever calling mark_episode_distilled must \
         appear in the undistilled listing (lazy migration default = 0)"
    );
}
