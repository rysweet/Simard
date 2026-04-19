//! Integration tests for the OODA loop, scheduler, and skill builder.

use serde_json::json;
use simard::bridge::BridgeErrorPayload;
use simard::bridge_subprocess::InMemoryBridgeTransport;
use simard::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, add_active_goal};
use simard::gym_bridge::GymBridge;
use simard::knowledge_bridge::KnowledgeBridge;
use simard::memory_bridge::CognitiveMemoryBridge;
use simard::ooda_loop::{
    ActionKind, OodaBridges, OodaConfig, OodaState, act, decide, observe, orient, run_ooda_cycle,
    summarize_cycle_report,
};
use simard::ooda_scheduler::{
    Scheduler, SlotStatus, complete_slot, drain_finished, fail_slot, poll_slots, schedule_actions,
    scheduler_summary, start_slot,
};
use simard::skill_builder::{
    SkillTemplate, extract_skill_candidates, generate_skill_definition, install_skill,
};

fn mock_memory_transport() -> InMemoryBridgeTransport {
    InMemoryBridgeTransport::new("test-memory", |method, _params| match method {
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
                "steps": ["compile", "test"], "prerequisites": ["rust"], "usage_count": 5}]
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
    })
}

fn mock_gym_transport() -> InMemoryBridgeTransport {
    InMemoryBridgeTransport::new("test-gym", |_method, _params| {
        Ok(json!({
            "suite_id": "progressive", "success": true, "overall_score": 0.75,
            "dimensions": {"factual_accuracy": 0.8, "specificity": 0.7,
                "temporal_awareness": 0.75, "source_attribution": 0.7,
                "confidence_calibration": 0.8},
            "scenario_results": [], "scenarios_passed": 6, "scenarios_total": 6,
            "degraded_sources": []
        }))
    })
}

fn mock_knowledge_transport() -> InMemoryBridgeTransport {
    InMemoryBridgeTransport::new("test-knowledge", |method, _params| match method {
        "knowledge.list_packs" => Ok(json!({"packs": [{"name": "rust-expert",
            "description": "Rust knowledge", "article_count": 100, "section_count": 400}]})),
        _ => Err(BridgeErrorPayload {
            code: -32601,
            message: format!("unknown: {method}"),
        }),
    })
}

fn test_bridges() -> OodaBridges {
    OodaBridges {
        memory: Box::new(CognitiveMemoryBridge::new(
            Box::new(mock_memory_transport()),
        )),
        knowledge: KnowledgeBridge::new(Box::new(mock_knowledge_transport())),
        gym: GymBridge::new(Box::new(mock_gym_transport())),
        session: None,
    }
}

fn sample_goal(id: &str, priority: u32, progress: GoalProgress) -> ActiveGoal {
    ActiveGoal {
        id: id.to_string(),
        description: format!("Goal {id}"),
        priority,
        status: progress,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    }
}

fn board_with_goals() -> GoalBoard {
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, sample_goal("g1", 1, GoalProgress::NotStarted)).unwrap();
    add_active_goal(
        &mut board,
        sample_goal("g2", 2, GoalProgress::InProgress { percent: 50 }),
    )
    .unwrap();
    add_active_goal(
        &mut board,
        sample_goal(
            "g3",
            3,
            GoalProgress::Blocked("waiting on review".to_string()),
        ),
    )
    .unwrap();
    board
}

#[test]
fn observe_gathers_goal_statuses_and_gym_health() {
    let bridges = test_bridges();
    let mut state = OodaState::new(board_with_goals());
    let obs = observe(&mut state, &bridges).unwrap();
    assert_eq!(obs.goal_statuses.len(), 3);
    assert!(obs.gym_health.is_some());
    assert!(obs.memory_stats.total() > 0);
}

#[test]
fn orient_produces_ranked_priorities() {
    let bridges = test_bridges();
    let board = board_with_goals();
    let mut state = OodaState::new(board.clone());
    let obs = observe(&mut state, &bridges).unwrap();
    let priorities = orient(&obs, &board).unwrap();
    assert!(!priorities.is_empty());
    assert_eq!(priorities[0].goal_id, "g3"); // blocked = highest urgency
    assert!(priorities[0].urgency > priorities.last().unwrap().urgency);
}

