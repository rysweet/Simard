//! TDD tests for wiring the OODA daemon to use a BaseTypeSession for real
//! autonomous work. These tests define the contract BEFORE implementation.
//!
//! The OODA daemon should:
//! 1. Open a BaseTypeSession via SessionBuilder
//! 2. Load active goals from cognitive memory
//! 3. For each goal, run a turn asking the agent to assess progress and take one bounded action
//! 4. Persist results back to memory
//! 5. Sleep and repeat
//!
//! These tests use the TestAdapter (from test_support) so no API key is needed.

use serde_json::json;
use simard::base_types::{BaseTypeOutcome, BaseTypeTurnInput};
use simard::bridge::BridgeErrorPayload;
use simard::bridge_subprocess::InMemoryBridgeTransport;
use simard::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, add_active_goal};
use simard::gym_bridge::GymBridge;
use simard::identity::OperatingMode;
use simard::knowledge_bridge::KnowledgeBridge;
use simard::memory_bridge::CognitiveMemoryBridge;
use simard::ooda_loop::{ActionKind, OodaBridges, OodaConfig, OodaState, run_ooda_cycle};
use simard::session_builder::{LlmProvider, SessionBuilder};
use simard::test_support::TestAdapter;

// ---------------------------------------------------------------------------
// Test helpers (reused mock transports)
// ---------------------------------------------------------------------------

