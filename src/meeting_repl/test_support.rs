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

/// Mock agent whose first `fail_count` `run_turn` calls return `Err`, then
/// succeeds with a canned response. Used to exercise the structured error
/// banner and orphan-turn counting (issue #1983).
pub(super) struct FailingThenOkMockAgent {
    descriptor: BaseTypeDescriptor,
    is_open: bool,
    is_closed: bool,
    canned_response: String,
    fail_count: usize,
    calls: usize,
}

impl FailingThenOkMockAgent {
    pub(super) fn new(fail_count: usize, ok_response: &str) -> Self {
        Self {
            descriptor: BaseTypeDescriptor {
                id: BaseTypeId::new("mock-failing-agent"),
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "mock-failing-agent",
                    "test:mock-failing-agent",
                    Freshness::now().unwrap(),
                ),
                capabilities: standard_session_capabilities(),
                supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
            },
            is_open: true,
            is_closed: false,
            canned_response: ok_response.to_string(),
            fail_count,
            calls: 0,
        }
    }
}

impl BaseTypeSession for FailingThenOkMockAgent {
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
        self.calls += 1;
        if self.calls <= self.fail_count {
            return Err(crate::error::SimardError::ActionExecutionFailed {
                action: "run_turn".to_string(),
                reason: "simulated transient LLM failure".to_string(),
            });
        }
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
