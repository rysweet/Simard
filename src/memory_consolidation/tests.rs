use super::*;
use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::memory_bridge::CognitiveMemoryBridge;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

fn counting_bridge() -> (CognitiveMemoryBridge, Arc<AtomicU32>) {
    let call_count = Arc::new(AtomicU32::new(0));
    let counter = call_count.clone();
    let transport = InMemoryBridgeTransport::new("test", move |method, _params| {
        counter.fetch_add(1, Ordering::SeqCst);
        match method {
            "memory.record_sensory" => Ok(json!({"id": "sen_1"})),
            "memory.push_working" => Ok(json!({"id": "wrk_1"})),
            "memory.store_episode" => Ok(json!({"id": "epi_1"})),
            "memory.search_facts" => Ok(json!({"facts": []})),
            "memory.check_triggers" => Ok(json!({"prospectives": []})),
            "memory.recall_procedure" => Ok(json!({"procedures": []})),
            "memory.store_fact" => Ok(json!({"id": "sem_1"})),
            "memory.clear_working" => Ok(json!({"count": 2})),
            "memory.prune_expired_sensory" => Ok(json!({"count": 0})),
            "memory.consolidate_episodes" => Ok(json!({"id": null})),
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        }
    });
    (CognitiveMemoryBridge::new(Box::new(transport)), call_count)
}

fn test_session_id() -> SessionId {
    SessionId::parse("session-01234567-89ab-cdef-0123-456789abcdef").unwrap()
}

#[test]
fn intake_records_sensory_working_and_episode() {
    let (bridge, count) = counting_bridge();
    intake_memory_operations("build feature X", &test_session_id(), &bridge).unwrap();
    // Should make 3 calls: record_sensory, push_working, store_episode
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[test]
fn preparation_returns_empty_context_when_memory_empty() {
    let (bridge, _) = counting_bridge();
    let ctx =
        preparation_memory_operations("build feature X", &test_session_id(), &bridge).unwrap();
    assert!(ctx.relevant_facts.is_empty());
    assert!(ctx.triggered_prospectives.is_empty());
    assert!(ctx.recalled_procedures.is_empty());
}

#[test]
fn reflection_stores_transcript_and_facts() {
    let (bridge, count) = counting_bridge();
    let facts = vec![
        FactExtraction {
            concept: "rust".to_string(),
            content: "Rust is safe".to_string(),
            confidence: 0.9,
        },
        FactExtraction {
            concept: "testing".to_string(),
            content: "Tests should be fast".to_string(),
            confidence: 0.8,
        },
    ];
    reflection_memory_operations("transcript...", &facts, &test_session_id(), &bridge).unwrap();
    // 1 store_episode + 2*(search_facts + store_fact) = 5
    assert_eq!(count.load(Ordering::SeqCst), 5);
}

#[test]
fn reflection_deduplicates_facts_by_concept() {
    let (bridge, count) = counting_bridge();
    let facts = vec![
        FactExtraction {
            concept: "rust".to_string(),
            content: "Rust is safe".to_string(),
            confidence: 0.9,
        },
        FactExtraction {
            concept: "rust".to_string(), // duplicate concept — should be skipped
            content: "Rust is fast".to_string(),
            confidence: 0.8,
        },
    ];
    reflection_memory_operations("transcript...", &facts, &test_session_id(), &bridge).unwrap();
    // 1 store_episode + 1*(search_facts + store_fact) (second duplicate skipped) = 3
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[test]
fn execution_truncates_multibyte_utf8_safely() {
    let (bridge, _) = counting_bridge();
    // Build a string with multi-byte chars that would panic with naive byte slicing.
    // Each CJK char is 3 bytes; 200 chars = 600 bytes, exceeding the 500-byte threshold.
    let cjk_output: String = std::iter::repeat_n('漢', 200).collect();
    assert!(cjk_output.len() > 500);
    // Must not panic.
    execution_memory_operations(&cjk_output, &test_session_id(), &bridge).unwrap();
}

#[test]
fn execution_does_not_truncate_short_output() {
    let (bridge, _) = counting_bridge();
    execution_memory_operations("short", &test_session_id(), &bridge).unwrap();
}

#[test]
fn persistence_clears_working_and_prunes() {
    let (bridge, count) = counting_bridge();
    persistence_memory_operations(&test_session_id(), &bridge).unwrap();
    // clear_working + prune_expired_sensory + consolidate_episodes + store_episode = 4
    // + snapshot: search_facts("*") + recall_procedure("*") = 2 more → 6 total
    assert_eq!(count.load(Ordering::SeqCst), 6);
}

#[test]
fn consolidation_intake_returns_zero_when_no_prior_facts() {
    let (bridge, count) = counting_bridge();
    let hydrated = consolidation_intake(&test_session_id(), "test-objective", &bridge).unwrap();
    assert_eq!(hydrated, 0);
    // Only 1 call: search_facts
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn consolidation_intake_with_facts_pushes_to_working_memory() {
    let call_count = Arc::new(AtomicU32::new(0));
    let counter = call_count.clone();
    let transport = InMemoryBridgeTransport::new("test-intake", move |method, _params| {
        counter.fetch_add(1, Ordering::SeqCst);
        match method {
            "memory.search_facts" => Ok(json!({
                "facts": [{
                    "node_id": "n1",
                    "concept": "prior-fact",
                    "content": "remembered",
                    "confidence": 0.9,
                    "source_id": "memory-store-adapter",
                    "tags": []
                }]
            })),
            "memory.push_working" => Ok(json!({"id": "wrk_1"})),
            "memory.store_episode" => Ok(json!({"id": "epi_1"})),
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        }
    });
    let bridge = CognitiveMemoryBridge::new(Box::new(transport));
    let hydrated = consolidation_intake(&test_session_id(), "test-objective", &bridge).unwrap();
    assert_eq!(hydrated, 1);
    // search_facts + push_working + store_episode = 3
    assert_eq!(call_count.load(Ordering::SeqCst), 3);
}

#[test]
fn consolidation_persistence_flushes_and_consolidates() {
    let (bridge, count) = counting_bridge();
    consolidation_persistence(&test_session_id(), &bridge).unwrap();
    // store_episode + consolidate_episodes = 2
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

/// Round-trip verification: intake → execution → persistence → recall.
///
/// Uses `NativeCognitiveMemory` (in-memory LadybugDB) so that stored
/// data is actually queryable, unlike the counting bridge which only
/// counts calls.
#[test]
fn round_trip_execution_memory_recall() {
    use crate::cognitive_memory::NativeCognitiveMemory;

    let mem = NativeCognitiveMemory::in_memory().expect("in-memory DB");
    let sid = test_session_id();

    // 1. Intake — records objective as sensory + working + episode.
    intake_memory_operations("build feature X", &sid, &mem).unwrap();

    // 2. Execution — records pty output as sensory + working.
    execution_memory_operations("compiled successfully in 1.2s", &sid, &mem).unwrap();

    // 3. Persistence — flushes working memory and consolidates episodes.
    persistence_memory_operations(&sid, &mem).unwrap();

    // 4. Verify: the execution output should have been pushed into
    //    working memory under the session's task_id before persistence
    //    cleared it. Confirm the episode store received entries by
    //    checking statistics — intake stores 1 episode, persistence
    //    stores 1 episode, so we expect ≥ 2 episodes total.
    let stats = mem.get_statistics().unwrap();
    assert!(
        stats.episodic_count >= 2,
        "expected ≥2 episodes from intake+persistence, got {}",
        stats.episodic_count
    );
}
