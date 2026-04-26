use super::*;
use crate::bridge_subprocess::InMemoryBridgeTransport;

fn mock_bridge() -> CognitiveMemoryBridge {
    let transport = InMemoryBridgeTransport::new("test-memory", |method, params| match method {
        "memory.store_fact" => Ok(json!({"id": "sem_test123"})),
        "memory.search_facts" => Ok(json!({
            "facts": [{
                "node_id": "sem_test123",
                "concept": params["query"].as_str().unwrap_or("unknown"),
                "content": "test content",
                "confidence": 0.9,
                "source_id": "",
                "tags": []
            }]
        })),
        "memory.get_statistics" => Ok(json!({
            "sensory_count": 1,
            "working_count": 2,
            "episodic_count": 3,
            "semantic_count": 4,
            "procedural_count": 5,
            "prospective_count": 6
        })),
        "memory.push_working" => Ok(json!({"id": "wrk_test"})),
        "memory.get_working" => Ok(json!({
            "slots": [{
                "node_id": "wrk_test",
                "slot_type": "goal",
                "content": "test",
                "relevance": 1.0,
                "task_id": params["task_id"].as_str().unwrap_or("t1")
            }]
        })),
        "memory.clear_working" => Ok(json!({"count": 1})),
        "memory.record_sensory" => Ok(json!({"id": "sen_test"})),
        "memory.prune_expired_sensory" => Ok(json!({"count": 0})),
        "memory.store_episode" => Ok(json!({"id": "epi_test"})),
        "memory.consolidate_episodes" => Ok(json!({"id": null})),
        "memory.store_procedure" => Ok(json!({"id": "proc_test"})),
        "memory.recall_procedure" => Ok(json!({
            "procedures": [{
                "node_id": "proc_test",
                "name": "build",
                "steps": ["compile", "test"],
                "prerequisites": [],
                "usage_count": 1
            }]
        })),
        "memory.store_prospective" => Ok(json!({"id": "pro_test"})),
        "memory.check_triggers" => Ok(json!({"prospectives": []})),
        _ => Err(crate::bridge::BridgeErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    CognitiveMemoryBridge::new(Box::new(transport))
}

#[test]
fn store_and_search_fact_via_bridge() {
    let bridge = mock_bridge();
    let id = bridge
        .store_fact("rust", "systems language", 0.9, &[], "")
        .unwrap();
    assert_eq!(id, "sem_test123");
    let facts = bridge.search_facts("rust", 10, 0.0).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].concept, "rust");
}

#[test]
fn get_statistics_returns_typed_result() {
    let bridge = mock_bridge();
    let stats = bridge.get_statistics().unwrap();
    assert_eq!(stats.sensory_count, 1);
    assert_eq!(stats.total(), 21);
}

// --- RPC round-trip tests for every operation ---

#[test]
fn record_sensory_returns_node_id() {
    let bridge = mock_bridge();
    let id = bridge.record_sensory("visual", "raw pixels", 60).unwrap();
    assert_eq!(id, "sen_test");
}

#[test]
fn prune_expired_sensory_returns_count() {
    let bridge = mock_bridge();
    let count = bridge.prune_expired_sensory().unwrap();
    assert_eq!(count, 0);
}

#[test]
fn push_and_get_working_round_trip() {
    let bridge = mock_bridge();
    let id = bridge
        .push_working("goal", "finish task", "task-1", 0.95)
        .unwrap();
    assert_eq!(id, "wrk_test");

    let slots = bridge.get_working("task-1").unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].slot_type, "goal");
    assert_eq!(slots[0].task_id, "task-1");
}

#[test]
fn clear_working_returns_count() {
    let bridge = mock_bridge();
    let count = bridge.clear_working("task-1").unwrap();
    assert_eq!(count, 1);
}

#[test]
fn store_episode_returns_node_id() {
    let bridge = mock_bridge();
    let id = bridge
        .store_episode("something happened", "test-source", None)
        .unwrap();
    assert_eq!(id, "epi_test");
}

#[test]
fn store_episode_with_metadata() {
    let bridge = mock_bridge();
    let meta = json!({"key": "value"});
    let id = bridge
        .store_episode("event", "source", Some(&meta))
        .unwrap();
    assert_eq!(id, "epi_test");
}

#[test]
fn consolidate_episodes_returns_none_when_insufficient() {
    let bridge = mock_bridge();
    let result = bridge.consolidate_episodes(10).unwrap();
    assert!(result.is_none());
}

#[test]
fn store_procedure_returns_node_id() {
    let bridge = mock_bridge();
    let id = bridge
        .store_procedure(
            "build",
            &["compile".into(), "test".into()],
            &["cargo".into()],
        )
        .unwrap();
    assert_eq!(id, "proc_test");
}

#[test]
fn recall_procedure_returns_list() {
    let bridge = mock_bridge();
    let procs = bridge.recall_procedure("build", 5).unwrap();
    assert_eq!(procs.len(), 1);
    assert_eq!(procs[0].name, "build");
    assert_eq!(procs[0].steps, vec!["compile", "test"]);
}

#[test]
fn store_prospective_returns_node_id() {
    let bridge = mock_bridge();
    let id = bridge
        .store_prospective("remind me", "when idle", "do thing", 5)
        .unwrap();
    assert_eq!(id, "pro_test");
}

#[test]
fn check_triggers_returns_empty_vec() {
    let bridge = mock_bridge();
    let triggered = bridge.check_triggers("some content").unwrap();
    assert!(triggered.is_empty());
}

// --- Error propagation tests ---

fn error_bridge() -> CognitiveMemoryBridge {
    let transport = InMemoryBridgeTransport::new("error-bridge", |method, _params| {
        Err(crate::bridge::BridgeErrorPayload {
            code: crate::bridge::BRIDGE_ERROR_INTERNAL,
            message: format!("server error on {method}"),
        })
    });
    CognitiveMemoryBridge::new(Box::new(transport))
}

#[test]
fn store_fact_propagates_bridge_error() {
    let bridge = error_bridge();
    let result = bridge.store_fact("c", "content", 0.5, &[], "src");
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("server error"), "got: {msg}");
}

#[test]
fn search_facts_propagates_bridge_error() {
    let bridge = error_bridge();
    let result = bridge.search_facts("q", 10, 0.0);
    assert!(result.is_err());
}

#[test]
fn record_sensory_propagates_bridge_error() {
    let bridge = error_bridge();
    let result = bridge.record_sensory("audio", "data", 30);
    assert!(result.is_err());
}

#[test]
fn get_working_propagates_bridge_error() {
    let bridge = error_bridge();
    let result = bridge.get_working("task-1");
    assert!(result.is_err());
}

#[test]
fn get_statistics_propagates_bridge_error() {
    let bridge = error_bridge();
    let result = bridge.get_statistics();
    assert!(result.is_err());
}

#[test]
fn consolidate_episodes_propagates_bridge_error() {
    let bridge = error_bridge();
    let result = bridge.consolidate_episodes(5);
    assert!(result.is_err());
}

#[test]
fn recall_procedure_propagates_bridge_error() {
    let bridge = error_bridge();
    let result = bridge.recall_procedure("build", 5);
    assert!(result.is_err());
}
