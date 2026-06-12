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
// Issue #2207 Finding 1: preparation_memory_operations must dedup goal facts
// by slug (latest-per-slug) so that historical revisions of a goal don't
// crowd out current goals, and must use a limit high enough (256) that
// status churn doesn't cause current goals to fall off the result set.
// ───────────────────────────────────────────────────────────────────────────

/// Build a bridge whose `search_facts("goal-store:record", ...)` returns
/// multiple revisions for the same goal slug. This simulates the append-only
/// fact store returning historical rows alongside the current version.
fn goal_dedup_bridge() -> CognitiveMemoryBridge {
    use crate::goals::GoalRecord;
    let transport = InMemoryBridgeTransport::new("goal-dedup", move |method, params| {
        match method {
            "memory.search_facts" => {
                // Inspect the query parameter to route goal vs objective searches.
                let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
                if query == "goal-store:record" {
                    // Two revisions of slug "alpha" (node_id n2 > n1) plus
                    // one entry for slug "beta".
                    let alpha_v1 = GoalRecord {
                        slug: "alpha".to_string(),
                        title: "Alpha v1".to_string(),
                        rationale: "old rationale".to_string(),
                        status: crate::goals::GoalStatus::Proposed,
                        priority: 3,
                        owner_identity: "test".to_string(),
                        source_session_id: crate::session::SessionId::parse(
                            "session-01234567-89ab-cdef-0123-456789abcdef",
                        )
                        .unwrap(),
                        updated_in: crate::session::SessionPhase::Persistence,
                    };
                    let alpha_v2 = GoalRecord {
                        slug: "alpha".to_string(),
                        title: "Alpha v2".to_string(),
                        rationale: "new rationale".to_string(),
                        status: crate::goals::GoalStatus::Active,
                        priority: 1,
                        owner_identity: "test".to_string(),
                        source_session_id: crate::session::SessionId::parse(
                            "session-01234567-89ab-cdef-0123-456789abcdef",
                        )
                        .unwrap(),
                        updated_in: crate::session::SessionPhase::Persistence,
                    };
                    let beta = GoalRecord {
                        slug: "beta".to_string(),
                        title: "Beta".to_string(),
                        rationale: "beta rationale".to_string(),
                        status: crate::goals::GoalStatus::Active,
                        priority: 2,
                        owner_identity: "test".to_string(),
                        source_session_id: crate::session::SessionId::parse(
                            "session-01234567-89ab-cdef-0123-456789abcdef",
                        )
                        .unwrap(),
                        updated_in: crate::session::SessionPhase::Persistence,
                    };
                    Ok(json!({
                        "facts": [
                            {
                                "node_id": "n_0001",
                                "concept": "goal-store:record",
                                "content": serde_json::to_string(&alpha_v1).unwrap(),
                                "confidence": 1.0,
                                "source_id": "goal-store",
                                "tags": ["goal-store"]
                            },
                            {
                                "node_id": "n_0002",
                                "concept": "goal-store:record",
                                "content": serde_json::to_string(&alpha_v2).unwrap(),
                                "confidence": 1.0,
                                "source_id": "goal-store",
                                "tags": ["goal-store"]
                            },
                            {
                                "node_id": "n_0003",
                                "concept": "goal-store:record",
                                "content": serde_json::to_string(&beta).unwrap(),
                                "confidence": 1.0,
                                "source_id": "goal-store",
                                "tags": ["goal-store"]
                            }
                        ]
                    }))
                } else {
                    // Objective search returns nothing.
                    Ok(json!({"facts": []}))
                }
            }
            "memory.check_triggers" => Ok(json!({"prospectives": []})),
            "memory.recall_procedure" => Ok(json!({"procedures": []})),
            "memory.push_working" => Ok(json!({"id": "wrk_1"})),
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        }
    });
    CognitiveMemoryBridge::new(Box::new(transport))
}

