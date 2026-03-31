//! Knowledge Graph Pack bridge for querying agent-kgpacks from Simard.
//!
//! This module wraps a [`BridgeTransport`] to provide a typed interface for
//! querying knowledge graph packs, listing available packs, and retrieving
//! pack metadata. The Python side (`simard_knowledge_bridge.py`) handles the
//! actual KnowledgeGraphAgent and PackRegistry interactions.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bridge::{
    BridgeHealth, BridgeRequest, BridgeTransport, new_request_id, unpack_bridge_response,
};
use crate::error::SimardResult;

/// Name used in error messages for this bridge.
const BRIDGE_NAME: &str = "knowledge";

/// Wire-protocol wrapper for `list_packs` response consistency.
#[derive(Deserialize)]
struct ListPacksResponse {
    packs: Vec<KnowledgePackInfo>,
}

/// Result of querying a knowledge pack with a natural-language question.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgeQueryResult {
    /// The synthesized natural-language answer.
    pub answer: String,
    /// Sources that contributed to the answer.
    pub sources: Vec<KnowledgeSource>,
    /// Confidence score in [0.0, 1.0].
    pub confidence: f64,
}

/// A single source citation from a knowledge pack query.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgeSource {
    /// Article or document title.
    pub title: String,
    /// Section within the article.
    pub section: String,
    /// Optional URL for the source material.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Metadata about an installed knowledge pack.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgePackInfo {
    /// Pack name (e.g. "rust-expert", "python-expert").
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Number of articles in the pack.
    pub article_count: u32,
    /// Number of sections across all articles.
    pub section_count: u32,
}

/// Typed client for the knowledge graph pack bridge.
///
/// All methods delegate to the underlying [`BridgeTransport`] using the
/// bridge JSON-line protocol. The Python bridge server maps these to
/// KnowledgeGraphAgent and PackRegistry calls.
pub struct KnowledgeBridge {
    transport: Box<dyn BridgeTransport>,
}

impl KnowledgeBridge {
    /// Create a new knowledge bridge wrapping the given transport.
    pub fn new(transport: Box<dyn BridgeTransport>) -> Self {
        Self { transport }
    }

    /// Query a knowledge pack with a natural-language question.
    ///
    /// # Arguments
    /// - `pack_name`: name of the pack to query (e.g. "rust-expert")
    /// - `question`: natural-language question
    /// - `limit`: maximum number of sources to return
    pub fn query(
        &self,
        pack_name: &str,
        question: &str,
        limit: u32,
    ) -> SimardResult<KnowledgeQueryResult> {
        let params = serde_json::json!({
            "pack_name": pack_name,
            "question": question,
            "limit": limit,
        });
        let response = self.call("knowledge.query", params)?;
        unpack_bridge_response(BRIDGE_NAME, "knowledge.query", response)
    }

    /// List all available knowledge packs.
    pub fn list_packs(&self) -> SimardResult<Vec<KnowledgePackInfo>> {
        let response = self.call("knowledge.list_packs", serde_json::json!({}))?;
        let wrapper: ListPacksResponse =
            unpack_bridge_response(BRIDGE_NAME, "knowledge.list_packs", response)?;
        Ok(wrapper.packs)
    }

    /// Get metadata for a specific pack.
    pub fn pack_info(&self, pack_name: &str) -> SimardResult<KnowledgePackInfo> {
        let params = serde_json::json!({ "pack_name": pack_name });
        let response = self.call("knowledge.pack_info", params)?;
        unpack_bridge_response(BRIDGE_NAME, "knowledge.pack_info", response)
    }

    /// Check whether the bridge server is alive and responsive.
    pub fn health(&self) -> SimardResult<BridgeHealth> {
        self.transport.health()
    }

