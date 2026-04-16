//! Bridge client for cognitive memory operations.
//!
//! `CognitiveMemoryBridge` wraps a `BridgeTransport` and provides typed
//! methods for each of the six cognitive memory types. Each method serializes
//! parameters to JSON, sends them over the bridge, and deserializes the
//! response into the corresponding Rust type from `memory_cognitive`.
//!
//! Wire methods use the `memory.*` namespace (e.g. `memory.store_fact`).

use serde_json::json;

use crate::bridge::{BridgeRequest, BridgeTransport, new_request_id, unpack_bridge_response};
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

const BRIDGE_NAME: &str = "cognitive-memory";

/// Typed client for the cognitive memory bridge server.
///
/// All methods are synchronous and block on the underlying transport. Errors
/// from the bridge server (e.g. invalid parameters, database failures) are
/// returned as `SimardError::BridgeCallFailed`.
pub struct CognitiveMemoryBridge {
    transport: Box<dyn BridgeTransport>,
}

impl CognitiveMemoryBridge {
    pub fn new(transport: Box<dyn BridgeTransport>) -> Self {
        Self { transport }
    }

    /// Call a bridge method and deserialize the response.
    fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> SimardResult<T> {
        let request = BridgeRequest {
            id: new_request_id(),
            method: method.to_string(),
            params,
        };
        let response = self.transport.call(request)?;
        unpack_bridge_response(BRIDGE_NAME, method, response)
    }

    /// Record a short-lived sensory observation. Returns the `node_id`.
    #[tracing::instrument(skip(self))]
    pub fn record_sensory(
        &self,
        modality: &str,
        raw_data: &str,
        ttl_seconds: u64,
    ) -> SimardResult<String> {
        let result: IdResponse = self.call(
            "memory.record_sensory",
            json!({
                "modality": modality,
                "raw_data": raw_data,
                "ttl_seconds": ttl_seconds,
            }),
        )?;
        Ok(result.id)
    }

    /// Delete sensory items past their expiry time. Returns the count pruned.
    pub fn prune_expired_sensory(&self) -> SimardResult<usize> {
        let result: CountResponse = self.call("memory.prune_expired_sensory", json!({}))?;
        Ok(result.count)
    }

    /// Push a slot into working memory for a given task. Returns the `node_id`.
    #[tracing::instrument(skip(self))]
    pub fn push_working(
        &self,
        slot_type: &str,
        content: &str,
        task_id: &str,
        relevance: f64,
    ) -> SimardResult<String> {
        let result: IdResponse = self.call(
            "memory.push_working",
            json!({
                "slot_type": slot_type,
                "content": content,
                "task_id": task_id,
                "relevance": relevance,
            }),
        )?;
        Ok(result.id)
    }

    /// Retrieve working memory slots for a task.
    pub fn get_working(&self, task_id: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        let result: SlotsResponse = self.call("memory.get_working", json!({"task_id": task_id}))?;
        Ok(result.slots)
    }

    /// Clear all working memory slots for a task. Returns the count cleared.
    pub fn clear_working(&self, task_id: &str) -> SimardResult<usize> {
        let result: CountResponse =
            self.call("memory.clear_working", json!({"task_id": task_id}))?;
        Ok(result.count)
    }

    /// Store an episodic memory. Returns the `node_id`.
    #[tracing::instrument(skip(self))]
    pub fn store_episode(
        &self,
        content: &str,
        source_label: &str,
        metadata: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        let result: IdResponse = self.call(
            "memory.store_episode",
            json!({
                "content": content,
                "source_label": source_label,
                "metadata": metadata.unwrap_or(&json!({})),
            }),
        )?;
        Ok(result.id)
    }

    /// Consolidate oldest un-compressed episodes. Returns `None` if insufficient.
    #[tracing::instrument(skip(self))]
    pub fn consolidate_episodes(&self, batch_size: u32) -> SimardResult<Option<String>> {
        let result: OptionalIdResponse = self.call(
            "memory.consolidate_episodes",
            json!({"batch_size": batch_size}),
        )?;
        Ok(result.id)
    }

    /// Store a semantic fact. Returns the `node_id`.
    #[tracing::instrument(skip(self))]
    pub fn store_fact(
        &self,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        let result: IdResponse = self.call(
            "memory.store_fact",
            json!({
                "concept": concept,
                "content": content,
                "confidence": confidence,
                "tags": tags,
                "source_id": source_id,
            }),
        )?;
        Ok(result.id)
    }

