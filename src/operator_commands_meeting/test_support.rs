use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::memory_bridge::CognitiveMemoryBridge;

/// Create a `CognitiveMemoryBridge` backed by an in-memory stub that
/// returns empty results for all `search_facts` queries.
pub fn empty_bridge() -> CognitiveMemoryBridge {
    let transport = InMemoryBridgeTransport::new("test-empty", |method, _params| match method {
        "memory.search_facts" => Ok(serde_json::json!({"facts": []})),
        "memory.get_statistics" => Ok(serde_json::json!({
            "sensory_count": 0, "working_count": 0, "episodic_count": 0,
            "semantic_count": 0, "procedural_count": 0, "prospective_count": 0
        })),
        _ => Err(crate::bridge::BridgeErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    CognitiveMemoryBridge::new(Box::new(transport))
}

/// Create a bridge that returns a single meeting fact for `"meeting:"`
/// queries and empty results for everything else.
pub fn bridge_with_meeting_facts() -> CognitiveMemoryBridge {
    let transport = InMemoryBridgeTransport::new("test-facts", |method, params| match method {
        "memory.search_facts" => {
            let query = params["query"].as_str().unwrap_or("");
            if query.starts_with("meeting:") {
                Ok(serde_json::json!({
                    "facts": [{
                        "node_id": "f1",
                        "concept": "weekly-sync",
                        "content": "Discussed deployment timeline",
                        "confidence": 0.9,
                        "source_id": "s1",
                        "tags": []
                    }]
                }))
            } else {
                Ok(serde_json::json!({"facts": []}))
            }
        }
        _ => Err(crate::bridge::BridgeErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    CognitiveMemoryBridge::new(Box::new(transport))
}

/// Create a bridge that returns facts for a specific query prefix.
pub fn bridge_with_specific_facts(
    prefix: &'static str,
    concept: &'static str,
    content: &'static str,
) -> CognitiveMemoryBridge {
    let transport =
        InMemoryBridgeTransport::new("test-specific", move |method, params| match method {
            "memory.search_facts" => {
                let query = params["query"].as_str().unwrap_or("");
                if query.starts_with(prefix) {
                    Ok(serde_json::json!({
                        "facts": [{
                            "node_id": "n1",
                            "concept": concept,
                            "content": content,
                            "confidence": 0.9,
                            "source_id": "s1",
                            "tags": []
                        }]
                    }))
                } else {
                    Ok(serde_json::json!({"facts": []}))
                }
            }
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    CognitiveMemoryBridge::new(Box::new(transport))
}

/// Create a bridge that returns facts for all query prefixes used by
/// `build_live_meeting_context`.
pub fn bridge_with_all_fact_types() -> CognitiveMemoryBridge {
    let transport = InMemoryBridgeTransport::new("test-all", |method, params| match method {
        "memory.search_facts" => {
            let query = params["query"].as_str().unwrap_or("");
            let facts = if query.starts_with("meeting:") {
                serde_json::json!([{
                    "node_id": "m1", "concept": "weekly-sync",
                    "content": "Sprint review completed", "confidence": 0.9,
                    "source_id": "s1", "tags": []
                }])
            } else if query.starts_with("decision:") {
                serde_json::json!([{
                    "node_id": "d1", "concept": "decision",
                    "content": "Approved migration plan", "confidence": 0.9,
                    "source_id": "s2", "tags": []
                }])
            } else if query.starts_with("goal:") {
                serde_json::json!([{
                    "node_id": "g1", "concept": "goal",
                    "content": "Complete API refactor", "confidence": 0.9,
                    "source_id": "s3", "tags": []
                }])
            } else if query.starts_with("operator:") {
                serde_json::json!([{
                    "node_id": "o1", "concept": "operator",
                    "content": "Test Operator identity", "confidence": 0.9,
                    "source_id": "s4", "tags": []
                }])
            } else if query.starts_with("project:") {
                serde_json::json!([{
                    "node_id": "p1", "concept": "project",
                    "content": "TestProject — testing suite", "confidence": 0.9,
                    "source_id": "s5", "tags": []
                }])
            } else if query.starts_with("research:") {
                serde_json::json!([{
                    "node_id": "r1", "concept": "research",
                    "content": "Investigating new LLM patterns", "confidence": 0.9,
                    "source_id": "s6", "tags": []
                }])
            } else if query.starts_with("improvement:") {
                serde_json::json!([{
                    "node_id": "i1", "concept": "improvement",
                    "content": "Add better error handling", "confidence": 0.9,
                    "source_id": "s7", "tags": []
                }])
            } else {
                serde_json::json!([])
            };
            Ok(serde_json::json!({"facts": facts}))
        }
        _ => Err(crate::bridge::BridgeErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    CognitiveMemoryBridge::new(Box::new(transport))
}
