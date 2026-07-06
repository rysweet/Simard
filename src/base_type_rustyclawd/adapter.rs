use std::path::PathBuf;

use crate::base_type_turn::EnrichmentSource;
use crate::base_types::{
    BaseTypeDescriptor, BaseTypeFactory, BaseTypeId, BaseTypeSession, BaseTypeSessionRequest,
    standard_session_capabilities,
};
use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;

use super::session::RustyClawdSession;

#[derive(Debug)]
pub struct RustyClawdAdapter {
    pub(super) descriptor: BaseTypeDescriptor,
    /// Where this adapter sources per-turn memory + knowledge enrichment.
    ///
    /// [`EnrichmentSource::Disabled`] by default so lightweight callers and
    /// unit tests incur no filesystem side effects; the live production path
    /// ([`crate::session_builder::SessionBuilder`]) opts in via
    /// [`RustyClawdAdapter::with_enrichment`] (issue #2383).
    enrichment: EnrichmentSource,
}

impl RustyClawdAdapter {
    pub fn registered(id: impl Into<String>) -> SimardResult<Self> {
        let id = BaseTypeId::new(id);
        Ok(Self {
            descriptor: BaseTypeDescriptor {
                id,
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "rusty-clawd::session-backend",
                    "registered-base-type:rusty-clawd",
                    Freshness::now()?,
                ),
                capabilities: standard_session_capabilities(),
                supported_topologies: [
                    RuntimeTopology::SingleProcess,
                    RuntimeTopology::MultiProcess,
                ]
                .into_iter()
                .collect(),
            },
            enrichment: EnrichmentSource::default(),
        })
    }

    /// Enable per-turn memory + knowledge enrichment for sessions opened by
    /// this adapter, reading cognitive memory from `state_root`.
    ///
    /// Without this, sessions are created with both bridges set to `None` and
    /// every turn runs through `enrich_input` with an empty
    /// [`crate::base_type_turn::EnrichmentClients`] — the no-op of issue #2383
    /// that left RustyClawd recalling no memory facts/procedures or domain
    /// knowledge in production even though the #1665 `enrich_input` entry point
    /// was already wired through `run_turn`.
    ///
    /// Mirrors [`crate::base_type_copilot::CopilotSdkAdapter::with_enrichment`]:
    /// bridges are launched lazily in [`BaseTypeFactory::open_session`] via the
    /// shared [`EnrichmentSource::resolve`]; a launch failure logs and degrades
    /// to `None` so a missing knowledge pack or an unavailable memory store
    /// never breaks turn dispatch.
    #[must_use]
    pub fn with_enrichment(mut self, state_root: PathBuf) -> Self {
        self.enrichment = EnrichmentSource::Native { state_root };
        self
    }
}

