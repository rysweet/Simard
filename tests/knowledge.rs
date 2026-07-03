//! Integration tests for the knowledge adapter and planning context enrichment.
//!
//! These tests use an in-memory adapter transport to verify the full contract
//! without requiring a running Python adapter server or installed knowledge packs.

use simard::knowledge_client::{KnowledgeClient, KnowledgePackInfo};
use simard::knowledge_context::{PlanningContext, enrich_planning_context};
use simard::server_subprocess::InMemoryServerTransport;
use simard::server_transport::{SERVER_ERROR_METHOD_NOT_FOUND, ServerErrorPayload};

/// Build a mock transport that simulates the Python knowledge adapter server.
fn mock_knowledge_transport() -> InMemoryServerTransport {
    InMemoryServerTransport::new("knowledge-integration", |method, params| match method {
        "bridge.health" => Ok(serde_json::json!({
            "server_name": "simard-knowledge",
            "healthy": true,
        })),
        "knowledge.query" => {
            let pack = params
                .get("pack_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let question = params
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if pack == "unknown-pack" {
                return Err(ServerErrorPayload {
                    code: -32603,
                    message: format!("pack '{pack}' not found"),
                });
            }
            if question.is_empty() {
                return Ok(serde_json::json!({
                    "answer": "Please provide a question.",
                    "sources": [],
                    "confidence": 0.0,
                }));
            }
            Ok(serde_json::json!({
                "answer": format!("Here is what I know about '{question}' from the {pack} pack."),
                "sources": [
                    {
                        "title": format!("{pack} Core Concepts"),
                        "section": "Overview",
                        "url": format!("https://example.com/{pack}/overview"),
                    },
                    {
                        "title": format!("{pack} Advanced Topics"),
                        "section": "Details",
                    },
                ],
                "confidence": 0.82,
            }))
        }
        "knowledge.list_packs" => Ok(serde_json::json!({
            "packs": [
                {
                    "name": "rust-expert",
                    "description": "Rust programming language ownership borrowing lifetimes",
                    "article_count": 150,
                    "section_count": 520,
                },
                {
                    "name": "python-expert",
                    "description": "Python programming language stdlib asyncio",
                    "article_count": 210,
                    "section_count": 890,
                },
                {
                    "name": "docker-expert",
                    "description": "Docker containers images Dockerfile",
                    "article_count": 75,
                    "section_count": 280,
                },
                {
                    "name": "kubernetes-expert",
                    "description": "Kubernetes k8s pods deployments services",
                    "article_count": 180,
                    "section_count": 650,
                },
            ]
        })),
        "knowledge.pack_info" => {
            let pack = params
                .get("pack_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match pack {
                "rust-expert" => Ok(serde_json::json!({
                    "name": "rust-expert",
                    "description": "Rust programming language ownership borrowing lifetimes",
                    "article_count": 150,
                    "section_count": 520,
                })),
                "python-expert" => Ok(serde_json::json!({
                    "name": "python-expert",
                    "description": "Python programming language stdlib asyncio",
                    "article_count": 210,
                    "section_count": 890,
                })),
                _ => Err(ServerErrorPayload {
                    code: -32603,
                    message: format!("pack '{pack}' not found"),
                }),
            }
        }
        _ => Err(ServerErrorPayload {
            code: SERVER_ERROR_METHOD_NOT_FOUND,
            message: format!("unknown method: {method}"),
        }),
    })
}

// ---------------------------------------------------------------------------
// knowledge.list_packs
// ---------------------------------------------------------------------------

#[test]
fn list_packs_returns_all_available() {
    let adapter = KnowledgeClient::new(Box::new(mock_knowledge_transport()));
    let packs = adapter.list_packs().unwrap();
    assert_eq!(packs.len(), 4);

    let names: Vec<&str> = packs.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"rust-expert"));
    assert!(names.contains(&"python-expert"));
    assert!(names.contains(&"docker-expert"));
    assert!(names.contains(&"kubernetes-expert"));
}

#[test]
fn list_packs_includes_article_counts() {
    let adapter = KnowledgeClient::new(Box::new(mock_knowledge_transport()));
    let packs = adapter.list_packs().unwrap();
    let rust = packs.iter().find(|p| p.name == "rust-expert").unwrap();
    assert_eq!(rust.article_count, 150);
    assert_eq!(rust.section_count, 520);
}

// ---------------------------------------------------------------------------
// knowledge.query
// ---------------------------------------------------------------------------

#[test]
fn query_pack_returns_answer_and_sources() {
    let adapter = KnowledgeClient::new(Box::new(mock_knowledge_transport()));
    let result = adapter
        .query("rust-expert", "What is ownership?", 10)
        .unwrap();

    assert!(result.answer.contains("ownership"));
    assert_eq!(result.sources.len(), 2);
    assert_eq!(result.sources[0].title, "rust-expert Core Concepts");
    assert_eq!(result.sources[0].section, "Overview");
    assert!(result.sources[0].url.is_some());
    assert!(result.sources[1].url.is_none());
    assert!((result.confidence - 0.82).abs() < 0.01);
}

