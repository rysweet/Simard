//! Live integration tests for cognitive memory.
//!
//! These tests exercise the native LadybugDB backend directly.

use std::path::PathBuf;

use simard::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};

fn test_state_root() -> PathBuf {
    std::env::temp_dir().join(format!("simard-live-test-{}", uuid::Uuid::now_v7()))
}

#[test]
#[ignore] // Requires LadybugDB — run with: cargo test -- --ignored
fn live_memory_bridge_stores_and_retrieves_fact() {
    let state_root = test_state_root();
    let bridge = NativeCognitiveMemory::open(&state_root).expect("native memory should open");

    // Store a fact
    let fact_id = bridge
        .store_fact(
            "rust-testing",
            "cargo test runs all tests",
            0.95,
            &["rust".to_string()],
            "",
        )
        .expect("store_fact should succeed");
    assert!(!fact_id.is_empty(), "fact_id should be non-empty");

    // Search for it
    let facts = bridge
        .search_facts("rust testing", 10, 0.0)
        .expect("search_facts should succeed");
    assert!(!facts.is_empty(), "should find the fact we just stored");
    assert_eq!(facts[0].concept, "rust-testing");
    assert!(facts[0].content.contains("cargo test"));

    // Check statistics
    let stats = bridge
        .get_statistics()
        .expect("get_statistics should succeed");
    assert!(
        stats.semantic_count >= 1,
        "should have at least 1 semantic fact"
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&state_root);
}

#[test]
#[ignore] // Requires LadybugDB — run with: cargo test -- --ignored
fn live_memory_bridge_working_memory_lifecycle() {
    let state_root = test_state_root();
    let bridge = NativeCognitiveMemory::open(&state_root).expect("native memory should open");

    // Push a working memory slot
    let slot_id = bridge
        .push_working("goal", "fix the auth bug", "session-001", 1.0)
        .expect("push_working");
    assert!(!slot_id.is_empty());

    // Get working memory
    let slots = bridge.get_working("session-001").expect("get_working");
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].slot_type, "goal");
    assert!(slots[0].content.contains("auth bug"));

    // Clear working memory
    let cleared = bridge.clear_working("session-001").expect("clear_working");
    assert!(cleared >= 1);

    // Verify cleared
    let after = bridge
        .get_working("session-001")
        .expect("get_working after clear");
    assert!(after.is_empty());

    let _ = std::fs::remove_dir_all(&state_root);
}

#[test]
#[ignore] // Requires LadybugDB — run with: cargo test -- --ignored
fn live_memory_bridge_episode_and_consolidation() {
    let state_root = test_state_root();
    let bridge = NativeCognitiveMemory::open(&state_root).expect("native memory should open");

    // Store several episodes
    for i in 0..3 {
        bridge
            .store_episode(
                &format!("Session {i}: worked on feature X"),
                "session",
                None,
            )
            .expect("store_episode");
    }

    let stats = bridge.get_statistics().expect("stats");
    assert!(stats.episodic_count >= 3);

    // Consolidation with batch_size > available episodes returns None
    let consolidated = bridge.consolidate_episodes(10).expect("consolidate");
    assert!(
        consolidated.is_none(),
        "not enough episodes to consolidate with batch_size=10"
    );

    let _ = std::fs::remove_dir_all(&state_root);
}

#[test]
#[ignore] // Requires LadybugDB — run with: cargo test -- --ignored
fn live_memory_bridge_procedure_store_and_recall() {
    let state_root = test_state_root();
    let bridge = NativeCognitiveMemory::open(&state_root).expect("native memory should open");

    bridge
        .store_procedure(
            "fix-and-verify",
            &[
                "read file".to_string(),
                "edit".to_string(),
                "cargo test".to_string(),
                "commit".to_string(),
            ],
            &["git repo".to_string()],
        )
        .expect("store_procedure");

    let procs = bridge
        .recall_procedure("how to fix a bug", 5)
        .expect("recall_procedure");
    assert!(!procs.is_empty(), "should recall the procedure");
    assert_eq!(procs[0].name, "fix-and-verify");

    let _ = std::fs::remove_dir_all(&state_root);
}