impl BaseTypeFactory for RustyClawdAdapter {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open_session(
        &self,
        request: BaseTypeSessionRequest,
    ) -> SimardResult<Box<dyn BaseTypeSession>> {
        if !self.descriptor.supports_topology(request.topology) {
            return Err(SimardError::UnsupportedTopology {
                base_type: self.descriptor.id.to_string(),
                topology: request.topology,
            });
        }

        // Issue #2383: populate the session's enrichment bridges from the
        // configured source (graceful degradation inside `resolve`) so the
        // shared `enrich_input` entry point actually injects memory facts,
        // procedures, and domain knowledge into every production turn — instead
        // of the empty `EnrichmentClients::new()` that made enrichment inert.
        Ok(Box::new(RustyClawdSession {
            descriptor: self.descriptor.clone(),
            request,
            is_open: false,
            is_closed: false,
            client: None,
            rt: None,
            conversation_history: Vec::new(),
            enrichment: self.enrichment.resolve(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::BaseTypeFactory;
    use crate::runtime::RuntimeTopology;

    // ── RustyClawdAdapter construction ──

    #[test]
    fn registered_adapter_has_correct_backend_identity() {
        let adapter = RustyClawdAdapter::registered("rusty-clawd").unwrap();
        assert_eq!(
            adapter.descriptor().backend.identity,
            "rusty-clawd::session-backend"
        );
    }

    #[test]
    fn registered_adapter_has_expected_id() {
        let adapter = RustyClawdAdapter::registered("my-id").unwrap();
        assert_eq!(adapter.descriptor().id.as_str(), "my-id");
    }

    #[test]
    fn registered_adapter_supports_single_and_multi_process() {
        let adapter = RustyClawdAdapter::registered("rc").unwrap();
        let desc = adapter.descriptor();
        assert!(desc.supports_topology(RuntimeTopology::SingleProcess));
        assert!(desc.supports_topology(RuntimeTopology::MultiProcess));
        assert!(!desc.supports_topology(RuntimeTopology::Distributed));
    }

    #[test]
    fn registered_adapter_has_standard_capabilities() {
        let adapter = RustyClawdAdapter::registered("rc").unwrap();
        let caps = &adapter.descriptor().capabilities;
        assert!(
            !caps.is_empty(),
            "should have standard session capabilities"
        );
    }

    #[test]
    fn descriptor_returns_reference_to_stored_descriptor() {
        let adapter = RustyClawdAdapter::registered("rc").unwrap();
        let d1 = adapter.descriptor();
        let d2 = adapter.descriptor();
        assert_eq!(d1.id, d2.id);
    }

    #[test]
    fn adapter_debug_format_contains_type_name() {
        let adapter = RustyClawdAdapter::registered("debug-test").unwrap();
        let debug = format!("{adapter:?}");
        assert!(debug.contains("RustyClawdAdapter"));
    }

    #[test]
    fn registered_adapter_with_empty_id() {
        let adapter = RustyClawdAdapter::registered("");
        assert!(adapter.is_ok(), "empty id should still construct");
        assert_eq!(adapter.unwrap().descriptor().id.as_str(), "");
    }

    #[test]
    fn registered_adapter_with_hyphenated_id() {
        let adapter = RustyClawdAdapter::registered("my-custom-agent-type").unwrap();
        assert_eq!(adapter.descriptor().id.as_str(), "my-custom-agent-type");
    }

    #[test]
    fn registered_adapter_backend_identity_is_stable() {
        let a1 = RustyClawdAdapter::registered("test1").unwrap();
        let a2 = RustyClawdAdapter::registered("test2").unwrap();
        assert_eq!(
            a1.descriptor().backend.identity,
            a2.descriptor().backend.identity,
            "backend identity should be the same regardless of adapter id"
        );
    }

    #[test]
    fn registered_adapter_does_not_support_distributed() {
        let adapter = RustyClawdAdapter::registered("rc").unwrap();
        assert!(
            !adapter
                .descriptor()
                .supports_topology(RuntimeTopology::Distributed),
        );
    }

    // ── open_session ──

    #[test]
    fn open_session_rejects_unsupported_topology() {
        use crate::base_types::BaseTypeSessionRequest;
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-test").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000001")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::Distributed, // not supported
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let result = adapter.open_session(request);
        assert!(result.is_err());
        match result {
            Err(SimardError::UnsupportedTopology {
                base_type,
                topology,
            }) => {
                assert_eq!(base_type, "rc-test");
                assert_eq!(topology, RuntimeTopology::Distributed);
            }
            Err(other) => panic!("expected UnsupportedTopology, got {other:?}"),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn open_session_succeeds_for_supported_topology() {
        use crate::base_types::BaseTypeSessionRequest;
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-test").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000002")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let result = adapter.open_session(request);
        assert!(result.is_ok());
    }

    #[test]
    fn open_session_succeeds_for_multi_process() {
        use crate::base_types::BaseTypeSessionRequest;
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-test").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000010")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::MultiProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let result = adapter.open_session(request);
        assert!(result.is_ok());
    }

    // ── Session lifecycle guards ──

    #[test]
    fn session_run_turn_before_open_fails() {
        use crate::base_types::{BaseTypeSessionRequest, BaseTypeTurnInput};
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-test").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000003")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let mut session = adapter.open_session(request).unwrap();

        let input = BaseTypeTurnInput {
            objective: "test".to_string(),
            identity_context: "".to_string(),
            prompt_preamble: "".to_string(),
        };
        let result = session.run_turn(input);
        assert!(result.is_err(), "run_turn before open should fail");
    }

    #[test]
    fn session_close_before_open_fails() {
        use crate::base_types::BaseTypeSessionRequest;
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-test").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000004")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let mut session = adapter.open_session(request).unwrap();
        let result = session.close();
        assert!(result.is_err(), "close before open should fail");
    }

    #[test]
    fn session_descriptor_matches_adapter_descriptor() {
        use crate::base_types::BaseTypeSessionRequest;
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-desc").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000011")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let session = adapter.open_session(request).unwrap();
        assert_eq!(
            session.descriptor().id.as_str(),
            adapter.descriptor().id.as_str()
        );
    }

    // ── Issue #2383: production memory + knowledge enrichment wiring ──

    fn enrichment_test_request() -> BaseTypeSessionRequest {
        use crate::base_types::BaseTypeSessionRequest;
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        BaseTypeSessionRequest {
            session_id: SessionId::from_uuid(uuid::Uuid::now_v7()),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::new("test-node"),
            mailbox_address: RuntimeAddress::new("test-addr"),
        }
    }

    /// Mock memory bridge mirroring `tests/base_type_enrichment.rs`: returns a
    /// single fact and procedure for any query so the rendered prompt is
    /// deterministic.
    fn mock_memory_client() -> Box<dyn crate::cognitive_memory::CognitiveMemoryOps> {
        use crate::memory_client::CognitiveMemoryClient;
        use crate::rpc::RpcErrorPayload;
        use crate::rpc_transport::InMemoryRpcTransport;
        use serde_json::json;

        let transport =
            InMemoryRpcTransport::new("rc-test-memory", |method, params| match method {
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
                _ => Err(RpcErrorPayload {
                    code: -32601,
                    message: format!("unknown method: {method}"),
                }),
            });
        Box::new(CognitiveMemoryClient::new(Box::new(transport)))
    }

    /// The production-wiring regression guard for issue #2383: a RustyClawd
    /// session built through `with_enrichment` (the same seam `SessionBuilder`
    /// uses) must have non-empty bridges, instead of the empty
    /// `EnrichmentClients::new()` that made enrichment a permanent no-op.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn production_session_with_enrichment_has_nonempty_bridges() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let state_root = tmp.path().join("state");
        std::fs::create_dir_all(&state_root).unwrap();

        let session = RustyClawdAdapter::registered("rc-enrich")
            .unwrap()
            .with_enrichment(state_root)
            .open_session(enrichment_test_request())
            .expect("session must open with enrichment");

        let bridges = session
            .enrichment()
            .expect("RustyClawd session must expose enrichment bridges");
        assert!(
            bridges.is_configured(),
            "with_enrichment must populate the session's bridges (issue #2383)"
        );
        assert!(
            bridges.memory.is_some(),
            "open_session must wire the memory bridge when enrichment is Native"
        );
        assert!(
            bridges.knowledge.is_some(),
            "open_session must wire the knowledge bridge when enrichment is Native"
        );
    }