#[test]
fn query_unknown_pack_returns_error() {
    let adapter = KnowledgeClient::new(Box::new(mock_knowledge_transport()));
    let result = adapter.query("unknown-pack", "anything", 5);
    assert!(result.is_err());
    let error_msg = result.unwrap_err().to_string();
    assert!(error_msg.contains("not found"), "got: {error_msg}");
}

#[test]
fn query_empty_question_returns_zero_confidence() {
    let adapter = KnowledgeClient::new(Box::new(mock_knowledge_transport()));
    let result = adapter.query("rust-expert", "", 5).unwrap();
    assert!(result.confidence < f64::EPSILON);
    assert!(result.sources.is_empty());
    assert!(!result.answer.is_empty()); // Should still have a placeholder answer.
}

// ---------------------------------------------------------------------------
// knowledge.pack_info
// ---------------------------------------------------------------------------

#[test]
fn pack_info_returns_metadata() {
    let adapter = KnowledgeClient::new(Box::new(mock_knowledge_transport()));
    let info = adapter.pack_info("rust-expert").unwrap();
    assert_eq!(info.name, "rust-expert");
    assert_eq!(
        info.description,
        "Rust programming language ownership borrowing lifetimes"
    );
    assert_eq!(info.article_count, 150);
    assert_eq!(info.section_count, 520);
}

#[test]
fn pack_info_unknown_returns_error() {
    let adapter = KnowledgeClient::new(Box::new(mock_knowledge_transport()));
    let result = adapter.pack_info("nonexistent");
    assert!(result.is_err());
    let error_msg = result.unwrap_err().to_string();
    assert!(error_msg.contains("not found"), "got: {error_msg}");
}

// ---------------------------------------------------------------------------
// health
// ---------------------------------------------------------------------------

#[test]
fn health_check_reports_healthy() {
    let adapter = KnowledgeClient::new(Box::new(mock_knowledge_transport()));
    let health = adapter.health().unwrap();
    assert_eq!(health.server_name, "simard-knowledge");
    assert!(health.healthy);
}

// ---------------------------------------------------------------------------
// Planning context enrichment
// ---------------------------------------------------------------------------

#[test]
fn enrich_context_picks_relevant_packs_for_rust_objective() {
    let adapter = KnowledgeClient::new(Box::new(mock_knowledge_transport()));
    let ctx =
        enrich_planning_context("Fix Rust ownership bug in the borrow checker", &adapter).unwrap();

    assert!(!ctx.is_empty());
    assert!(
        ctx.pack_sources.contains(&"rust-expert".to_string()),
        "expected rust-expert in sources: {:?}",
        ctx.pack_sources,
    );
}

#[test]
fn enrich_context_picks_docker_for_container_objective() {
    let adapter = KnowledgeClient::new(Box::new(mock_knowledge_transport()));
    let ctx = enrich_planning_context(
        "Build Docker containers for the microservice deployment",
        &adapter,
    )
    .unwrap();

    assert!(!ctx.is_empty());
    assert!(
        ctx.pack_sources.contains(&"docker-expert".to_string()),
        "expected docker-expert in sources: {:?}",
        ctx.pack_sources,
    );
}

#[test]
fn enrich_context_returns_empty_for_unrelated_objective() {
    let adapter = KnowledgeClient::new(Box::new(mock_knowledge_transport()));
    let ctx = enrich_planning_context("xyzzy plugh frobnicate", &adapter).unwrap();
    assert!(ctx.is_empty());
    assert!(ctx.pack_sources.is_empty());
}

#[test]
fn enrich_context_caps_at_three_packs() {
    let adapter = KnowledgeClient::new(Box::new(mock_knowledge_transport()));
    // Objective that matches all four packs.
    let ctx = enrich_planning_context(
        "Deploy Rust Python Docker Kubernetes containers pods programming language",
        &adapter,
    )
    .unwrap();
    assert!(
        ctx.pack_sources.len() <= 3,
        "expected at most 3 packs, got {}",
        ctx.pack_sources.len(),
    );
}

#[test]
fn enrich_context_returns_error_on_adapter_failure() {
    let failing = InMemoryServerTransport::new("knowledge-fail", |_method, _params| {
        Err(ServerErrorPayload {
            code: -32603,
            message: "adapter is down".to_string(),
        })
    });
    let adapter = KnowledgeClient::new(Box::new(failing));
    let result = enrich_planning_context("Fix Rust bug", &adapter);
    assert!(
        result.is_err(),
        "should propagate adapter error, not silently degrade"
    );
}

// ---------------------------------------------------------------------------
// Serialization round-trip
// ---------------------------------------------------------------------------

#[test]
fn knowledge_pack_info_serializes_roundtrip() {
    let info = KnowledgePackInfo {
        name: "test-pack".to_string(),
        description: "Test pack".to_string(),
        article_count: 42,
        section_count: 100,
    };
    let json = serde_json::to_string(&info).unwrap();
    let parsed: KnowledgePackInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.name, "test-pack");
    assert_eq!(parsed.article_count, 42);
}

#[test]
fn planning_context_empty_check() {
    let empty = PlanningContext {
        relevant_knowledge: vec![],
        pack_sources: vec![],
    };
    assert!(empty.is_empty());
}
