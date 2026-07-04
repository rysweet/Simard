//! Issue #2550 (P0), Fix #4: the periodic snapshot export must capture ALL
//! cognitive-memory types (episodes and prospective/triggers), not just facts +
//! procedures — otherwise a restore silently drops them. This is exactly the
//! gap that made the incident unrecoverable: the on-disk backups
//! (`cognitive_snapshot.json`) held only facts + procedures, so episodes and
//! prospective memories were lost for good when the store was reset.
//!
//! No sleeps, no network: a `TempDir`-rooted persistent store so enumeration
//! behaves exactly like production.

use simard::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use simard::remote_transfer::export_full_memory_snapshot;

#[test]
fn full_snapshot_includes_every_memory_type_not_just_facts_and_procedures() {
    let tmp = tempfile::tempdir().unwrap();
    let mem = LibraryCognitiveMemory::open(tmp.path()).expect("open store");

    mem.store_fact(
        "rust",
        "Rust is a systems language",
        0.9,
        &["lang".to_string()],
        "issue-2550",
    )
    .expect("store_fact");
    mem.store_procedure("ooda:consolidate", &["distill episodes".to_string()], &[])
        .expect("store_procedure");
    mem.store_episode(
        "episode: ran cargo test; 0 failures (issue-2550)",
        "engineer-cycle",
        None,
    )
    .expect("store_episode");
    mem.store_prospective(
        "goal: ship the CLI",
        "trigger:ship-the-cli-2550",
        "Pursue goal",
        1,
    )
    .expect("store_prospective");

    let snapshot = export_full_memory_snapshot(&mem, "issue-2550").expect("export");
    let value = serde_json::to_value(&snapshot).expect("serialize snapshot");

    // Facts + procedures already work today (sanity: the seeding + export path
    // is sound before we assert the new behaviour).
    assert!(
        value["facts"].as_array().map_or(0, Vec::len) >= 1,
        "facts must be captured: {value}"
    );
    assert!(
        value["procedures"].as_array().map_or(0, Vec::len) >= 1,
        "procedures must be captured: {value}"
    );

    // Issue #2550: a *complete* snapshot must ALSO carry episodes and
    // prospective triggers so a restore round-trips the whole store.
    let episodes = value
        .get("episodes")
        .and_then(serde_json::Value::as_array)
        .expect("snapshot must contain an `episodes` array (issue #2550)");
    assert!(
        episodes
            .iter()
            .any(|e| e.to_string().contains("issue-2550")),
        "seeded episode must be captured in the snapshot: {value}"
    );

    let prospective = value
        .get("prospective")
        .and_then(serde_json::Value::as_array)
        .expect("snapshot must contain a `prospective` array (issue #2550)");
    assert!(
        prospective
            .iter()
            .any(|p| p.to_string().contains("ship-the-cli-2550")),
        "seeded prospective trigger must be captured in the snapshot: {value}"
    );
}
