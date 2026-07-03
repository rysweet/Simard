//! Adapter-parameterised enrichment tests (issue #1665).
//!
//! Before #1665 only `CopilotSdkAdapter` invoked `prepare_turn_context`, so the
//! other shipped base-type adapters (local-harness, rusty-clawd,
//! claude-agent-sdk, ms-agent-framework) ran with empty memory/knowledge
//! context. These tests assert that *every* shipped adapter now renders the
//! same `## Relevant Memory Facts` block through the shared, normalized
//! `BaseTypeSession::enrich_input` entry point when fed an identical
//! mock-connected objective.

use serde_json::json;

use simard::base_types::{
    BaseTypeFactory, BaseTypeSession, BaseTypeSessionRequest, BaseTypeTurnInput,
};
use simard::identity::OperatingMode;
use simard::memory_adapter::CognitiveMemoryAdapter;
use simard::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
use simard::server_subprocess::InMemoryServerTransport;
use simard::server_transport::ServerErrorPayload;
use simard::session::SessionId;
use simard::{
    CognitiveMemoryOps, CopilotSdkAdapter, RealLocalHarnessAdapter, RustyClawdAdapter,
    claude_agent_sdk_adapter, ms_agent_framework_adapter,
};

const OBJECTIVE: &str = "implement error handling";

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

