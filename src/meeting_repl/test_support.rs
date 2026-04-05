//! Shared test fixtures for the `meeting_repl` sub-modules.

use crate::base_types::{
    BaseTypeDescriptor, BaseTypeId, BaseTypeOutcome, BaseTypeSession, BaseTypeTurnInput,
    ensure_session_not_already_open, ensure_session_not_closed, ensure_session_open,
    standard_session_capabilities,
};
use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::memory_bridge::CognitiveMemoryBridge;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;
use serde_json::json;

pub(super) fn mock_bridge() -> CognitiveMemoryBridge {
    let transport =
        InMemoryBridgeTransport::new("test-meeting-repl", |method, _params| match method {
            "memory.record_sensory" => Ok(json!({"id": "sen_r1"})),
            "memory.store_episode" => Ok(json!({"id": "epi_r1"})),
            "memory.store_fact" => Ok(json!({"id": "sem_r1"})),
            "memory.store_prospective" => Ok(json!({"id": "pro_r1"})),
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    CognitiveMemoryBridge::new(Box::new(transport))
}

/// Mock agent that returns a canned response for every turn.
pub(super) struct MockAgentSession {
    descriptor: BaseTypeDescriptor,
    is_open: bool,
    is_closed: bool,
    canned_response: String,
}

impl MockAgentSession {
    pub(super) fn new(response: &str) -> Self {
        Self {
            descriptor: BaseTypeDescriptor {
                id: BaseTypeId::new("mock-meeting-agent"),
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "mock-agent",
                    "test:mock-meeting-agent",
                    Freshness::now().unwrap(),
                ),
                capabilities: standard_session_capabilities(),
                supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
            },
            is_open: true,
            is_closed: false,
            canned_response: response.to_string(),
        }
    }
}

impl BaseTypeSession for MockAgentSession {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open(&mut self) -> crate::error::SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "open")?;
        ensure_session_not_already_open(&self.descriptor, self.is_open)?;
        self.is_open = true;
        Ok(())
    }

    fn run_turn(
        &mut self,
        _input: BaseTypeTurnInput,
    ) -> crate::error::SimardResult<BaseTypeOutcome> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "run_turn")?;
        ensure_session_open(&self.descriptor, self.is_open, "run_turn")?;
        Ok(BaseTypeOutcome {
            plan: String::new(),
            execution_summary: self.canned_response.clone(),
            evidence: Vec::new(),
        })
    }

    fn close(&mut self) -> crate::error::SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
        self.is_closed = true;
        Ok(())
    }
}
