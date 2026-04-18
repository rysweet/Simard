//! Shared test helpers for the ooda_actions module.

use crate::base_types::{BaseTypeDescriptor, BaseTypeOutcome, BaseTypeSession, BaseTypeTurnInput};
use crate::bridge::BridgeErrorPayload;
use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardError;
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, add_active_goal};
use crate::gym_bridge::GymBridge;
use crate::knowledge_bridge::KnowledgeBridge;
use crate::memory_bridge::CognitiveMemoryBridge;
use crate::ooda_loop::OodaBridges;
use serde_json::json;
use std::cell::RefCell;
use std::rc::Rc;

/// A mock session that captures the input sent to `run_turn` and returns
/// a configurable response. Used to test `advance_goal_with_session`.
pub(crate) struct MockSession {
    captured_input: Rc<RefCell<Option<BaseTypeTurnInput>>>,
    response: Result<BaseTypeOutcome, String>,
}

impl MockSession {
    pub(crate) fn new_ok(
        summary: &str,
        evidence: Vec<String>,
    ) -> (Self, Rc<RefCell<Option<BaseTypeTurnInput>>>) {
        let captured = Rc::new(RefCell::new(None));
        let session = Self {
            captured_input: Rc::clone(&captured),
            response: Ok(BaseTypeOutcome {
                plan: String::new(),
                execution_summary: summary.to_string(),
                evidence,
            }),
        };
        (session, captured)
    }

    pub(crate) fn new_err(msg: &str) -> Self {
        Self {
            captured_input: Rc::new(RefCell::new(None)),
            response: Err(msg.to_string()),
        }
    }
}

// MockSession is !Send because of Rc<RefCell<...>>, but tests are single-threaded.
// We need Send for BaseTypeSession trait bound, so use an unsafe impl.
unsafe impl Send for MockSession {}

impl BaseTypeSession for MockSession {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        unimplemented!("not needed for advance_goal_with_session tests")
    }

    fn open(&mut self) -> crate::error::SimardResult<()> {
        Ok(())
    }

    fn run_turn(
        &mut self,
        input: BaseTypeTurnInput,
    ) -> crate::error::SimardResult<BaseTypeOutcome> {
        *self.captured_input.borrow_mut() = Some(input);
        match &self.response {
            Ok(outcome) => Ok(outcome.clone()),
            Err(msg) => Err(SimardError::BridgeTransportError {
                bridge: "mock-session".to_string(),
                reason: msg.clone(),
            }),
        }
    }

    fn close(&mut self) -> crate::error::SimardResult<()> {
        Ok(())
    }
}

pub(crate) fn mock_memory() -> Box<dyn CognitiveMemoryOps> {
    Box::new(CognitiveMemoryBridge::new(Box::new(
        InMemoryBridgeTransport::new("test-mem", |method, _params| match method {
            "memory.search_facts" => Ok(json!({"facts": []})),
            "memory.store_fact" => Ok(json!({"id": "sem_1"})),
            "memory.store_episode" => Ok(json!({"id": "epi_1"})),
            "memory.get_statistics" => Ok(json!({
                "sensory_count": 5, "working_count": 3, "episodic_count": 12,
                "semantic_count": 8, "procedural_count": 2, "prospective_count": 1
            })),
            "memory.consolidate_episodes" => Ok(json!({"id": null})),
            "memory.recall_procedure" => Ok(json!({
                "procedures": [{"node_id": "proc_1", "name": "cargo build",
                    "steps": ["compile", "test"], "prerequisites": ["rust"],
                    "usage_count": 5}]
            })),
            _ => Err(BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        }),
    )))
}

pub(crate) fn mock_gym() -> GymBridge {
    GymBridge::new(Box::new(InMemoryBridgeTransport::new(
        "test-gym",
        |_method, _params| {
            Ok(json!({
                "suite_id": "progressive", "success": true, "overall_score": 0.75,
                "dimensions": {"factual_accuracy": 0.8, "specificity": 0.7,
                    "temporal_awareness": 0.75, "source_attribution": 0.7,
                    "confidence_calibration": 0.8},
                "scenario_results": [], "scenarios_passed": 6, "scenarios_total": 6,
                "degraded_sources": []
            }))
        },
    )))
}

pub(crate) fn mock_knowledge() -> KnowledgeBridge {
    KnowledgeBridge::new(Box::new(InMemoryBridgeTransport::new(
        "test-knowledge",
        |method, _params| match method {
            "knowledge.list_packs" => Ok(json!({"packs": [{"name": "rust-expert",
                "description": "Rust knowledge", "article_count": 100,
                "section_count": 400}]})),
            _ => Err(BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        },
    )))
}

pub(crate) fn test_bridges() -> OodaBridges {
    OodaBridges {
        memory: mock_memory(),
        knowledge: mock_knowledge(),
        gym: mock_gym(),
        session: None,
    }
}

pub(crate) fn board_with_goal(
    id: &str,
    progress: GoalProgress,
    assigned: Option<&str>,
) -> GoalBoard {
    let mut board = GoalBoard::new();
    add_active_goal(
        &mut board,
        ActiveGoal {
            id: id.to_string(),
            description: format!("Goal {id}"),
            priority: 1,
            status: progress,
            assigned_to: assigned.map(String::from),
            current_activity: None,
            wip_refs: vec![],
        },
    )
    .unwrap();
    board
}

pub(crate) fn bridges_with_session(session: MockSession) -> OodaBridges {
    OodaBridges {
        memory: mock_memory(),
        knowledge: mock_knowledge(),
        gym: mock_gym(),
        session: Some(Box::new(session)),
    }
}