/// Mock memory adapter mirroring `tests/base_type_live.rs`: returns a single
/// fact and a single procedure for any query so the rendered prompt is
/// deterministic across adapters.
fn mock_memory_adapter() -> Box<dyn CognitiveMemoryOps> {
    let transport = InMemoryServerTransport::new("test-memory", |method, params| match method {
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
        "memory.search_episodes_by_keywords" => Ok(json!({"episodes": []})),
        _ => Err(ServerErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    Box::new(CognitiveMemoryAdapter::new(Box::new(transport)))
}

/// Construct every shipped base-type adapter session, named for diagnostics.
fn all_shipped_sessions() -> Vec<(&'static str, Box<dyn BaseTypeSession>)> {
    let copilot = CopilotSdkAdapter::registered("copilot-sdk")
        .unwrap()
        .open_session(test_request())
        .unwrap();
    let harness = RealLocalHarnessAdapter::registered("local-harness")
        .unwrap()
        .open_session(test_request())
        .unwrap();
    let rustyclawd = RustyClawdAdapter::registered("rusty-clawd")
        .unwrap()
        .open_session(test_request())
        .unwrap();
    let claude = claude_agent_sdk_adapter("claude-agent-sdk")
        .unwrap()
        .open_session(test_request())
        .unwrap();
    let ms_agent = ms_agent_framework_adapter("ms-agent-framework")
        .unwrap()
        .open_session(test_request())
        .unwrap();

    vec![
        ("copilot-sdk", copilot),
        ("local-harness", harness),
        ("rusty-clawd", rustyclawd),
        ("claude-agent-sdk", claude),
        ("ms-agent-framework", ms_agent),
    ]
}

#[test]
fn every_shipped_adapter_exposes_enrichment() {
    for (name, session) in all_shipped_sessions() {
        assert!(
            session.enrichment().is_some(),
            "adapter '{name}' must expose enrichment adapters"
        );
    }
}

#[test]
fn every_shipped_adapter_renders_memory_facts_with_adapter() {
    let input = BaseTypeTurnInput::objective_only(OBJECTIVE);

    for (name, mut session) in all_shipped_sessions() {
        session
            .enrichment_mut()
            .unwrap_or_else(|| panic!("adapter '{name}' must support enrichment injection"))
            .memory = Some(mock_memory_adapter());

        let enriched = session
            .enrich_input(&input)
            .unwrap_or_else(|e| panic!("adapter '{name}' enrich_input failed: {e}"));

        // The recalled memory + procedures are injected into prompt_preamble
        // (the per-turn system/preamble context), leaving the objective bare.
        let preamble = &enriched.prompt_preamble;
        assert!(
            preamble.contains("## Relevant Memory Facts"),
            "adapter '{name}' must render the memory facts block, got:\n{preamble}"
        );
        assert!(
            preamble.contains("[testing]"),
            "adapter '{name}' must render the recalled fact concept"
        );
        assert!(
            preamble.contains(&format!("relevant fact about '{OBJECTIVE}'")),
            "adapter '{name}' must render the recalled fact content"
        );
        assert!(
            preamble.contains("## Known Procedures"),
            "adapter '{name}' must render recalled procedures"
        );
        // The objective is preserved unchanged (kept out of the enrichment).
        assert_eq!(
            enriched.objective, OBJECTIVE,
            "adapter '{name}' must keep the objective bare"
        );
    }
}

#[test]
fn all_shipped_adapters_render_identical_enriched_prompt() {
    let input = BaseTypeTurnInput::objective_only(OBJECTIVE);

    let mut rendered: Vec<(String, String)> = Vec::new();
    for (name, mut session) in all_shipped_sessions() {
        session.enrichment_mut().unwrap().memory = Some(mock_memory_adapter());
        let enriched = session.enrich_input(&input).unwrap();
        rendered.push((name.to_string(), enriched.prompt_preamble));
    }

    let (baseline_name, baseline) = &rendered[0];
    for (name, preamble) in &rendered[1..] {
        assert_eq!(
            preamble, baseline,
            "adapter '{name}' produced a different enrichment block than '{baseline_name}'; \
             enrichment must be normalized across all adapters (#1665)"
        );
    }
}

#[test]
fn enrichment_is_noop_without_configured_adapter() {
    let input = BaseTypeTurnInput::objective_only(OBJECTIVE);

    for (name, session) in all_shipped_sessions() {
        // No adapter injected => input returned unchanged, no memory section.
        let enriched = session.enrich_input(&input).unwrap();
        assert_eq!(
            enriched.objective, OBJECTIVE,
            "adapter '{name}' must still preserve the objective without adapters"
        );
        assert!(
            !enriched
                .prompt_preamble
                .contains("## Relevant Memory Facts"),
            "adapter '{name}' must not fabricate memory facts when no adapter is configured"
        );
    }
}

/// Production-wiring parity gate (issue #2383).
///
/// The cross-adapter tests above inject a *mock* adapter through
/// `enrichment_mut`, which proves `enrich_input` consumes adapters but not that
/// the production factory seam (`with_enrichment` + `open_session`) actually
/// launches them. Before #2383 only `CopilotSdkAdapter` exposed
/// `with_enrichment`; RustyClawd's `SessionBuilder` arm built sessions with
/// empty adapters, so enrichment was inert in production.
///
/// This drives both production-wired adapters through their `with_enrichment`
/// builders against a real, writable state root and asserts they launch
/// identical, non-empty memory + knowledge adapters — the parity that must hold
/// so RustyClawd cannot silently regress to no-enrichment again. The
/// `SessionBuilder`-level seam itself (the line that actually regressed) is
/// covered by `session_builder`'s `*_provider_wires_enrichment_*` tests.
#[test]
#[serial_test::serial(cognitive_memory)]
fn production_wiring_launches_adapters_for_builder_adapters() {
    use std::path::PathBuf;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let state_root = tmp.path().join("state");
    std::fs::create_dir_all(&state_root).unwrap();

    fn assert_wired(name: &str, session: &dyn BaseTypeSession) {
        let adapters = session
            .enrichment()
            .unwrap_or_else(|| panic!("adapter '{name}' must expose enrichment adapters"));
        assert!(
            adapters.memory.is_some(),
            "adapter '{name}' production wiring must launch the memory adapter"
        );
        assert!(
            adapters.knowledge.is_some(),
            "adapter '{name}' production wiring must launch the knowledge adapter"
        );
    }

    let build = |state_root: PathBuf| -> Vec<(&'static str, Box<dyn BaseTypeSession>)> {
        let copilot = CopilotSdkAdapter::registered("copilot-sdk")
            .unwrap()
            .with_enrichment(state_root.clone())
            .open_session(test_request())
            .unwrap();
        let rustyclawd = RustyClawdAdapter::registered("rusty-clawd")
            .unwrap()
            .with_enrichment(state_root)
            .open_session(test_request())
            .unwrap();
        vec![("copilot-sdk", copilot), ("rusty-clawd", rustyclawd)]
    };

    for (name, session) in build(state_root) {
        assert_wired(name, session.as_ref());
    }
}