#[test]
fn preparation_deduplicates_goal_facts_by_slug_keeping_latest() {
    // The bridge returns 3 goal facts: 2 revisions of "alpha" and 1 "beta".
    // After dedup, only the latest "alpha" (node_id n_0002) and "beta" should
    // appear in relevant_facts.
    let bridge = goal_dedup_bridge();
    let ctx =
        preparation_memory_operations("unrelated objective", &test_session_id(), &bridge).unwrap();

    // Collect goal-record facts by parsing their content.
    let goal_facts: Vec<crate::goals::GoalRecord> = ctx
        .relevant_facts
        .iter()
        .filter(|f| f.concept == "goal-store:record")
        .filter_map(|f| serde_json::from_str(&f.content).ok())
        .collect();

    // Must have exactly 2 unique slugs (alpha latest + beta).
    assert_eq!(
        goal_facts.len(),
        2,
        "expected 2 goal facts after dedup (alpha latest + beta), got {}",
        goal_facts.len()
    );

    // The "alpha" fact must be the v2 revision (title "Alpha v2", priority 1).
    let alpha = goal_facts.iter().find(|r| r.slug == "alpha");
    assert!(alpha.is_some(), "alpha goal must be present after dedup");
    assert_eq!(
        alpha.unwrap().title,
        "Alpha v2",
        "dedup must keep the latest revision (highest node_id)"
    );
    assert_eq!(alpha.unwrap().priority, 1);

    // Beta must be present unchanged.
    assert!(
        goal_facts.iter().any(|r| r.slug == "beta"),
        "beta goal must be present after dedup"
    );
}

#[test]
fn preparation_does_not_include_unparseable_goal_facts() {
    // A bridge that returns one valid goal fact and one with malformed JSON.
    let transport = InMemoryBridgeTransport::new("bad-json", move |method, params| match method {
        "memory.search_facts" => {
            let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
            if query == "goal-store:record" {
                Ok(json!({
                    "facts": [
                        {
                            "node_id": "n_good",
                            "concept": "goal-store:record",
                            "content": "{\"slug\":\"good\",\"title\":\"Good\",\"rationale\":\"r\",\"status\":\"active\",\"priority\":1,\"owner_identity\":\"o\",\"source_session_id\":\"session-01234567-89ab-cdef-0123-456789abcdef\",\"updated_in\":\"persistence\"}",
                            "confidence": 1.0,
                            "source_id": "goal-store",
                            "tags": ["goal-store"]
                        },
                        {
                            "node_id": "n_bad",
                            "concept": "goal-store:record",
                            "content": "NOT VALID JSON {{{",
                            "confidence": 1.0,
                            "source_id": "goal-store",
                            "tags": ["goal-store"]
                        }
                    ]
                }))
            } else {
                Ok(json!({"facts": []}))
            }
        }
        "memory.check_triggers" => Ok(json!({"prospectives": []})),
        "memory.recall_procedure" => Ok(json!({"procedures": []})),
        "memory.push_working" => Ok(json!({"id": "wrk_1"})),
        _ => Err(crate::bridge::BridgeErrorPayload {
            code: -32601,
            message: format!("unknown: {method}"),
        }),
    });
    let bridge = CognitiveMemoryBridge::new(Box::new(transport));
    let ctx =
        preparation_memory_operations("unrelated objective", &test_session_id(), &bridge).unwrap();

    // The unparseable fact should be silently skipped (match+continue).
    // Only the valid "good" goal fact should survive dedup.
    let goal_facts: Vec<&crate::memory_cognitive::CognitiveFact> = ctx
        .relevant_facts
        .iter()
        .filter(|f| f.concept == "goal-store:record")
        .collect();

    assert_eq!(
        goal_facts.len(),
        1,
        "unparseable goal facts must be skipped; expected 1, got {}",
        goal_facts.len()
    );
    assert_eq!(goal_facts[0].node_id, "n_good");
}

