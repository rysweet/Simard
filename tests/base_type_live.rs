//! Integration tests for Phase 3 real base type adapters.

use serde_json::json;

use simard::base_type_copilot::{CopilotAdapterConfig, CopilotSdkAdapter};
use simard::base_type_harness::{HarnessConfig, RealLocalHarnessAdapter};
use simard::base_type_turn::{
    TurnContext, format_turn_input, parse_turn_output, prepare_turn_context,
};
use simard::base_types::{
    BaseTypeCapability, BaseTypeFactory, BaseTypeSessionRequest, BaseTypeTurnInput,
};
use simard::bridge::BridgeErrorPayload;
use simard::bridge_subprocess::InMemoryBridgeTransport;
use simard::identity::OperatingMode;
use simard::knowledge_bridge::{KnowledgeBridge, KnowledgeQueryResult, KnowledgeSource};
use simard::memory_bridge::CognitiveMemoryBridge;
use simard::memory_cognitive::{CognitiveFact, CognitiveProcedure};
use simard::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
use simard::session::SessionId;

fn test_request() -> BaseTypeSessionRequest {
    BaseTypeSessionRequest {
        session_id: SessionId::from_uuid(uuid::Uuid::now_v7()),
        mode: OperatingMode::Engineer,
        topology: RuntimeTopology::SingleProcess,
        prompt_assets: vec![],
        runtime_node: RuntimeNodeId::new("test-node"),
        mailbox_address: RuntimeAddress::new("test-addr"),
    }
}