#[test]
fn decide_selects_actions_within_concurrent_limit() {
    let bridges = test_bridges();
    let board = board_with_goals();
    let mut state = OodaState::new(board.clone());
    let obs = observe(&mut state, &bridges).unwrap();
    let priorities = orient(&obs, &board).unwrap();
    let config = OodaConfig {
        max_concurrent_actions: 2,
        ..Default::default()
    };
    let actions = decide(&priorities, &config).unwrap();
    assert!(actions.len() <= 2);
    assert!(!actions.is_empty());
}

#[test]
fn act_dispatches_and_returns_outcomes() {
    let mut bridges = test_bridges();
    let board = board_with_goals();
    let mut state = OodaState::new(board.clone());
    let obs = observe(&mut state, &bridges).unwrap();
    let priorities = orient(&obs, &board).unwrap();
    let actions = decide(&priorities, &OodaConfig::default()).unwrap();
    let outcomes = act(&actions, &mut bridges, &mut state).unwrap();
    assert_eq!(outcomes.len(), actions.len());
    // AdvanceGoal for blocked goals will fail (can't advance blocked goals).
    // All other outcomes should succeed.
    for outcome in &outcomes {
        match outcome.action.kind {
            ActionKind::AdvanceGoal => {
                // Blocked goals fail, non-blocked goals succeed.
            }
            _ => assert!(
                outcome.success,
                "expected success for {:?}: {}",
                outcome.action.kind, outcome.detail
            ),
        }
    }
}

#[test]
fn run_full_ooda_cycle_and_increments() {
    let mut bridges = test_bridges();
    let mut state = OodaState::new(board_with_goals());
    let config = OodaConfig::default();
    let r1 = run_ooda_cycle(&mut state, &mut bridges, &config).unwrap();
    assert_eq!(r1.cycle_number, 1);
    assert!(!r1.priorities.is_empty());
    assert!(!r1.outcomes.is_empty());
    assert!(summarize_cycle_report(&r1).contains("OODA cycle #1"));
    let r2 = run_ooda_cycle(&mut state, &mut bridges, &config).unwrap();
    assert_eq!(r2.cycle_number, 2);
    assert_eq!(state.cycle_count, 2);
}

#[test]
fn feral_empty_goals_seeds_defaults() {
    let mut bridges = test_bridges();
    let mut state = OodaState::new(GoalBoard::new());
    let report = run_ooda_cycle(&mut state, &mut bridges, &OodaConfig::default()).unwrap();
    // Empty boards now get seeded with 5 default goals before observation.
    assert_eq!(report.observation.goal_statuses.len(), 5);
    assert_eq!(report.cycle_number, 1);
}

#[test]
fn feral_all_goals_blocked() {
    let bridges = test_bridges();
    let mut board = GoalBoard::new();
    add_active_goal(
        &mut board,
        sample_goal("b1", 1, GoalProgress::Blocked("dep".into())),
    )
    .unwrap();
    add_active_goal(
        &mut board,
        sample_goal("b2", 2, GoalProgress::Blocked("rev".into())),
    )
    .unwrap();
    let mut state = OodaState::new(board.clone());
    let obs = observe(&mut state, &bridges).unwrap();
    let priorities = orient(&obs, &board).unwrap();
    for p in priorities.iter().filter(|p| !p.goal_id.starts_with("__")) {
        assert!((p.urgency - 1.0).abs() < f64::EPSILON);
    }
}

#[test]
fn feral_gym_bridge_down() {
    let failing_gym = InMemoryBridgeTransport::new("gym-fail", |_, _| {
        Err(BridgeErrorPayload {
            code: -32603,
            message: "crashed".into(),
        })
    });
    let mut bridges = OodaBridges {
        memory: Box::new(CognitiveMemoryBridge::new(
            Box::new(mock_memory_transport()),
        )),
        knowledge: KnowledgeBridge::new(Box::new(mock_knowledge_transport())),
        gym: GymBridge::new(Box::new(failing_gym)),
        session: None,
    };
    let mut state = OodaState::new(board_with_goals());
    let report = run_ooda_cycle(&mut state, &mut bridges, &OodaConfig::default()).unwrap();
    assert!(report.observation.gym_health.is_none());
    assert_eq!(report.cycle_number, 1);
}

