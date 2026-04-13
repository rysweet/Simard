//! Integration tests verifying end-to-end session lifecycle triggers
//! memory consolidation phases.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::json;
use simard::bridge::BridgeErrorPayload;
use simard::bridge_subprocess::InMemoryBridgeTransport;
use simard::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};
use simard::gym_bridge::GymBridge;
use simard::knowledge_bridge::KnowledgeBridge;
use simard::memory_bridge::CognitiveMemoryBridge;
use simard::memory_consolidation::{
    FactExtraction, consolidation_intake, consolidation_persistence, intake_memory_operations,
    persistence_memory_operations, preparation_memory_operations, reflection_memory_operations,
};
use simard::ooda_loop::{OodaBridges, OodaConfig, OodaState, run_ooda_cycle};
use simard::session::SessionId;

/// Build a bridge whose transport counts calls per method category.
fn counting_bridge() -> (CognitiveMemoryBridge, Arc<AtomicU32>) {
    let call_count = Arc::new(AtomicU32::new(0));
    let counter = call_count.clone();
    let transport = InMemoryBridgeTransport::new("lifecycle-test", move |method, _params| {
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
            _ => Err(BridgeErrorPayload {
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

// ============================================================================
// Test 1: Full session lifecycle triggers all consolidation phases
// ============================================================================

#[test]
fn full_session_lifecycle_triggers_all_consolidation_phases() {
    let (bridge, call_count) = counting_bridge();
    let sid = test_session_id();
    let objective = "implement feature X";

    // Phase 1: Intake
    intake_memory_operations(objective, &sid, &bridge).unwrap();
    let after_intake = call_count.load(Ordering::SeqCst);
    assert!(
        after_intake >= 3,
        "intake should make at least 3 bridge calls"
    );

    // Phase 1b: Cross-session recall
    consolidation_intake(&sid, &bridge).unwrap();
    let after_consolidation_intake = call_count.load(Ordering::SeqCst);
    assert!(
        after_consolidation_intake > after_intake,
        "consolidation_intake should make at least 1 bridge call"
    );

    // Phase 2: Preparation
    let ctx = preparation_memory_operations(objective, &sid, &bridge).unwrap();
    let after_prep = call_count.load(Ordering::SeqCst);
    assert!(
        after_prep > after_consolidation_intake,
        "preparation should make bridge calls"
    );
    // With empty memory, all results should be empty.
    assert!(ctx.relevant_facts.is_empty());
    assert!(ctx.triggered_prospectives.is_empty());
    assert!(ctx.recalled_procedures.is_empty());

    // Phase 3: Reflection
    let facts = vec![FactExtraction {
        concept: "rust-ownership".to_string(),
        content: "Rust uses ownership for memory safety".to_string(),
        confidence: 0.95,
    }];
    reflection_memory_operations("session transcript here", &facts, &sid, &bridge).unwrap();
    let after_reflection = call_count.load(Ordering::SeqCst);
    assert!(
        after_reflection > after_prep,
        "reflection should make bridge calls"
    );

    // Phase 4: Persistence
    consolidation_persistence(&sid, &bridge).unwrap();
    persistence_memory_operations(&sid, &bridge).unwrap();
    let after_persistence = call_count.load(Ordering::SeqCst);
    assert!(
        after_persistence > after_reflection,
        "persistence should make bridge calls"
    );
}

// ============================================================================
// Test 2: OODA cycle triggers consolidation phases end-to-end
// ============================================================================

fn full_mock_memory_transport() -> InMemoryBridgeTransport {
    InMemoryBridgeTransport::new("ooda-lifecycle-test", |method, _params| match method {
        "memory.search_facts" => Ok(json!({"facts": []})),
        "memory.store_fact" => Ok(json!({"id": "sem_1"})),
        "memory.store_episode" => Ok(json!({"id": "epi_1"})),
        "memory.get_statistics" => Ok(json!({
            "sensory_count": 5, "working_count": 3, "episodic_count": 12,
            "semantic_count": 8, "procedural_count": 2, "prospective_count": 1
        })),
        "memory.consolidate_episodes" => Ok(json!({"id": null})),
        "memory.recall_procedure" => Ok(json!({
            "procedures": []
        })),
        "memory.record_sensory" => Ok(json!({"id": "sen_1"})),
        "memory.push_working" => Ok(json!({"id": "wrk_1"})),
        "memory.check_triggers" => Ok(json!({"prospectives": []})),
        "memory.clear_working" => Ok(json!({"count": 0})),
        "memory.prune_expired_sensory" => Ok(json!({"count": 0})),
        _ => Err(BridgeErrorPayload {
            code: -32601,
            message: format!("unknown: {method}"),
        }),
    })
}

#[test]
fn ooda_cycle_runs_with_consolidation_wired_in() {
    let bridges = OodaBridges {
        memory: Box::new(CognitiveMemoryBridge::new(Box::new(
            full_mock_memory_transport(),
        ))),
        knowledge: KnowledgeBridge::new(Box::new(InMemoryBridgeTransport::new(
            "test-knowledge",
            |method, _params| match method {
                "knowledge.list_packs" => Ok(json!({"packs": []})),
                _ => Err(BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            },
        ))),
        gym: GymBridge::new(Box::new(InMemoryBridgeTransport::new(
            "test-gym",
            |_method, _params| {
                Ok(json!({
                    "suite_id": "progressive", "success": true, "overall_score": 0.75,
                    "dimensions": {"factual_accuracy": 0.8, "specificity": 0.7,
                        "temporal_awareness": 0.75, "source_attribution": 0.7,
                        "confidence_calibration": 0.8},
                    "scenario_results": [], "scenarios_passed": 6, "scenarios_total": 6,
                    "degraded_sources": []
                }))
            },
        ))),
        session: None,
    };

    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        id: "test-goal".to_string(),
        description: "Test goal for lifecycle".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
    });

    let mut state = OodaState::new(board);
    let mut bridges = bridges;
    let config = OodaConfig::default();

    // The cycle should complete without errors, proving consolidation
    // phases are wired in and the mock handles all required methods.
    let report = run_ooda_cycle(&mut state, &mut bridges, &config).unwrap();
    assert!(report.cycle_number > 0);
}

// ============================================================================
// Test 3: Cross-session recall via intake hydration
// ============================================================================

#[test]
fn cross_session_recall_hydrates_prior_facts() {
    let call_count = Arc::new(AtomicU32::new(0));
    let counter = call_count.clone();
    let transport = InMemoryBridgeTransport::new("recall-test", move |method, _params| {
        counter.fetch_add(1, Ordering::SeqCst);
        match method {
            "memory.search_facts" => Ok(json!({
                "facts": [
                    {
                        "node_id": "n1",
                        "concept": "prior-session-fact",
                        "content": "learned from session A",
                        "confidence": 0.9,
                        "source_id": "memory-store-adapter",
                        "tags": []
                    },
                    {
                        "node_id": "n2",
                        "concept": "another-prior-fact",
                        "content": "also from session A",
                        "confidence": 0.85,
                        "source_id": "memory-store-adapter",
                        "tags": []
                    }
                ]
            })),
            "memory.push_working" => Ok(json!({"id": "wrk_1"})),
            "memory.store_episode" => Ok(json!({"id": "epi_1"})),
            "memory.record_sensory" => Ok(json!({"id": "sen_1"})),
            _ => Err(BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        }
    });
    let bridge = CognitiveMemoryBridge::new(Box::new(transport));

    // Start a new session: intake records the objective.
    let sid = SessionId::parse("session-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap();
    intake_memory_operations("continue prior work", &sid, &bridge).unwrap();

    // Cross-session recall should find 2 prior facts and push summary to
    // working memory.
    let hydrated = consolidation_intake(&sid, &bridge).unwrap();
    assert_eq!(hydrated, 2, "should hydrate 2 prior-session facts");

    // Verify bridge calls: intake (3) + consolidation_intake with facts (3)
    let total = call_count.load(Ordering::SeqCst);
    assert!(
        total >= 6,
        "intake + consolidation_intake should make >= 6 calls, got {total}"
    );
}

// ============================================================================
// Test 4: Multiple OODA cycles accumulate consolidation
// ============================================================================

#[test]
fn multiple_ooda_cycles_accumulate_consolidation() {
    let call_count = Arc::new(AtomicU32::new(0));
    let counter = call_count.clone();
    let transport = InMemoryBridgeTransport::new("multi-cycle-test", move |method, _params| {
        counter.fetch_add(1, Ordering::SeqCst);
        match method {
            "memory.search_facts" => Ok(json!({"facts": []})),
            "memory.store_fact" => Ok(json!({"id": "sem_1"})),
            "memory.store_episode" => Ok(json!({"id": "epi_1"})),
            "memory.get_statistics" => Ok(json!({
                "sensory_count": 0, "working_count": 0, "episodic_count": 0,
                "semantic_count": 0, "procedural_count": 0, "prospective_count": 0
            })),
            "memory.consolidate_episodes" => Ok(json!({"id": null})),
            "memory.recall_procedure" => Ok(json!({"procedures": []})),
            "memory.record_sensory" => Ok(json!({"id": "sen_1"})),
            "memory.push_working" => Ok(json!({"id": "wrk_1"})),
            "memory.check_triggers" => Ok(json!({"prospectives": []})),
            "memory.clear_working" => Ok(json!({"count": 0})),
            "memory.prune_expired_sensory" => Ok(json!({"count": 0})),
            _ => Err(BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        }
    });

    let mut bridges = OodaBridges {
        memory: Box::new(CognitiveMemoryBridge::new(Box::new(transport))),
        knowledge: KnowledgeBridge::new(Box::new(InMemoryBridgeTransport::new(
            "k",
            |method, _params| match method {
                "knowledge.list_packs" => Ok(json!({"packs": []})),
                _ => Err(BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            },
        ))),
        gym: GymBridge::new(Box::new(InMemoryBridgeTransport::new(
            "g",
            |_method, _params| {
                Ok(json!({
                    "suite_id": "progressive", "success": true, "overall_score": 0.9,
                    "dimensions": {"factual_accuracy": 0.9, "specificity": 0.9,
                        "temporal_awareness": 0.9, "source_attribution": 0.9,
                        "confidence_calibration": 0.9},
                    "scenario_results": [], "scenarios_passed": 1, "scenarios_total": 1,
                    "degraded_sources": []
                }))
            },
        ))),
        session: None,
    };

    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        id: "g1".to_string(),
        description: "Accumulation test".to_string(),
        priority: 1,
        status: GoalProgress::InProgress { percent: 50 },
        assigned_to: None,
    });

    let mut state = OodaState::new(board);
    let config = OodaConfig::default();

    // Run two cycles — both should succeed.
    let r1 = run_ooda_cycle(&mut state, &mut bridges, &config).unwrap();
    let calls_after_first = call_count.load(Ordering::SeqCst);

    let r2 = run_ooda_cycle(&mut state, &mut bridges, &config).unwrap();
    let calls_after_second = call_count.load(Ordering::SeqCst);

    assert_eq!(r1.cycle_number, 1);
    assert_eq!(r2.cycle_number, 2);

    // Second cycle should make roughly the same number of bridge calls,
    // proving consolidation runs each cycle.
    let first_cycle_calls = calls_after_first;
    let second_cycle_calls = calls_after_second - calls_after_first;
    assert!(
        second_cycle_calls > 0,
        "second cycle should make bridge calls for consolidation"
    );
    assert!(
        (second_cycle_calls as f64) >= (first_cycle_calls as f64 * 0.5),
        "second cycle should make a similar number of calls as first: {second_cycle_calls} vs {first_cycle_calls}"
    );
}
