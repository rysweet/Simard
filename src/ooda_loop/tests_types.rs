use super::types::*;
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};

// --- OodaPhase Display ---

#[test]
fn ooda_phase_display_all_variants() {
    assert_eq!(OodaPhase::Observe.to_string(), "observe");
    assert_eq!(OodaPhase::Orient.to_string(), "orient");
    assert_eq!(OodaPhase::Decide.to_string(), "decide");
    assert_eq!(OodaPhase::Act.to_string(), "act");
    assert_eq!(OodaPhase::Sleep.to_string(), "sleep");
}

#[test]
fn ooda_phase_equality() {
    assert_eq!(OodaPhase::Observe, OodaPhase::Observe);
    assert_ne!(OodaPhase::Observe, OodaPhase::Orient);
}

#[test]
fn ooda_phase_clone() {
    let phase = OodaPhase::Decide;
    let cloned = phase;
    assert_eq!(phase, cloned);
}

// --- OodaState ---

#[test]
fn ooda_state_new_defaults() {
    let state = OodaState::new(GoalBoard::new());
    assert_eq!(state.current_phase, OodaPhase::Observe);
    assert_eq!(state.cycle_count, 0);
    assert!(state.last_observation.is_none());
    assert!(state.review_improvements.is_empty());
    assert!(state.active_goals.active.is_empty());
    assert!(state.prepared_context.is_none());
    assert_eq!(state.cycle_start_epoch, 0);
    assert!(state.last_cycle_summary.is_none());
    assert!(state.last_cycle_duration_secs.is_none());
}

#[test]
fn ooda_state_new_with_goals() {
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        id: "goal-1".to_string(),
        description: "Test goal".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });
    let state = OodaState::new(board);
    assert_eq!(state.active_goals.active.len(), 1);
}

// --- OodaStateSnapshot round-trip ---

fn populated_state() -> OodaState {
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        id: "goal-snap".to_string(),
        description: "Snapshot test".to_string(),
        priority: 3,
        status: GoalProgress::InProgress { percent: 25 },
        assigned_to: Some("engineer-7".to_string()),
        current_activity: Some("doing things".to_string()),
        wip_refs: vec![],
    });
    let mut state = OodaState::new(board);
    state.current_phase = OodaPhase::Decide;
    state.cycle_count = 7;
    state.cycle_start_epoch = 1_700_000_000;
    state.last_cycle_summary = Some("prior summary".to_string());
    state.last_cycle_duration_secs = Some(42);
    state.goal_failure_counts.insert("goal-snap".to_string(), 2);
    state
}

#[test]
fn snapshot_captures_serializable_fields() {
    let state = populated_state();
    let snap = OodaStateSnapshot::from(&state);
    assert_eq!(snap.current_phase, OodaPhase::Decide);
    assert_eq!(snap.cycle_count, 7);
    assert_eq!(snap.cycle_start_epoch, 1_700_000_000);
    assert_eq!(snap.last_cycle_summary.as_deref(), Some("prior summary"));
    assert_eq!(snap.last_cycle_duration_secs, Some(42));
    assert_eq!(snap.goal_failure_counts.get("goal-snap"), Some(&2));
    assert_eq!(snap.active_goals.active.len(), 1);
    assert_eq!(snap.active_goals.active[0].id, "goal-snap");
}

#[test]
fn snapshot_json_roundtrip_preserves_state() {
    let state = populated_state();
    let snap = OodaStateSnapshot::from(&state);
    let json = serde_json::to_string(&snap).expect("serialize snapshot");
    let parsed: OodaStateSnapshot = serde_json::from_str(&json).expect("deserialize snapshot");

    let mut restored = OodaState::new(GoalBoard::new());
    parsed.apply_to(&mut restored);

    assert_eq!(restored.current_phase, state.current_phase);
    assert_eq!(restored.cycle_count, state.cycle_count);
    assert_eq!(restored.cycle_start_epoch, state.cycle_start_epoch);
    assert_eq!(restored.last_cycle_summary, state.last_cycle_summary);
    assert_eq!(
        restored.last_cycle_duration_secs,
        state.last_cycle_duration_secs
    );
    assert_eq!(
        restored.goal_failure_counts.get("goal-snap"),
        state.goal_failure_counts.get("goal-snap")
    );
    assert_eq!(
        restored.active_goals.active.len(),
        state.active_goals.active.len()
    );
    assert_eq!(
        restored.active_goals.active[0].id,
        state.active_goals.active[0].id
    );
}

