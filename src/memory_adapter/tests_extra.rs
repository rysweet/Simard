use super::*;
use crate::server_subprocess::InMemoryServerTransport;

fn mock_adapter() -> CognitiveMemoryAdapter {
    let transport = InMemoryServerTransport::new("test-memory", |method, params| match method {
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
        "memory.search_episodes_by_keywords" => Ok(json!({"episodes": []})),
        _ => Err(crate::server_transport::ServerErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    CognitiveMemoryAdapter::new(Box::new(transport))
}

// --- Error propagation tests ---

fn error_adapter() -> CognitiveMemoryAdapter {
    let transport = InMemoryServerTransport::new("error-adapter", |method, _params| {
        Err(crate::server_transport::ServerErrorPayload {
            code: crate::server_transport::SERVER_ERROR_INTERNAL,
            message: format!("server error on {method}"),
        })
    });
    CognitiveMemoryAdapter::new(Box::new(transport))
}

#[test]
fn check_triggers_propagates_adapter_error() {
    let adapter = error_adapter();
    let result = adapter.check_triggers("content");
    assert!(result.is_err());
}

// --- Health check tests ---

#[test]
fn health_check_on_healthy_adapter() {
    let transport =
        InMemoryServerTransport::new("healthy-adapter", |method, _params| match method {
            "bridge.health" => Ok(json!({"server_name": "healthy-adapter", "healthy": true})),
            _ => Ok(json!({})),
        });
    let health = transport.health().unwrap();
    assert!(health.healthy);
    assert_eq!(health.server_name, "healthy-adapter");
}

#[test]
fn health_check_on_unhealthy_adapter() {
    let transport = InMemoryServerTransport::new("unhealthy", |_method, _params| {
        Err(crate::server_transport::ServerErrorPayload {
            code: crate::server_transport::SERVER_ERROR_INTERNAL,
            message: "adapter is down".to_string(),
        })
    });
    let result = transport.health();
    assert!(result.is_err());
}

// --- Circuit breaker integration tests ---

#[test]
fn circuit_breaker_passes_through_on_success() {
    use crate::server_circuit::{CircuitBreakerConfig, CircuitBreakerTransport};
    use std::time::Duration;

    let inner = InMemoryServerTransport::new("cb-ok", |method, params| match method {
        "memory.store_fact" => Ok(json!({"id": "cb_fact_1"})),
        _ => Ok(params.clone()),
    });
    let cb = CircuitBreakerTransport::new(
        inner,
        CircuitBreakerConfig {
            failure_threshold: 3,
            cooldown: Duration::from_secs(30),
        },
    );
    let adapter = CognitiveMemoryAdapter::new(Box::new(cb));
    let id = adapter.store_fact("test", "data", 0.8, &[], "").unwrap();
    assert_eq!(id, "cb_fact_1");
}

#[test]
fn circuit_breaker_opens_after_repeated_transport_failures() {
    use crate::server_circuit::{CircuitBreakerConfig, CircuitBreakerTransport, CircuitState};
    use std::time::Duration;

    let inner = InMemoryServerTransport::new("cb-fail", |_method, _params| {
        Err(crate::server_transport::ServerErrorPayload {
            code: crate::server_transport::SERVER_ERROR_TRANSPORT,
            message: "transport down".to_string(),
        })
    });
    let cb = CircuitBreakerTransport::new(
        inner,
        CircuitBreakerConfig {
            failure_threshold: 2,
            cooldown: Duration::from_secs(60),
        },
    );

    // Two failures should open the circuit.
    let _ = cb.call(crate::server_transport::ServerRequest {
        id: crate::server_transport::new_request_id(),
        method: "memory.store_fact".into(),
        params: json!({}),
    });
    let _ = cb.call(crate::server_transport::ServerRequest {
        id: crate::server_transport::new_request_id(),
        method: "memory.store_fact".into(),
        params: json!({}),
    });
    assert_eq!(cb.circuit_state(), CircuitState::Open);

    // Subsequent call is rejected immediately.
    let adapter = CognitiveMemoryAdapter::new(Box::new(cb));
    let result = adapter.store_fact("test", "data", 0.5, &[], "");
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("circuit is open"), "got: {msg}");
}

#[test]
fn circuit_breaker_recovers_after_cooldown() {
    use crate::server_circuit::{CircuitBreakerConfig, CircuitBreakerTransport, CircuitState};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    let call_count = std::sync::Arc::new(AtomicU32::new(0));
    let counter = call_count.clone();
    let inner = InMemoryServerTransport::new("cb-recover", move |_method, _params| {
        let n = counter.fetch_add(1, Ordering::SeqCst);
        if n < 2 {
            Err(crate::server_transport::ServerErrorPayload {
                code: crate::server_transport::SERVER_ERROR_TRANSPORT,
                message: "down".to_string(),
            })
        } else {
            Ok(json!({"id": "recovered_fact"}))
        }
    });
    let cb = CircuitBreakerTransport::new(
        inner,
        CircuitBreakerConfig {
            failure_threshold: 2,
            cooldown: Duration::from_millis(1),
        },
    );

    // Trip the circuit.
    let _ = cb.call(crate::server_transport::ServerRequest {
        id: crate::server_transport::new_request_id(),
        method: "memory.store_fact".into(),
        params: json!({}),
    });
    let _ = cb.call(crate::server_transport::ServerRequest {
        id: crate::server_transport::new_request_id(),
        method: "memory.store_fact".into(),
        params: json!({}),
    });
    assert_eq!(cb.circuit_state(), CircuitState::Open);

    // Wait for cooldown, then call through the adapter wrapper.
    std::thread::sleep(Duration::from_millis(10));
    let adapter = CognitiveMemoryAdapter::new(Box::new(cb));
    let id = adapter.store_fact("test", "data", 0.5, &[], "").unwrap();
    assert_eq!(id, "recovered_fact");
}

// --- Edge case tests ---

#[test]
fn empty_facts_response() {
    let transport = InMemoryServerTransport::new("empty-facts", |method, _params| match method {
        "memory.search_facts" => Ok(json!({"facts": []})),
        _ => Ok(json!({})),
    });
    let adapter = CognitiveMemoryAdapter::new(Box::new(transport));
    let facts = adapter.search_facts("nothing", 10, 0.0).unwrap();
    assert!(facts.is_empty());
}

#[test]
fn empty_working_slots_response() {
    let transport = InMemoryServerTransport::new("empty-working", |method, _params| match method {
        "memory.get_working" => Ok(json!({"slots": []})),
        _ => Ok(json!({})),
    });
    let adapter = CognitiveMemoryAdapter::new(Box::new(transport));
    let slots = adapter.get_working("no-task").unwrap();
    assert!(slots.is_empty());
}

#[test]
fn malformed_json_response_returns_error() {
    let transport = InMemoryServerTransport::new("malformed", |_method, _params| {
        Ok(json!({"unexpected_field": true}))
    });
    let adapter = CognitiveMemoryAdapter::new(Box::new(transport));
    let result = adapter.store_fact("c", "content", 0.5, &[], "src");
    assert!(
        result.is_err(),
        "missing 'id' field should cause deserialization error"
    );
}

#[test]
fn unknown_method_returns_error() {
    let adapter = mock_adapter();
    // Directly test the call path with an unknown method.
    let result: SimardResult<serde_json::Value> = adapter.call("memory.nonexistent", json!({}));
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("unknown method"), "got: {msg}");
}

#[test]
fn consolidate_episodes_with_present_id() {
    let transport =
        InMemoryServerTransport::new("consolidate-ok", |method, _params| match method {
            "memory.consolidate_episodes" => Ok(json!({"id": "consolidated_123"})),
            _ => Ok(json!({})),
        });
    let adapter = CognitiveMemoryAdapter::new(Box::new(transport));
    let result = adapter.consolidate_episodes(5).unwrap();
    assert_eq!(result, Some("consolidated_123".to_string()));
}

#[test]
fn store_fact_with_tags() {
    let adapter = mock_adapter();
    let id = adapter
        .store_fact(
            "rust",
            "fast language",
            0.95,
            &["lang".to_string(), "systems".to_string()],
            "source-1",
        )
        .unwrap();
    assert_eq!(id, "sem_test123");
}

#[test]
fn search_facts_respects_params() {
    let transport = InMemoryServerTransport::new("search-params", |method, params| match method {
        "memory.search_facts" => {
            assert_eq!(params["limit"], 5);
            assert!((params["min_confidence"].as_f64().unwrap() - 0.7).abs() < f64::EPSILON);
            Ok(json!({"facts": []}))
        }
        _ => Ok(json!({})),
    });
    let adapter = CognitiveMemoryAdapter::new(Box::new(transport));
    let facts = adapter.search_facts("query", 5, 0.7).unwrap();
    assert!(facts.is_empty());
}

#[test]
fn cognitive_memory_ops_trait_delegates_to_adapter() {
    let adapter = mock_adapter();
    // Call through the trait interface.
    let ops: &dyn CognitiveMemoryOps = &adapter;
    let id = ops
        .store_fact("concept", "content", 0.8, &[], "src")
        .unwrap();
    assert_eq!(id, "sem_test123");
    let stats = ops.get_statistics().unwrap();
    assert_eq!(stats.total(), 21);
}