    fn call(&self, method: &str, params: Value) -> SimardResult<crate::bridge::BridgeResponse> {
        let request = BridgeRequest {
            id: new_request_id(),
            method: method.to_string(),
            params,
        };
        self.transport.call(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{BRIDGE_ERROR_METHOD_NOT_FOUND, BridgeErrorPayload};
    use crate::bridge_subprocess::InMemoryBridgeTransport;

    fn mock_transport() -> InMemoryBridgeTransport {
        InMemoryBridgeTransport::new("knowledge-test", |method, params| match method {
            "bridge.health" => Ok(serde_json::json!({
                "server_name": "simard-knowledge",
                "healthy": true,
            })),
            "knowledge.query" => {
                let pack = params
                    .get("pack_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if pack == "nonexistent" {
                    return Err(BridgeErrorPayload {
                        code: -32603,
                        message: format!("pack '{pack}' not found"),
                    });
                }
                let question = params
                    .get("question")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if question.is_empty() {
                    return Ok(serde_json::json!({
                        "answer": "Please provide a question.",
                        "sources": [],
                        "confidence": 0.0,
                    }));
                }
                Ok(serde_json::json!({
                    "answer": format!("Answer about '{question}' from pack '{pack}'."),
                    "sources": [{
                        "title": "Test Article",
                        "section": "Overview",
                        "url": "https://example.com/test"
                    }],
                    "confidence": 0.85,
                }))
            }
            "knowledge.list_packs" => Ok(serde_json::json!({
                "packs": [
                    {
                        "name": "rust-expert",
                        "description": "Rust programming knowledge",
                        "article_count": 120,
                        "section_count": 450,
                    },
                    {
                        "name": "python-expert",
                        "description": "Python programming knowledge",
                        "article_count": 200,
                        "section_count": 800,
                    },
                ]
            })),
            "knowledge.pack_info" => {
                let pack = params
                    .get("pack_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if pack == "nonexistent" {
                    return Err(BridgeErrorPayload {
                        code: -32603,
                        message: format!("pack '{pack}' not found"),
                    });
                }
                Ok(serde_json::json!({
                    "name": pack,
                    "description": format!("{pack} knowledge"),
                    "article_count": 120,
                    "section_count": 450,
                }))
            }
            _ => Err(BridgeErrorPayload {
                code: BRIDGE_ERROR_METHOD_NOT_FOUND,
                message: format!("unknown method: {method}"),
            }),
        })
    }

    #[test]
    fn query_returns_typed_result() {
        let bridge = KnowledgeBridge::new(Box::new(mock_transport()));
        let result = bridge
            .query("rust-expert", "What is ownership?", 5)
            .unwrap();
        assert!(result.answer.contains("ownership"));
        assert_eq!(result.sources.len(), 1);
        assert_eq!(result.sources[0].title, "Test Article");
        assert!((result.confidence - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn query_unknown_pack_returns_error() {
        let bridge = KnowledgeBridge::new(Box::new(mock_transport()));
        let result = bridge.query("nonexistent", "anything", 5);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not found"));
    }

    #[test]
    fn query_empty_question_returns_low_confidence() {
        let bridge = KnowledgeBridge::new(Box::new(mock_transport()));
        let result = bridge.query("rust-expert", "", 5).unwrap();
        assert!(result.confidence < f64::EPSILON);
        assert!(result.sources.is_empty());
    }

    #[test]
    fn list_packs_returns_all() {
        let bridge = KnowledgeBridge::new(Box::new(mock_transport()));
        let packs = bridge.list_packs().unwrap();
        assert_eq!(packs.len(), 2);
        assert_eq!(packs[0].name, "rust-expert");
        assert_eq!(packs[1].name, "python-expert");
    }

    #[test]
    fn pack_info_returns_metadata() {
        let bridge = KnowledgeBridge::new(Box::new(mock_transport()));
        let info = bridge.pack_info("rust-expert").unwrap();
        assert_eq!(info.name, "rust-expert");
        assert_eq!(info.article_count, 120);
    }

    #[test]
    fn pack_info_unknown_returns_error() {
        let bridge = KnowledgeBridge::new(Box::new(mock_transport()));
        let result = bridge.pack_info("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn health_check_succeeds() {
        let bridge = KnowledgeBridge::new(Box::new(mock_transport()));
        let health = bridge.health().unwrap();
        assert_eq!(health.server_name, "simard-knowledge");
        assert!(health.healthy);
    }
}