#[test]
fn snapshot_apply_preserves_in_process_worktrees() {
    // worktrees are NOT round-tripped — applying a snapshot must not
    // touch them. Verify by leaving HashMap untouched after apply.
    let mut state = OodaState::new(GoalBoard::new());
    // We can't easily construct EngineerWorktree in tests (it owns a
    // real git path), so just assert the field stays an empty map
    // after a no-op snapshot apply.
    assert!(state.engineer_worktrees.is_empty());
    let snap = OodaStateSnapshot::from(&state);
    snap.apply_to(&mut state);
    assert!(
        state.engineer_worktrees.is_empty(),
        "apply_to must not clobber engineer_worktrees"
    );
}

#[test]
fn snapshot_into_state_constructs_fresh_state() {
    let original = populated_state();
    let snap = OodaStateSnapshot::from(&original);
    let restored = snap.into_state();
    assert_eq!(restored.cycle_count, original.cycle_count);
    assert_eq!(restored.current_phase, original.current_phase);
    assert!(restored.engineer_worktrees.is_empty());
}

// --- GoalSnapshot ---

#[test]
fn goal_snapshot_from_active_goal() {
    let goal = ActiveGoal {
        id: "g-1".to_string(),
        description: "Build widget".to_string(),
        priority: 2,
        status: GoalProgress::InProgress { percent: 50 },
        assigned_to: Some("engineer".to_string()),
        current_activity: None,
        wip_refs: vec![],
    };
    let snapshot = GoalSnapshot::from(&goal);
    assert_eq!(snapshot.id, "g-1");
    assert_eq!(snapshot.description, "Build widget");
    assert!(matches!(
        snapshot.progress,
        GoalProgress::InProgress { percent: 50 }
    ));
}

#[test]
fn goal_snapshot_from_blocked_goal() {
    let goal = ActiveGoal {
        id: "g-blocked".to_string(),
        description: "Blocked task".to_string(),
        priority: 1,
        status: GoalProgress::Blocked("dependency missing".to_string()),
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    };
    let snapshot = GoalSnapshot::from(&goal);
    assert!(matches!(snapshot.progress, GoalProgress::Blocked(_)));
}

// --- EnvironmentSnapshot ---

#[test]
fn environment_snapshot_default() {
    let env = EnvironmentSnapshot::default();
    assert!(env.git_status.is_empty());
    assert!(env.open_issues.is_empty());
    assert!(env.recent_commits.is_empty());
}

// --- ActionKind Display ---

#[test]
fn action_kind_display_all_variants() {
    assert_eq!(ActionKind::AdvanceGoal.to_string(), "advance-goal");
    assert_eq!(ActionKind::RunImprovement.to_string(), "run-improvement");
    assert_eq!(
        ActionKind::ConsolidateMemory.to_string(),
        "consolidate-memory"
    );
    assert_eq!(ActionKind::ResearchQuery.to_string(), "research-query");
    assert_eq!(ActionKind::RunGymEval.to_string(), "run-gym-eval");
    assert_eq!(ActionKind::BuildSkill.to_string(), "build-skill");
    assert_eq!(ActionKind::LaunchSession.to_string(), "launch-session");
}

#[test]
fn action_kind_equality() {
    assert_eq!(ActionKind::AdvanceGoal, ActionKind::AdvanceGoal);
    assert_ne!(ActionKind::AdvanceGoal, ActionKind::RunImprovement);
}

// --- OodaConfig ---

#[test]
fn ooda_config_default_values() {
    let config = OodaConfig::default();
    assert_eq!(config.max_concurrent_actions, 3);
    assert!((config.improvement_threshold - 0.02).abs() < f64::EPSILON);
    assert_eq!(config.gym_suite_id, "progressive");
}

// --- Observation / PlannedAction / CycleReport: struct construction ---

#[test]
fn planned_action_construction() {
    let action = PlannedAction {
        kind: ActionKind::BuildSkill,
        goal_id: Some("skill-1".to_string()),
        description: "Build skill".to_string(),
    };
    assert_eq!(action.kind, ActionKind::BuildSkill);
    assert_eq!(action.goal_id.unwrap(), "skill-1");
}

#[test]
fn action_outcome_construction() {
    let outcome = ActionOutcome {
        action: PlannedAction {
            kind: ActionKind::LaunchSession,
            goal_id: None,
            description: "launch".to_string(),
        },
        success: true,
        detail: "session launched".to_string(),
    };
    assert!(outcome.success);
    assert_eq!(outcome.detail, "session launched");
}