    /// The default adapter (no `with_enrichment`) must leave both bridges `None`
    /// so unit tests and lightweight callers incur no filesystem side effects.
    #[test]
    fn production_session_without_enrichment_has_empty_bridges() {
        let session = RustyClawdAdapter::registered("rc-no-enrich")
            .unwrap()
            .open_session(enrichment_test_request())
            .expect("session must open without enrichment");

        let bridges = session
            .enrichment()
            .expect("RustyClawd session must expose enrichment bridges");
        assert!(
            !bridges.is_configured(),
            "default adapter must not wire any enrichment bridge"
        );
        assert!(bridges.memory.is_none());
        assert!(bridges.knowledge.is_none());
    }

    /// With a memory bridge injected, `enrich_input` must inject the rendered
    /// `## Relevant Memory Facts` block into `prompt_preamble` while keeping the
    /// objective bare — proving the wired bridges flow through the shared
    /// `enrich_input` entry point for RustyClawd.
    #[test]
    fn enrich_input_injects_memory_facts_block_with_bridge() {
        use crate::base_types::BaseTypeTurnInput;

        let mut session = RustyClawdAdapter::registered("rc-enrich-input")
            .unwrap()
            .open_session(enrichment_test_request())
            .unwrap();

        session
            .enrichment_mut()
            .expect("RustyClawd must support enrichment injection")
            .memory = Some(mock_memory_client());

        let input = BaseTypeTurnInput::objective_only("implement error handling");
        let enriched = session.enrich_input(&input).expect("enrich_input");

        assert!(
            enriched
                .prompt_preamble
                .contains("## Relevant Memory Facts"),
            "enriched preamble must contain the memory facts block, got:\n{}",
            enriched.prompt_preamble
        );
        assert!(
            enriched.prompt_preamble.contains("[testing]"),
            "enriched preamble must render the recalled fact concept"
        );
        assert!(
            enriched
                .prompt_preamble
                .contains("relevant fact about 'implement error handling'"),
            "enriched preamble must render the recalled fact content"
        );
        assert!(
            enriched.prompt_preamble.contains("## Known Procedures"),
            "enriched preamble must render recalled procedures"
        );
        // The objective stays bare so the conversation history stays clean.
        assert_eq!(enriched.objective, "implement error handling");
    }

    /// Without a configured bridge, `enrich_input` is a no-op: the objective is
    /// preserved and no memory block is fabricated.
    #[test]
    fn enrich_input_is_noop_without_bridge() {
        use crate::base_types::BaseTypeTurnInput;

        let session = RustyClawdAdapter::registered("rc-noop")
            .unwrap()
            .open_session(enrichment_test_request())
            .unwrap();

        let input = BaseTypeTurnInput::objective_only("implement error handling");
        let enriched = session.enrich_input(&input).expect("enrich_input");

        assert_eq!(enriched.objective, "implement error handling");
        assert!(
            !enriched
                .prompt_preamble
                .contains("## Relevant Memory Facts"),
            "no bridge configured must not fabricate memory facts"
        );
    }
}
