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
mod tests;
#[cfg(test)]
mod tests_extra;