    /// Search semantic facts by keyword matching.
    #[tracing::instrument(skip(self))]
    pub fn search_facts(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
    ) -> SimardResult<Vec<CognitiveFact>> {
        let result: FactsResponse = self.call(
            "memory.search_facts",
            json!({
                "query": query,
                "limit": limit,
                "min_confidence": min_confidence,
            }),
        )?;
        Ok(result.facts)
    }

    /// Store a reusable procedure. Returns the `node_id`.
    pub fn store_procedure(
        &self,
        name: &str,
        steps: &[String],
        prerequisites: &[String],
    ) -> SimardResult<String> {
        let result: IdResponse = self.call(
            "memory.store_procedure",
            json!({
                "name": name,
                "steps": steps,
                "prerequisites": prerequisites,
            }),
        )?;
        Ok(result.id)
    }

    /// Recall procedures matching a query.
    pub fn recall_procedure(
        &self,
        query: &str,
        limit: u32,
    ) -> SimardResult<Vec<CognitiveProcedure>> {
        let result: ProceduresResponse = self.call(
            "memory.recall_procedure",
            json!({
                "query": query,
                "limit": limit,
            }),
        )?;
        Ok(result.procedures)
    }

    /// Store a trigger-action pair for future evaluation. Returns the `node_id`.
    pub fn store_prospective(
        &self,
        description: &str,
        trigger_condition: &str,
        action_on_trigger: &str,
        priority: i64,
    ) -> SimardResult<String> {
        let result: IdResponse = self.call(
            "memory.store_prospective",
            json!({
                "description": description,
                "trigger_condition": trigger_condition,
                "action_on_trigger": action_on_trigger,
                "priority": priority,
            }),
        )?;
        Ok(result.id)
    }

    /// Check pending prospective memories against provided content.
    pub fn check_triggers(&self, content: &str) -> SimardResult<Vec<CognitiveProspective>> {
        let result: ProspectivesResponse =
            self.call("memory.check_triggers", json!({"content": content}))?;
        Ok(result.prospectives)
    }

    /// Return aggregate counts across all six cognitive memory types.
    pub fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        self.call("memory.get_statistics", json!({}))
    }
}

impl CognitiveMemoryOps for CognitiveMemoryBridge {
    fn record_sensory(
        &self,
        modality: &str,
        raw_data: &str,
        ttl_seconds: u64,
    ) -> SimardResult<String> {
        CognitiveMemoryBridge::record_sensory(self, modality, raw_data, ttl_seconds)
    }

    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        CognitiveMemoryBridge::prune_expired_sensory(self)
    }

    fn push_working(
        &self,
        slot_type: &str,
        content: &str,
        task_id: &str,
        relevance: f64,
    ) -> SimardResult<String> {
        CognitiveMemoryBridge::push_working(self, slot_type, content, task_id, relevance)
    }

    fn get_working(&self, task_id: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        CognitiveMemoryBridge::get_working(self, task_id)
    }

    fn clear_working(&self, task_id: &str) -> SimardResult<usize> {
        CognitiveMemoryBridge::clear_working(self, task_id)
    }

    fn store_episode(
        &self,
        content: &str,
        source_label: &str,
        metadata: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        CognitiveMemoryBridge::store_episode(self, content, source_label, metadata)
    }

    fn consolidate_episodes(&self, batch_size: u32) -> SimardResult<Option<String>> {
        CognitiveMemoryBridge::consolidate_episodes(self, batch_size)
    }

    fn store_fact(
        &self,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        CognitiveMemoryBridge::store_fact(self, concept, content, confidence, tags, source_id)
    }

    fn search_facts(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
    ) -> SimardResult<Vec<CognitiveFact>> {
        CognitiveMemoryBridge::search_facts(self, query, limit, min_confidence)
    }

    fn store_procedure(
        &self,
        name: &str,
        steps: &[String],
        prerequisites: &[String],
    ) -> SimardResult<String> {
        CognitiveMemoryBridge::store_procedure(self, name, steps, prerequisites)
    }

    fn recall_procedure(&self, query: &str, limit: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        CognitiveMemoryBridge::recall_procedure(self, query, limit)
    }

    fn store_prospective(
        &self,
        description: &str,
        trigger_condition: &str,
        action_on_trigger: &str,
        priority: i64,
    ) -> SimardResult<String> {
        CognitiveMemoryBridge::store_prospective(
            self,
            description,
            trigger_condition,
            action_on_trigger,
            priority,
        )
    }

    fn check_triggers(&self, content: &str) -> SimardResult<Vec<CognitiveProspective>> {
        CognitiveMemoryBridge::check_triggers(self, content)
    }

    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        CognitiveMemoryBridge::get_statistics(self)
    }
}