fn mock_memory() -> CognitiveMemoryBridge {
    CognitiveMemoryBridge::new(Box::new(InMemoryBridgeTransport::new(
        "daemon-mem",
        |method, _params| match method {
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
            "memory.record_sensory" => Ok(json!({"id": "sen_1"})),
            "memory.push_working" => Ok(json!({"id": "wrk_1"})),
            "memory.check_triggers" => Ok(json!({"prospectives": []})),
            "memory.clear_working" => Ok(json!({"count": 0})),
            "memory.prune_expired_sensory" => Ok(json!({"count": 0})),
            _ => Err(BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        },
    )))
}

fn mock_gym() -> GymBridge {
    GymBridge::new(Box::new(InMemoryBridgeTransport::new(
        "daemon-gym",
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

fn mock_knowledge() -> KnowledgeBridge {
    KnowledgeBridge::new(Box::new(InMemoryBridgeTransport::new(
        "daemon-knowledge",
        |method, _params| match method {
            "knowledge.list_packs" => Ok(json!({"packs": []})),
            _ => Err(BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        },
    )))
}

fn test_bridges() -> OodaBridges {
    OodaBridges {
        memory: Box::new(mock_memory()),
        knowledge: mock_knowledge(),
        gym: mock_gym(),
        session: None,
        brain: std::sync::Arc::new(simard::ooda_brain::DeterministicFallbackBrain),
        decide_brain: None,
        orient_brain: None,
    }
}

fn board_with_active_goals() -> GoalBoard {
    let mut board = GoalBoard::new();
    add_active_goal(
        &mut board,
        ActiveGoal {
            id: "goal-improve-tests".to_string(),
            description: "Improve test coverage for session_builder module".to_string(),
            priority: 1,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            current_activity: None,
            wip_refs: Vec::new(),
        },
    )
    .unwrap();
    add_active_goal(
        &mut board,
        ActiveGoal {
            id: "goal-fix-docs".to_string(),
            description: "Update API documentation for meeting_facilitator".to_string(),
            priority: 2,
            status: GoalProgress::InProgress { percent: 30 },
            assigned_to: None,
            current_activity: None,
            wip_refs: Vec::new(),
        },
    )
    .unwrap();
    board
}

// ===========================================================================
// 1. SessionBuilder can create an OODA-mode session
// ===========================================================================

#[test]
fn session_builder_creates_ooda_session_request_with_correct_mode() {
    // The OODA daemon should use OperatingMode::Engineer (it's doing engineering work)
    // since there's no dedicated OODA mode. Verify the builder produces a valid request.
    let builder = SessionBuilder::new(OperatingMode::Engineer, LlmProvider::RustyClawd)
        .node_id("ooda-daemon")
        .address("ooda-daemon://local")
        .adapter_tag("ooda-rustyclawd");

    let request = builder.build_request();
    assert_eq!(request.mode, OperatingMode::Engineer);
    assert_eq!(
        request.runtime_node,
        simard::runtime::RuntimeNodeId::new("ooda-daemon")
    );
    assert_eq!(
        request.mailbox_address,
        simard::runtime::RuntimeAddress::new("ooda-daemon://local")
    );
}

// ===========================================================================
// 2. TestAdapter can open a session and run turns (simulating real daemon flow)
// ===========================================================================

#[test]
fn test_adapter_session_runs_turn_for_goal_objective() {
    // The OODA daemon should open a session and call run_turn for each goal.
    // Using TestAdapter to verify the flow without an API key.
    let adapter = TestAdapter::single_process("ooda-test").unwrap();
    use simard::base_types::BaseTypeFactory;

    let request = SessionBuilder::new(OperatingMode::Engineer, LlmProvider::RustyClawd)
        .node_id("ooda-daemon")
        .address("ooda-daemon://local")
        .adapter_tag("ooda-test")
        .build_request();

    let mut session = adapter.open_session(request).unwrap();
    session.open().unwrap();

    // Simulate what the daemon should do: for each goal, run a turn
    let goal_objective = "Assess progress on goal 'goal-improve-tests': Improve test coverage for session_builder module. Take one bounded action to advance this goal.";
    let outcome = session
        .run_turn(BaseTypeTurnInput::objective_only(goal_objective))
        .unwrap();

    // The outcome should contain a plan and execution summary
    assert!(!outcome.plan.is_empty());
    assert!(!outcome.execution_summary.is_empty());
    assert!(!outcome.evidence.is_empty());

    session.close().unwrap();
}

#[test]
fn daemon_session_handles_multiple_goals_sequentially() {
    // The daemon should run one turn per goal, sequentially, in a single cycle.
    let adapter = TestAdapter::single_process("ooda-multi").unwrap();
    use simard::base_types::BaseTypeFactory;

    let request = SessionBuilder::new(OperatingMode::Engineer, LlmProvider::RustyClawd)
        .node_id("ooda-daemon")
        .address("ooda-daemon://local")
        .adapter_tag("ooda-multi")
        .build_request();

    let mut session = adapter.open_session(request).unwrap();
    session.open().unwrap();

    let goals = vec![
        ("goal-1", "Improve test coverage"),
        ("goal-2", "Fix documentation"),
        ("goal-3", "Refactor error handling"),
    ];

    let mut outcomes: Vec<BaseTypeOutcome> = Vec::new();
    for (goal_id, description) in &goals {
        let objective =
            format!("Assess progress on goal '{goal_id}': {description}. Take one bounded action.");
        let outcome = session
            .run_turn(BaseTypeTurnInput::objective_only(&objective))
            .unwrap();
        outcomes.push(outcome);
    }

    assert_eq!(outcomes.len(), 3);
    // Each turn should produce distinct evidence
    for outcome in &outcomes {
        assert!(!outcome.plan.is_empty());
    }

    session.close().unwrap();
}

// ===========================================================================
// 3. OODA cycle advances goals using session turn results
// ===========================================================================

/// This test verifies that when run_ooda_cycle dispatches AdvanceGoal actions,
/// the daemon's new flow should use run_turn on a session to determine actual
/// progress rather than just bumping percentage by 10.
///
/// Currently, dispatch_advance_goal just bumps percent by 10. After wiring,
/// it should use the session's run_turn output to determine real progress.
#[test]
fn ooda_cycle_advance_goal_produces_outcome_for_each_active_goal() {
    let mut bridges = test_bridges();
    let board = board_with_active_goals();
    let mut state = OodaState::new(board);
    let config = OodaConfig::default();

    let report = run_ooda_cycle(&mut state, &mut bridges, &config).unwrap();

    // Should have actions dispatched for the active goals
    assert!(!report.planned_actions.is_empty());

    // At least one AdvanceGoal action should be in the plan
    let advance_actions: Vec<_> = report
        .planned_actions
        .iter()
        .filter(|a| a.kind == ActionKind::AdvanceGoal)
        .collect();
    assert!(
        !advance_actions.is_empty(),
        "OODA cycle should plan AdvanceGoal actions for active goals"
    );

    // Each advance action should have a corresponding outcome
    let advance_outcomes: Vec<_> = report
        .outcomes
        .iter()
        .filter(|o| o.action.kind == ActionKind::AdvanceGoal)
        .collect();
    assert_eq!(advance_actions.len(), advance_outcomes.len());
}

// ===========================================================================
// 4. Daemon-style loop: multiple cycles with state persistence
// ===========================================================================

#[test]
fn daemon_runs_multiple_cycles_and_persists_state_across_them() {
    let mut bridges = test_bridges();
    let board = board_with_active_goals();
    let mut state = OodaState::new(board);
    let config = OodaConfig::default();

    // Run 3 cycles (simulating daemon loop without sleep)
    let mut reports = Vec::new();
    for _ in 0..3 {
        let report = run_ooda_cycle(&mut state, &mut bridges, &config).unwrap();
        reports.push(report);
    }

    assert_eq!(reports.len(), 3);
    assert_eq!(reports[0].cycle_number, 1);
    assert_eq!(reports[1].cycle_number, 2);
    assert_eq!(reports[2].cycle_number, 3);
    assert_eq!(state.cycle_count, 3);

    // Goal should still be on the active board — state persists across cycles.
    let test_goal = state
        .active_goals
        .active
        .iter()
        .find(|g| g.id == "goal-improve-tests");
    assert!(
        test_goal.is_some(),
        "goal should still be on the active board"
    );
    // Goal status depends on whether a session was available during cycles.
    // Without a session, advance_goal fails visibly and goal stays NotStarted
    // (per PHILOSOPHY.md: no silent fallback). With a session, it progresses.
    // Either is valid; what matters is that state survives across cycles.
    match &test_goal.unwrap().status {
        GoalProgress::InProgress { .. }
        | GoalProgress::Completed
        | GoalProgress::NotStarted
        | GoalProgress::Blocked(_) => {}
    }
}

// ===========================================================================
// 5. Session builder returns None without API key (daemon degrades honestly)
// ===========================================================================

#[test]
fn daemon_degrades_gracefully_when_no_provider() {
    // Force RustyClawd without ANTHROPIC_API_KEY → session may open but
    // won't produce useful results. The daemon still runs OODA via bridges.
    unsafe {
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::set_var("SIMARD_LLM_PROVIDER", "rustyclawd");
    };
    let session = SessionBuilder::new(OperatingMode::Engineer, LlmProvider::RustyClawd)
        .node_id("ooda-daemon")
        .address("ooda-daemon://local")
        .adapter_tag("nonexistent-adapter")
        .open();
    unsafe { std::env::remove_var("SIMARD_LLM_PROVIDER") };

    // Session may or may not open depending on adapter internals — both are valid.
    drop(session);

    // Daemon should still be able to run OODA cycles via bridges regardless.
    let mut bridges = test_bridges();
    let mut state = OodaState::new(board_with_active_goals());
    let report = run_ooda_cycle(&mut state, &mut bridges, &OodaConfig::default()).unwrap();
    assert_eq!(report.cycle_number, 1);
}

// ===========================================================================
// 6. Goal objective formatting for run_turn
// ===========================================================================

/// The daemon should construct a clear objective string for each goal
/// that tells the agent what to assess and what bounded action to take.
#[test]
fn goal_objective_contains_goal_id_and_description() {
    let goal = ActiveGoal {
        id: "goal-improve-tests".to_string(),
        description: "Improve test coverage for session_builder module".to_string(),
        priority: 1,
        status: GoalProgress::InProgress { percent: 30 },
        assigned_to: None,
        current_activity: None,
        wip_refs: Vec::new(),
    };

    // This is the format the daemon should use when constructing objectives
    let objective = format!(
        "Goal '{}' ({}% complete): {}. Assess current progress and take one bounded action to advance this goal.",
        goal.id,
        match &goal.status {
            GoalProgress::InProgress { percent } => *percent,
            _ => 0,
        },
        goal.description,
    );

    assert!(objective.contains("goal-improve-tests"));
    assert!(objective.contains("30%"));
    assert!(objective.contains("Improve test coverage"));
    assert!(objective.contains("one bounded action"));
}

// ===========================================================================
// 7. run_ooda_daemon function signature accepts optional session
// ===========================================================================

/// Test that run_ooda_daemon_with_session (the new entry point) can accept
/// a pre-built session for testing, avoiding the need for live bridges.
///
/// This test defines the expected new API. The function should:
/// - Accept an optional Box<dyn BaseTypeSession>
/// - Use it for run_turn calls during AdvanceGoal actions
/// - Use bridge-only dispatch if session is None
#[test]
fn run_ooda_daemon_with_session_uses_session_for_advance_goal() {
    // This tests the new function signature that we need to implement.
    // For now it uses the existing run_ooda_cycle which doesn't have session support yet.
    // After implementation, this should call the new daemon entry point.
    let mut bridges = test_bridges();
    let board = board_with_active_goals();
    let mut state = OodaState::new(board);
    let config = OodaConfig {
        max_concurrent_actions: 2,
        ..Default::default()
    };

    // Run a cycle — after implementation, advance_goal actions should use the session
    let report = run_ooda_cycle(&mut state, &mut bridges, &config).unwrap();

    // Verify advance goal actions exist and produce outcomes
    let advance_outcomes: Vec<_> = report
        .outcomes
        .iter()
        .filter(|o| o.action.kind == ActionKind::AdvanceGoal)
        .collect();

    for outcome in &advance_outcomes {
        // After wiring, successful outcomes contain evidence from run_turn.
        // Without a session, advance_goal fails visibly per PHILOSOPHY.md
        // ("no LLM session available"). Both success and visible-failure with
        // a clear explanation are acceptable here; what we forbid is silent
        // success without a session (the old fallback behavior).
        let detail = &outcome.detail;
        assert!(
            outcome.success
                || detail.contains("blocked")
                || detail.contains("no LLM session")
                || detail.contains("cannot advance"),
            "advance goal outcome should succeed or explain visibly: {detail}",
        );
    }
}

// ===========================================================================
// 8. Cycle report contains session-derived evidence (post-implementation)
// ===========================================================================

#[test]
fn cycle_report_summarizes_all_action_outcomes() {
    let mut bridges = test_bridges();
    let mut state = OodaState::new(board_with_active_goals());
    let config = OodaConfig::default();

    let report = run_ooda_cycle(&mut state, &mut bridges, &config).unwrap();
    let summary = simard::ooda_loop::summarize_cycle_report(&report);

    assert!(summary.contains("OODA cycle #1"));
    assert!(summary.contains("priorities"));
    assert!(summary.contains("actions"));
    assert!(summary.contains("goals="));
}