#[test]
fn scheduler_slot_lifecycle() {
    use simard::ooda_loop::{ActionKind, ActionOutcome, PlannedAction};
    let mut sched = Scheduler::new(3);
    assert!(sched.has_capacity());
    let actions = vec![
        PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "adv".into(),
        },
        PlannedAction {
            kind: ActionKind::ConsolidateMemory,
            goal_id: None,
            description: "cons".into(),
        },
    ];
    let scheduled = schedule_actions(&mut sched, actions).unwrap();
    assert_eq!(scheduled.len(), 2);
    start_slot(&mut sched, 0).unwrap();
    assert!(matches!(sched.slots[0].status, SlotStatus::Running { .. }));
    complete_slot(
        &mut sched,
        0,
        ActionOutcome {
            action: scheduled[0].action.clone(),
            success: true,
            detail: "done".into(),
        },
    )
    .unwrap();
    start_slot(&mut sched, 1).unwrap();
    fail_slot(&mut sched, 1, "bridge down".into()).unwrap();
    let completed = poll_slots(&mut sched);
    assert_eq!(completed.len(), 2);
    assert!(completed[0].outcome.is_ok());
    assert!(completed[1].outcome.is_err());
}

#[test]
fn scheduler_respects_capacity_and_drain() {
    use simard::ooda_loop::{ActionKind, ActionOutcome, PlannedAction};
    let mut sched = Scheduler::new(1);
    let actions = vec![
        PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "a".into(),
        },
        PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g2".into()),
            description: "b".into(),
        },
    ];
    let scheduled = schedule_actions(&mut sched, actions).unwrap();
    assert_eq!(scheduled.len(), 1);
    start_slot(&mut sched, 0).unwrap();
    complete_slot(
        &mut sched,
        0,
        ActionOutcome {
            action: scheduled[0].action.clone(),
            success: true,
            detail: "ok".into(),
        },
    )
    .unwrap();
    let drained = drain_finished(&mut sched);
    assert_eq!(drained.len(), 1);
    assert!(sched.slots.is_empty());
    let summary = scheduler_summary(&sched);
    assert!(summary.contains("max=1"));
}

#[test]
fn start_non_pending_slot_fails() {
    use simard::ooda_loop::{ActionKind, PlannedAction};
    let mut sched = Scheduler::new(3);
    schedule_actions(
        &mut sched,
        vec![PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "t".into(),
        }],
    )
    .unwrap();
    start_slot(&mut sched, 0).unwrap();
    assert!(
        start_slot(&mut sched, 0)
            .unwrap_err()
            .to_string()
            .contains("not pending")
    );
}

#[test]
fn extract_skill_candidates_filters_by_usage() {
    let bridge = CognitiveMemoryBridge::new(Box::new(mock_memory_transport()));
    let candidates = extract_skill_candidates(&bridge, 3).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].name, "cargo-build");
    assert!(extract_skill_candidates(&bridge, 10).unwrap().is_empty());
}

#[test]
fn generate_skill_definition_produces_valid_markdown() {
    let template = SkillTemplate {
        name: "deploy-service".into(),
        description: "Deploy a microservice".into(),
        steps: vec!["build image".into(), "push".into(), "update deploy".into()],
        trigger_patterns: vec!["deploy".into(), "release".into()],
    };
    let md = generate_skill_definition(&template);
    assert!(md.contains("# deploy-service"));
    assert!(md.contains("- `deploy`"));
    assert!(md.contains("1. build image"));
    assert!(md.contains("3. update deploy"));
}

#[test]
fn install_skill_refuses_overwrite() {
    let dir = std::env::temp_dir().join("simard-ooda-test-skill-overwrite");
    let _ = std::fs::remove_dir_all(&dir);
    let template = SkillTemplate {
        name: "overwrite-test".into(),
        description: "test".into(),
        steps: vec!["step".into()],
        trigger_patterns: vec!["test".into()],
    };
    let path = install_skill(&template, &dir).unwrap();
    assert!(path.exists());
    assert!(
        install_skill(&template, &dir)
            .unwrap_err()
            .to_string()
            .contains("already exists")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
