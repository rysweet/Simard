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
            "memory.get_working" => Ok(json!({"slots": []})),
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
    // get_working + store_episode + consolidate_episodes = 3
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

// ───────────────────────────────────────────────────────────────────────────
// G10 (issue #1604): persistence_memory_operations must propagate snapshot
// save failures rather than silently swallowing them via `eprintln!`.
// ───────────────────────────────────────────────────────────────────────────

/// Build a counting bridge that satisfies every call needed by the
/// persistence phase *and* the snapshot save (search_facts + recall_procedure).
fn persistence_capable_bridge() -> CognitiveMemoryBridge {
    let transport = InMemoryBridgeTransport::new("snapshot-fail", |method, _params| match method {
        "memory.consolidate_episodes" => Ok(json!({"id": null})),
        "memory.clear_working" => Ok(json!({"count": 0})),
        "memory.prune_expired_sensory" => Ok(json!({"count": 0})),
        "memory.store_episode" => Ok(json!({"id": "epi_1"})),
        "memory.search_facts" => Ok(json!({"facts": []})),
        "memory.recall_procedure" => Ok(json!({"procedures": []})),
        _ => Err(crate::bridge::BridgeErrorPayload {
            code: -32601,
            message: format!("unknown: {method}"),
        }),
    });
    CognitiveMemoryBridge::new(Box::new(transport))
}

#[test]
fn persistence_propagates_snapshot_save_error_when_dir_is_a_file() {
    // The snapshot-save path resolves to `<dir>/<agent>-<epoch>.json`.
    // If `dir` is actually a regular file, `std::fs::write` returns
    // `ENOTDIR`.  The fix for G10 (issue #1604) propagates that error
    // instead of swallowing it via `eprintln!`.
    let bridge = persistence_capable_bridge();
    let tmp_file = tempfile::NamedTempFile::new().expect("create tmp file");
    let dir_that_is_a_file = tmp_file.path().to_path_buf();

    let err = persistence_memory_operations_with_snapshot_dir(
        &test_session_id(),
        &bridge,
        Some(&dir_that_is_a_file),
    )
    .expect_err("snapshot save into a non-directory must propagate as Err");

    let msg = format!("{err}");
    assert!(
        msg.contains("memory-snapshot")
            || msg.contains("memory_snapshot")
            || msg.to_lowercase().contains("not a directory")
            || msg.to_lowercase().contains("notadirectory"),
        "expected error to mention snapshot/IO failure, got: {msg}",
    );
}

#[test]
fn persistence_with_valid_override_dir_writes_snapshot_and_returns_ok() {
    // Sanity check: the override mechanism still writes a snapshot when
    // pointed at a real directory, so the G10 propagation path does not
    // regress the happy case.
    let bridge = persistence_capable_bridge();
    let tmp_dir = tempfile::tempdir().expect("create tmp dir");

    persistence_memory_operations_with_snapshot_dir(
        &test_session_id(),
        &bridge,
        Some(tmp_dir.path()),
    )
    .expect("happy-path snapshot save must succeed");

    let entries: Vec<_> = std::fs::read_dir(tmp_dir.path())
        .expect("read snapshot dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one snapshot file, found {}",
        entries.len()
    );
}

// ───────────────────────────────────────────────────────────────────────────
// G11 (issue #1604): prune_snapshots must remain non-fatal AND switch its
// telemetry from `eprintln!` to `tracing::warn!`.  We cannot intercept the
// `tracing` event in a unit test without pulling in a subscriber, so the
// behavioural assertion focuses on the contract: pruning still removes the
// oldest entries, leaves the newest `keep` files intact, and never panics
// when individual deletions fail (e.g. read-only files).
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn prune_snapshots_keeps_newest_and_deletes_oldest() {
    let tmp_dir = tempfile::tempdir().expect("create tmp dir");
    // Create five snapshot-like files with embedded epochs so lexicographic
    // sort matches chronological order.
    for epoch in 1_000_000..1_000_005u64 {
        let p = tmp_dir.path().join(format!("agent-{epoch}.json"));
        std::fs::write(&p, b"{}").expect("write tmp snapshot");
    }
    super::prune_snapshots(tmp_dir.path(), 2);
    let mut remaining: Vec<String> = std::fs::read_dir(tmp_dir.path())
        .expect("read tmp dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    remaining.sort();
    assert_eq!(
        remaining,
        vec![
            "agent-1000003.json".to_string(),
            "agent-1000004.json".to_string()
        ],
        "prune_snapshots must keep the two newest entries and delete the rest",
    );
}

#[test]
fn prune_snapshots_does_not_panic_when_dir_missing() {
    // read_dir failure path — the function must log via tracing::warn!
    // (no eprintln, no panic, no propagated error).
    let missing = std::path::Path::new("/nonexistent/simard/prune-target");
    super::prune_snapshots(missing, 1); // must not panic
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