fn mock_memory_bridge() -> CognitiveMemoryBridge {
    let transport = InMemoryBridgeTransport::new("test-memory", |method, params| match method {
        "memory.search_facts" => {
            let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
            Ok(
                json!({"facts": [{"node_id": "sem_001", "concept": "testing",
                "content": format!("relevant fact about '{query}'"),
                "confidence": 0.85, "source_id": "src_1", "tags": ["test"]}]}),
            )
        }
        "memory.recall_procedure" => Ok(json!({"procedures": [{"node_id": "proc_001",
            "name": "build-and-test", "steps": ["cargo build", "cargo test"],
            "prerequisites": ["rust toolchain"], "usage_count": 5}]})),
        _ => Err(BridgeErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    CognitiveMemoryBridge::new(Box::new(transport))
}

fn mock_knowledge_bridge() -> KnowledgeBridge {
    let transport = InMemoryBridgeTransport::new("test-knowledge", |method, params| match method {
        "knowledge.list_packs" => Ok(json!([{"name": "rust-expert",
            "description": "Rust programming knowledge",
            "article_count": 120, "section_count": 450}])),
        "knowledge.query" => {
            let q = params
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Ok(json!({"answer": format!("Knowledge about '{q}'"),
                "sources": [{"title": "Rust Guide", "section": "Overview",
                    "url": "https://example.com/rust"}], "confidence": 0.9}))
        }
        _ => Err(BridgeErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    KnowledgeBridge::new(Box::new(transport))
}

// -- Turn context preparation --

#[test]
fn prepare_context_with_both_bridges() {
    let memory = mock_memory_bridge();
    let knowledge = mock_knowledge_bridge();
    let context = prepare_turn_context("implement error handling", Some(&memory), Some(&knowledge));
    assert_eq!(context.objective, "implement error handling");
    assert!(!context.memory_facts.is_empty());
    assert_eq!(context.memory_facts[0].concept, "testing");
    assert!(!context.procedures.is_empty());
    assert_eq!(context.procedures[0].name, "build-and-test");
}

#[test]
fn prepare_context_without_bridges() {
    let context = prepare_turn_context("do something", None, None);
    assert!(context.memory_facts.is_empty());
    assert!(context.procedures.is_empty());
    assert!(context.knowledge.is_empty());
}

#[test]
fn prepare_context_with_only_memory() {
    let memory = mock_memory_bridge();
    let context = prepare_turn_context("test objective", Some(&memory), None);
    assert!(!context.memory_facts.is_empty());
    assert!(context.knowledge.is_empty());
}

// -- Turn input formatting --

#[test]
fn format_minimal_context() {
    let ctx = TurnContext {
        objective: "build the widget".to_string(),
        memory_facts: vec![],
        knowledge: vec![],
        procedures: vec![],
        degraded_sources: vec![],
    };
    let prompt = format_turn_input(&ctx);
    assert!(prompt.contains("## Objective"));
    assert!(prompt.contains("build the widget"));
    assert!(prompt.contains("## Instructions"));
    assert!(!prompt.contains("## Relevant Memory Facts"));
    assert!(!prompt.contains("## Known Procedures"));
}

#[test]
fn format_full_context() {
    let ctx = TurnContext {
        objective: "optimize the query".to_string(),
        memory_facts: vec![CognitiveFact {
            node_id: "n1".into(),
            concept: "indexing".into(),
            content: "B-tree indexes speed up lookups".into(),
            confidence: 0.92,
            source_id: "s1".into(),
            tags: vec!["database".into()],
        }],
        knowledge: vec![KnowledgeQueryResult {
            answer: "Use EXPLAIN to analyze query plans.".into(),
            sources: vec![KnowledgeSource {
                title: "SQL Guide".into(),
                section: "Query Optimization".into(),
                url: Some("https://example.com/sql".into()),
            }],
            confidence: 0.88,
        }],
        procedures: vec![CognitiveProcedure {
            node_id: "p1".into(),
            name: "query-tune".into(),
            steps: vec!["run EXPLAIN".into(), "add index".into()],
            prerequisites: vec!["database access".into()],
            usage_count: 3,
        }],
        degraded_sources: vec![],
    };
    let prompt = format_turn_input(&ctx);
    assert!(prompt.contains("[indexing]"));
    assert!(prompt.contains("## Known Procedures"));
    assert!(prompt.contains("## Domain Knowledge"));
    assert!(prompt.contains("SQL Guide"));
}

// -- Turn output parsing --

#[test]
fn parse_well_formed_output() {
    let raw = "ACTION: create \u{2014} Create the module\n\
               ACTION: test \u{2014} Write tests\n\
               EXPLANATION: Both needed.\nCONFIDENCE: 0.91";
    let output = parse_turn_output(raw).unwrap();
    assert_eq!(output.actions.len(), 2);
    assert_eq!(output.actions[0].kind, "create");
    assert!(output.explanation.contains("Both"));
    assert!((output.confidence.unwrap() - 0.91).abs() < f64::EPSILON);
}

#[test]
fn parse_output_with_hyphen_separator() {
    let raw = "ACTION: deploy - Deploy to staging\nCONFIDENCE: 0.7";
    let output = parse_turn_output(raw).unwrap();
    assert_eq!(output.actions[0].kind, "deploy");
}

#[test]
fn parse_output_case_insensitive() {
    let raw = "action: build \u{2014} Build the project\nexplanation: Needed.\nconfidence: 0.6";
    let output = parse_turn_output(raw).unwrap();
    assert_eq!(output.actions.len(), 1);
    assert!((output.confidence.unwrap() - 0.6).abs() < f64::EPSILON);
}

// -- Feral inputs --

#[test]
fn feral_empty_output() {
    assert!(parse_turn_output("").is_err());
    assert!(parse_turn_output("   \n  \n  ").is_err());
}

#[test]
fn feral_malformed_output_still_extracts_something() {
    let output = parse_turn_output("Just random text.").unwrap();
    assert!(output.actions.is_empty());
    assert!(!output.explanation.is_empty());
    assert!(
        output.confidence.is_none(),
        "missing CONFIDENCE line should yield None, not a default"
    );
}

#[test]
fn feral_confidence_out_of_range() {
    let o1 = parse_turn_output("CONFIDENCE: -5.0").unwrap();
    assert!((o1.confidence.unwrap() - 0.0).abs() < f64::EPSILON);
    let o2 = parse_turn_output("CONFIDENCE: 999.0").unwrap();
    assert!((o2.confidence.unwrap() - 1.0).abs() < f64::EPSILON);
}

#[test]
fn feral_confidence_non_numeric() {
    let output = parse_turn_output("CONFIDENCE: very-high").unwrap();
    assert!(
        output.confidence.is_none(),
        "unparseable CONFIDENCE should yield None, not a default"
    );
}

// -- Copilot adapter contract tests --

#[test]
fn copilot_adapter_descriptor() {
    let adapter = CopilotSdkAdapter::registered("copilot-cap").unwrap();
    let desc = adapter.descriptor();
    assert!(
        desc.capabilities
            .contains(&BaseTypeCapability::TerminalSession)
    );
    assert!(desc.capabilities.contains(&BaseTypeCapability::Memory));
    assert!(
        desc.supported_topologies
            .contains(&RuntimeTopology::SingleProcess)
    );
    assert!(
        !desc
            .supported_topologies
            .contains(&RuntimeTopology::MultiProcess)
    );
}

#[test]
fn copilot_adapter_with_custom_config() {
    let config = CopilotAdapterConfig {
        command: "my-custom-copilot".into(),
        working_directory: Some("/home/user/project".into()),
    };
    let adapter = CopilotSdkAdapter::with_config("copilot-custom", config).unwrap();
    assert_eq!(adapter.config().command, "my-custom-copilot");
    assert_eq!(
        adapter.config().working_directory.as_deref(),
        Some("/home/user/project")
    );
}

#[test]
fn copilot_session_lifecycle_enforcement() {
    let adapter = CopilotSdkAdapter::registered("copilot-enforce").unwrap();
    let mut session = adapter.open_session(test_request()).unwrap();

    // Cannot run turn or close before open.
    assert!(
        session
            .run_turn(BaseTypeTurnInput {
                objective: "t".into()
            })
            .is_err()
    );
    assert!(session.close().is_err());

    session.open().unwrap();
    assert!(session.open().is_err()); // double open
    session.close().unwrap();

    // All operations after close fail.
    assert!(session.open().is_err());
    assert!(
        session
            .run_turn(BaseTypeTurnInput {
                objective: "t".into()
            })
            .is_err()
    );
}

// -- Harness adapter contract tests --

#[test]
fn harness_adapter_descriptor() {
    let adapter = RealLocalHarnessAdapter::registered("harness-cap").unwrap();
    let desc = adapter.descriptor();
    assert!(
        desc.capabilities
            .contains(&BaseTypeCapability::TerminalSession)
    );
    assert!(desc.capabilities.contains(&BaseTypeCapability::Evidence));
    assert!(!desc.capabilities.contains(&BaseTypeCapability::Memory));
    assert!(!desc.capabilities.contains(&BaseTypeCapability::Reflection));
}

#[test]
fn harness_adapter_with_custom_config() {
    let config = HarnessConfig {
        command: Some("cat".into()),
        shell: Some("/bin/sh".into()),
        working_directory: Some("/tmp".into()),
    };
    let adapter = RealLocalHarnessAdapter::with_config("harness-custom", config).unwrap();
    assert_eq!(adapter.config().command.as_deref(), Some("cat"));
    assert_eq!(adapter.config().shell.as_deref(), Some("/bin/sh"));
}

#[test]
fn harness_session_lifecycle_enforcement() {
    let adapter = RealLocalHarnessAdapter::registered("harness-enforce").unwrap();
    let mut session = adapter.open_session(test_request()).unwrap();

    assert!(
        session
            .run_turn(BaseTypeTurnInput {
                objective: "echo hi".into()
            })
            .is_err()
    );
    session.open().unwrap();
    session.close().unwrap();
    assert!(session.open().is_err());
}

#[test]
fn harness_adapter_rejects_multi_process_topology() {
    let adapter = RealLocalHarnessAdapter::registered("harness-topo").unwrap();
    let mut req = test_request();
    req.topology = RuntimeTopology::MultiProcess;
    assert!(adapter.open_session(req).is_err());
}

// -- Round-trip: prepare -> format -> parse --

#[test]
fn full_turn_round_trip() {
    let memory = mock_memory_bridge();
    let knowledge = mock_knowledge_bridge();
    let context = prepare_turn_context(
        "implement error handling in Rust",
        Some(&memory),
        Some(&knowledge),
    );
    let prompt = format_turn_input(&context);
    assert!(prompt.contains("implement error handling in Rust"));

    let simulated = "ACTION: create \u{2014} Create error.rs module\n\
                     ACTION: test \u{2014} Add error handling tests\n\
                     EXPLANATION: The module needs proper error types.\n\
                     CONFIDENCE: 0.88";
    let output = parse_turn_output(simulated).unwrap();
    assert_eq!(output.actions.len(), 2);
    assert!((output.confidence.unwrap() - 0.88).abs() < f64::EPSILON);
}