// Wire-format response wrappers
#[derive(serde::Deserialize)]
struct IdResponse {
    id: String,
}

#[derive(serde::Deserialize)]
struct OptionalIdResponse {
    id: Option<String>,
}

#[derive(serde::Deserialize)]
struct CountResponse {
    count: usize,
}

#[derive(serde::Deserialize)]
struct SlotsResponse {
    slots: Vec<CognitiveWorkingSlot>,
}

#[derive(serde::Deserialize)]
struct FactsResponse {
    facts: Vec<CognitiveFact>,
}

#[derive(serde::Deserialize)]
struct ProceduresResponse {
    procedures: Vec<CognitiveProcedure>,
}

#[derive(serde::Deserialize)]
struct ProspectivesResponse {
    prospectives: Vec<CognitiveProspective>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_subprocess::InMemoryBridgeTransport;

    fn mock_bridge() -> CognitiveMemoryBridge {
        let transport =
            InMemoryBridgeTransport::new("test-memory", |method, params| match method {
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

    #[test]
    fn check_triggers_propagates_bridge_error() {
        let bridge = error_bridge();
        let result = bridge.check_triggers("content");
        assert!(result.is_err());
    }

    // --- Health check tests ---

    #[test]
    fn health_check_on_healthy_bridge() {
        let transport =
            InMemoryBridgeTransport::new("healthy-bridge", |method, _params| match method {
                "bridge.health" => Ok(json!({"server_name": "healthy-bridge", "healthy": true})),
                _ => Ok(json!({})),
            });
        let health = transport.health().unwrap();
        assert!(health.healthy);
        assert_eq!(health.server_name, "healthy-bridge");
    }

    #[test]
    fn health_check_on_unhealthy_bridge() {
        let transport = InMemoryBridgeTransport::new("unhealthy", |_method, _params| {
            Err(crate::bridge::BridgeErrorPayload {
                code: crate::bridge::BRIDGE_ERROR_INTERNAL,
                message: "bridge is down".to_string(),
            })
        });
        let result = transport.health();
        assert!(result.is_err());
    }

    // --- Circuit breaker integration tests ---

    #[test]
    fn circuit_breaker_passes_through_on_success() {
        use crate::bridge_circuit::{CircuitBreakerConfig, CircuitBreakerTransport};
        use std::time::Duration;

        let inner = InMemoryBridgeTransport::new("cb-ok", |method, params| match method {
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
        let bridge = CognitiveMemoryBridge::new(Box::new(cb));
        let id = bridge.store_fact("test", "data", 0.8, &[], "").unwrap();
        assert_eq!(id, "cb_fact_1");
    }

    #[test]
    fn circuit_breaker_opens_after_repeated_transport_failures() {
        use crate::bridge_circuit::{CircuitBreakerConfig, CircuitBreakerTransport, CircuitState};
        use std::time::Duration;

        let inner = InMemoryBridgeTransport::new("cb-fail", |_method, _params| {
            Err(crate::bridge::BridgeErrorPayload {
                code: crate::bridge::BRIDGE_ERROR_TRANSPORT,
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
        let _ = cb.call(crate::bridge::BridgeRequest {
            id: crate::bridge::new_request_id(),
            method: "memory.store_fact".into(),
            params: json!({}),
        });
        let _ = cb.call(crate::bridge::BridgeRequest {
            id: crate::bridge::new_request_id(),
            method: "memory.store_fact".into(),
            params: json!({}),
        });
        assert_eq!(cb.circuit_state(), CircuitState::Open);

        // Subsequent call is rejected immediately.
        let bridge = CognitiveMemoryBridge::new(Box::new(cb));
        let result = bridge.store_fact("test", "data", 0.5, &[], "");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("circuit is open"), "got: {msg}");
    }

    #[test]
    fn circuit_breaker_recovers_after_cooldown() {
        use crate::bridge_circuit::{CircuitBreakerConfig, CircuitBreakerTransport, CircuitState};
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::time::Duration;

        let call_count = std::sync::Arc::new(AtomicU32::new(0));
        let counter = call_count.clone();
        let inner = InMemoryBridgeTransport::new("cb-recover", move |_method, _params| {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(crate::bridge::BridgeErrorPayload {
                    code: crate::bridge::BRIDGE_ERROR_TRANSPORT,
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
        let _ = cb.call(crate::bridge::BridgeRequest {
            id: crate::bridge::new_request_id(),
            method: "memory.store_fact".into(),
            params: json!({}),
        });
        let _ = cb.call(crate::bridge::BridgeRequest {
            id: crate::bridge::new_request_id(),
            method: "memory.store_fact".into(),
            params: json!({}),
        });
        assert_eq!(cb.circuit_state(), CircuitState::Open);

        // Wait for cooldown, then call through the bridge wrapper.
        std::thread::sleep(Duration::from_millis(10));
        let bridge = CognitiveMemoryBridge::new(Box::new(cb));
        let id = bridge.store_fact("test", "data", 0.5, &[], "").unwrap();
        assert_eq!(id, "recovered_fact");
    }

    // --- Edge case tests ---

    #[test]
    fn empty_facts_response() {
        let transport =
            InMemoryBridgeTransport::new("empty-facts", |method, _params| match method {
                "memory.search_facts" => Ok(json!({"facts": []})),
                _ => Ok(json!({})),
            });
        let bridge = CognitiveMemoryBridge::new(Box::new(transport));
        let facts = bridge.search_facts("nothing", 10, 0.0).unwrap();
        assert!(facts.is_empty());
    }

    #[test]
    fn empty_working_slots_response() {
        let transport =
            InMemoryBridgeTransport::new("empty-working", |method, _params| match method {
                "memory.get_working" => Ok(json!({"slots": []})),
                _ => Ok(json!({})),
            });
        let bridge = CognitiveMemoryBridge::new(Box::new(transport));
        let slots = bridge.get_working("no-task").unwrap();
        assert!(slots.is_empty());
    }

    #[test]
    fn malformed_json_response_returns_error() {
        let transport = InMemoryBridgeTransport::new("malformed", |_method, _params| {
            Ok(json!({"unexpected_field": true}))
        });
        let bridge = CognitiveMemoryBridge::new(Box::new(transport));
        let result = bridge.store_fact("c", "content", 0.5, &[], "src");
        assert!(
            result.is_err(),
            "missing 'id' field should cause deserialization error"
        );
    }

    #[test]
    fn unknown_method_returns_error() {
        let bridge = mock_bridge();
        // Directly test the call path with an unknown method.
        let result: SimardResult<serde_json::Value> = bridge.call("memory.nonexistent", json!({}));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("unknown method"), "got: {msg}");
    }

    #[test]
    fn consolidate_episodes_with_present_id() {
        let transport =
            InMemoryBridgeTransport::new("consolidate-ok", |method, _params| match method {
                "memory.consolidate_episodes" => Ok(json!({"id": "consolidated_123"})),
                _ => Ok(json!({})),
            });
        let bridge = CognitiveMemoryBridge::new(Box::new(transport));
        let result = bridge.consolidate_episodes(5).unwrap();
        assert_eq!(result, Some("consolidated_123".to_string()));
    }

    #[test]
    fn store_fact_with_tags() {
        let bridge = mock_bridge();
        let id = bridge
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
        let transport =
            InMemoryBridgeTransport::new("search-params", |method, params| match method {
                "memory.search_facts" => {
                    assert_eq!(params["limit"], 5);
                    assert!(
                        (params["min_confidence"].as_f64().unwrap() - 0.7).abs() < f64::EPSILON
                    );
                    Ok(json!({"facts": []}))
                }
                _ => Ok(json!({})),
            });
        let bridge = CognitiveMemoryBridge::new(Box::new(transport));
        let facts = bridge.search_facts("query", 5, 0.7).unwrap();
        assert!(facts.is_empty());
    }

    #[test]
    fn cognitive_memory_ops_trait_delegates_to_bridge() {
        let bridge = mock_bridge();
        // Call through the trait interface.
        let ops: &dyn CognitiveMemoryOps = &bridge;
        let id = ops
            .store_fact("concept", "content", 0.8, &[], "src")
            .unwrap();
        assert_eq!(id, "sem_test123");
        let stats = ops.get_statistics().unwrap();
        assert_eq!(stats.total(), 21);
    }
}