/// Verify that goal facts use the same limit constant as the goal store's
/// `list_via_reader` (256), not the old hardcoded 20.
#[test]
fn preparation_uses_goal_store_list_limit_not_hardcoded_20() {
    use std::sync::{Arc, Mutex};

    let captured_limit = Arc::new(Mutex::new(0u32));
    let limit_capture = captured_limit.clone();

    let transport = InMemoryBridgeTransport::new("limit-check", move |method, params| {
        match method {
            "memory.search_facts" => {
                let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
                if query == "goal-store:record" {
                    // Capture the limit parameter.
                    if let Some(limit) = params.get("limit").and_then(|v| v.as_u64()) {
                        *limit_capture.lock().unwrap() = limit as u32;
                    }
                }
                Ok(json!({"facts": []}))
            }
            "memory.check_triggers" => Ok(json!({"prospectives": []})),
            "memory.recall_procedure" => Ok(json!({"procedures": []})),
            "memory.push_working" => Ok(json!({"id": "wrk_1"})),
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        }
    });
    let bridge = CognitiveMemoryBridge::new(Box::new(transport));
    preparation_memory_operations("check limit", &test_session_id(), &bridge).unwrap();

    let limit = *captured_limit.lock().unwrap();
    assert!(
        limit >= 256,
        "goal fact search must use limit >= 256 (GOAL_STORE_LIST_LIMIT), got {limit}"
    );
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

// ═══════════════════════════════════════════════════════════════════════════
// Issue #2270 Fix 1: preparation_memory_operations must split compound
// objectives on "; " and search each fragment independently, then dedup
// results by node_id and cap total at 10.
// ═══════════════════════════════════════════════════════════════════════════

/// Build a bridge that captures search_facts query strings and returns
/// per-fragment results with controllable node_ids for dedup testing.
fn compound_objective_bridge(
    captured_queries: Arc<std::sync::Mutex<Vec<String>>>,
    facts_per_query: std::collections::HashMap<String, Vec<serde_json::Value>>,
) -> CognitiveMemoryBridge {
    let transport =
        InMemoryBridgeTransport::new("compound-obj", move |method, params| match method {
            "memory.search_facts" => {
                let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
                captured_queries.lock().unwrap().push(query.to_string());
                let facts = facts_per_query.get(query).cloned().unwrap_or_default();
                Ok(json!({ "facts": facts }))
            }
            "memory.check_triggers" => Ok(json!({"prospectives": []})),
            "memory.recall_procedure" => Ok(json!({"procedures": []})),
            "memory.push_working" => Ok(json!({"id": "wrk_1"})),
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        });
    CognitiveMemoryBridge::new(Box::new(transport))
}

#[test]
fn preparation_splits_compound_objective_into_separate_searches() {
    let queries = Arc::new(std::sync::Mutex::new(Vec::new()));
    let bridge = compound_objective_bridge(queries.clone(), std::collections::HashMap::new());

    let _ctx = preparation_memory_operations(
        "fix auth bug; deploy to staging; update docs",
        &test_session_id(),
        &bridge,
    )
    .unwrap();

    let captured = queries.lock().unwrap();
    // The objective has 3 fragments separated by "; ".
    // After Fix 1: search_facts must be called once per fragment (3 times)
    // PLUS once for goal-store:record = 4 total search_facts calls.
    let objective_queries: Vec<&String> = captured
        .iter()
        .filter(|q| *q != "goal-store:record")
        .collect();
    assert_eq!(
        objective_queries.len(),
        3,
        "compound objective with 3 fragments must produce 3 search_facts calls, got {}: {:?}",
        objective_queries.len(),
        objective_queries
    );
    assert!(
        objective_queries.contains(&&"fix auth bug".to_string()),
        "must search for first fragment"
    );
    assert!(
        objective_queries.contains(&&"deploy to staging".to_string()),
        "must search for second fragment"
    );
    assert!(
        objective_queries.contains(&&"update docs".to_string()),
        "must search for third fragment"
    );
}

#[test]
fn preparation_single_fragment_objective_produces_one_search() {
    let queries = Arc::new(std::sync::Mutex::new(Vec::new()));
    let bridge = compound_objective_bridge(queries.clone(), std::collections::HashMap::new());

    let _ctx = preparation_memory_operations("fix auth bug", &test_session_id(), &bridge).unwrap();

    let captured = queries.lock().unwrap();
    let objective_queries: Vec<&String> = captured
        .iter()
        .filter(|q| *q != "goal-store:record")
        .collect();
    // Single fragment (no "; " delimiter) → exactly 1 search_facts call.
    assert_eq!(
        objective_queries.len(),
        1,
        "single-fragment objective must produce exactly 1 search_facts call, got {}",
        objective_queries.len()
    );
    assert_eq!(objective_queries[0], "fix auth bug");
}

#[test]
fn preparation_deduplicates_split_results_by_node_id() {
    let queries = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut facts_map = std::collections::HashMap::new();

    // Fragment "goal A" returns facts n1 and n2.
    facts_map.insert(
        "goal A".to_string(),
        vec![
            json!({"node_id": "n1", "concept": "c1", "content": "fact1", "confidence": 0.9, "source_id": "src", "tags": []}),
            json!({"node_id": "n2", "concept": "c2", "content": "fact2", "confidence": 0.8, "source_id": "src", "tags": []}),
        ],
    );
    // Fragment "goal B" returns facts n2 (duplicate!) and n3.
    facts_map.insert(
        "goal B".to_string(),
        vec![
            json!({"node_id": "n2", "concept": "c2", "content": "fact2", "confidence": 0.8, "source_id": "src", "tags": []}),
            json!({"node_id": "n3", "concept": "c3", "content": "fact3", "confidence": 0.7, "source_id": "src", "tags": []}),
        ],
    );

    let bridge = compound_objective_bridge(queries, facts_map);
    let ctx = preparation_memory_operations("goal A; goal B", &test_session_id(), &bridge).unwrap();

    // n1, n2, n3 — n2 appears in both fragments but should only appear once.
    let node_ids: Vec<&str> = ctx
        .relevant_facts
        .iter()
        .map(|f| f.node_id.as_str())
        .collect();

    let unique: std::collections::HashSet<&str> = node_ids.iter().copied().collect();
    assert_eq!(
        node_ids.len(),
        unique.len(),
        "relevant_facts must not contain duplicate node_ids; found duplicates in {:?}",
        node_ids
    );
    assert_eq!(
        unique.len(),
        3,
        "expected 3 unique facts (n1, n2, n3), got {}",
        unique.len()
    );
}

#[test]
fn preparation_caps_split_results_at_ten() {
    let queries = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut facts_map = std::collections::HashMap::new();

    // Fragment "goal A" returns 8 unique facts.
    let mut goal_a_facts = Vec::new();
    for i in 0..8 {
        goal_a_facts.push(json!({
            "node_id": format!("a{i}"), "concept": format!("ca{i}"),
            "content": format!("fact a{i}"), "confidence": 0.9,
            "source_id": "src", "tags": []
        }));
    }
    facts_map.insert("goal A".to_string(), goal_a_facts);

    // Fragment "goal B" returns 6 unique facts (no overlap with A).
    let mut goal_b_facts = Vec::new();
    for i in 0..6 {
        goal_b_facts.push(json!({
            "node_id": format!("b{i}"), "concept": format!("cb{i}"),
            "content": format!("fact b{i}"), "confidence": 0.8,
            "source_id": "src", "tags": []
        }));
    }
    facts_map.insert("goal B".to_string(), goal_b_facts);

    let bridge = compound_objective_bridge(queries, facts_map);
    let ctx = preparation_memory_operations("goal A; goal B", &test_session_id(), &bridge).unwrap();

    // 8 + 6 = 14 unique facts, but total must be capped at 10.
    assert!(
        ctx.relevant_facts.len() <= 10,
        "relevant_facts must be capped at 10, got {}",
        ctx.relevant_facts.len()
    );
}

#[test]
fn preparation_skips_empty_fragments_from_splitting() {
    let queries = Arc::new(std::sync::Mutex::new(Vec::new()));
    let bridge = compound_objective_bridge(queries.clone(), std::collections::HashMap::new());

    // Trailing "; " produces an empty fragment that must be skipped.
    let _ctx =
        preparation_memory_operations("goal A; ; goal B", &test_session_id(), &bridge).unwrap();

    let captured = queries.lock().unwrap();
    let objective_queries: Vec<&String> = captured
        .iter()
        .filter(|q| *q != "goal-store:record")
        .collect();

    // Only non-empty fragments should be searched.
    for q in &objective_queries {
        assert!(
            !q.trim().is_empty(),
            "must not search with empty/whitespace-only query, found: {:?}",
            q
        );
    }
    assert_eq!(
        objective_queries.len(),
        2,
        "expected 2 non-empty fragments (goal A, goal B), got {}: {:?}",
        objective_queries.len(),
        objective_queries
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Issue #2270 Fix 1 integration: verify with NativeCognitiveMemory that
// compound objectives actually find facts that single-goal queries match.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn preparation_compound_objective_finds_per_goal_facts_native() {
    use crate::cognitive_memory::NativeCognitiveMemory;

    let mem = NativeCognitiveMemory::in_memory().expect("in-memory DB");
    let sid = test_session_id();

    // Store facts that match individual goal fragments but NOT the joined string.
    mem.store_fact("auth", "JWT tokens expire after 1 hour", 0.9, &[], "src")
        .unwrap();
    mem.store_fact(
        "deploy",
        "Staging uses blue-green deploys",
        0.85,
        &[],
        "src",
    )
    .unwrap();

    // A compound objective "auth; deploy" — the joined string "auth; deploy" would
    // NOT match via CONTAINS because no fact contains that exact substring.
    // After Fix 1: splitting on "; " must find both facts.
    let ctx = preparation_memory_operations("auth; deploy", &sid, &mem).unwrap();

    let concepts: Vec<&str> = ctx
        .relevant_facts
        .iter()
        .map(|f| f.concept.as_str())
        .collect();

    assert!(
        concepts.contains(&"auth"),
        "compound search must find 'auth' fact; got {:?}",
        concepts
    );
    assert!(
        concepts.contains(&"deploy"),
        "compound search must find 'deploy' fact; got {:?}",
        concepts
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Issue #2270 Fix 3: prune_old_backups integration test — verify the
// function from cognitive_memory::backup keeps the specified number and
// deletes the rest, including paired WAL files.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn prune_old_backups_keeps_newest_and_removes_oldest() {
    use crate::cognitive_memory::NativeCognitiveMemory;

    let tmp_dir = tempfile::tempdir().expect("create tmp dir");
    let state_root = tmp_dir.path();
    let backup_dir = state_root.join("backups");
    std::fs::create_dir_all(&backup_dir).expect("create backups dir");

    // Create 25 fake backup files with epoch-based names.
    for epoch in 1_000_000..1_000_025u64 {
        let main_path = backup_dir.join(format!("cognitive_memory.ladybug.{epoch}"));
        std::fs::write(&main_path, b"db-data").expect("write backup");
        // Create a paired WAL file for some.
        if epoch % 3 == 0 {
            let wal_path = backup_dir.join(format!("cognitive_memory.ladybug.wal.{epoch}"));
            std::fs::write(&wal_path, b"wal-data").expect("write wal");
        }
    }

    let outcome = NativeCognitiveMemory::prune_old_backups(state_root, 20);

    // 25 - 20 = 5 main backups should be removed. Some have paired WALs.
    assert!(
        outcome.removed > 0,
        "prune_old_backups must remove some files when count exceeds keep"
    );
    assert!(
        outcome.failed.is_empty(),
        "expected no failures in prune, got {:?}",
        outcome
            .failed
            .iter()
            .map(|(p, e)| format!("{}: {e}", p.display()))
            .collect::<Vec<_>>()
    );

    // Count remaining main backup files.
    let remaining_main: Vec<_> = std::fs::read_dir(&backup_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let s = name.to_string_lossy();
            s.starts_with("cognitive_memory.ladybug.") && !s.contains("wal")
        })
        .collect();

    assert_eq!(
        remaining_main.len(),
        20,
        "must keep exactly 20 most recent backups, got {}",
        remaining_main.len()
    );
}

#[test]
fn prune_old_backups_handles_missing_backup_dir() {
    let tmp_dir = tempfile::tempdir().expect("create tmp dir");
    // state_root exists but backups/ subdirectory does not.
    let outcome =
        crate::cognitive_memory::NativeCognitiveMemory::prune_old_backups(tmp_dir.path(), 20);
    assert_eq!(outcome.removed, 0);
    assert!(outcome.failed.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════
// Issue #2270 Fix 4: search_facts diagnostic logging. We verify that the
// search_facts function still returns correct results (the logging itself
// is tracing::debug! which is a no-op without a subscriber in tests).
// The critical contract: logging must NOT alter search behavior.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn search_facts_with_logging_returns_correct_results() {
    use crate::cognitive_memory::NativeCognitiveMemory;

    let mem = NativeCognitiveMemory::in_memory().expect("in-memory DB");
    mem.store_fact("rust-perf", "Zero-cost abstractions", 0.95, &[], "test")
        .unwrap();

    // Normal query — must still work after logging is added.
    let results = mem.search_facts("rust-perf", 10, 0.0).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].concept, "rust-perf");

    // Wildcard query — must still work.
    let all = mem.search_facts("*", 100, 0.0).unwrap();
    assert_eq!(all.len(), 1);

    // Empty-result query — must still work (no panic from logging empty results).
    let empty = mem.search_facts("nonexistent-query", 10, 0.0).unwrap();
    assert!(empty.is_empty());
}
