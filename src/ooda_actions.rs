//! Action dispatch for the OODA loop.
//!
//! Extracted from `ooda_loop.rs` to keep each module under 400 LOC.
//! Each [`ActionKind`] maps to a concrete subsystem call. Failures are
//! per-action, not cycle-wide (Pillar 11: honest degradation).

use crate::agent_supervisor::{HeartbeatStatus, check_heartbeat};
use crate::error::SimardResult;
use crate::goal_curation::{GoalProgress, update_goal_progress};
use crate::ooda_loop::{ActionKind, ActionOutcome, OodaBridges, OodaState, PlannedAction};
use crate::self_improve::{ImprovementConfig, run_improvement_cycle, summarize_cycle};
use crate::skill_builder::extract_skill_candidates;

/// Minimum procedure usage count required for skill extraction.
const SKILL_MIN_USAGE: u32 = 3;

/// Advance a goal's progress by one step: `NotStarted → InProgress(10)`,
/// `InProgress(N) → InProgress(N+10)` or `Completed` at 100.
fn next_progress(current: &GoalProgress) -> GoalProgress {
    match current {
        GoalProgress::NotStarted => GoalProgress::InProgress { percent: 10 },
        GoalProgress::InProgress { percent } => {
            let next = (*percent + 10).min(100);
            if next >= 100 {
                GoalProgress::Completed
            } else {
                GoalProgress::InProgress { percent: next }
            }
        }
        other => other.clone(),
    }
}

/// Construct an [`ActionOutcome`] from the shared action reference.
///
/// Centralises the single unavoidable clone of the [`PlannedAction`] so
/// dispatch helpers only need `(action, success, detail)`.
#[inline]
fn make_outcome(action: &PlannedAction, success: bool, detail: String) -> ActionOutcome {
    ActionOutcome {
        action: action.clone(),
        success,
        detail,
    }
}

/// Dispatch a batch of planned actions against live bridges and state.
///
/// Each action is dispatched independently; a failure in one does not
/// abort the others. Returns one [`ActionOutcome`] per input action.
pub fn dispatch_actions(
    actions: &[PlannedAction],
    bridges: &mut OodaBridges,
    state: &mut OodaState,
) -> SimardResult<Vec<ActionOutcome>> {
    let mut outcomes = Vec::with_capacity(actions.len());
    for action in actions {
        let outcome = dispatch_one(action, bridges, state);
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

/// Dispatch a single planned action and return its outcome.
fn dispatch_one(
    action: &PlannedAction,
    bridges: &mut OodaBridges,
    state: &mut OodaState,
) -> ActionOutcome {
    match action.kind {
        ActionKind::ConsolidateMemory => dispatch_consolidate_memory(action, bridges),
        ActionKind::ResearchQuery => dispatch_research_query(action, bridges),
        ActionKind::RunImprovement => dispatch_run_improvement(action, bridges),
        ActionKind::AdvanceGoal => dispatch_advance_goal(action, bridges, state),
        ActionKind::RunGymEval => dispatch_run_gym_eval(action, bridges),
        ActionKind::BuildSkill => dispatch_build_skill(action, bridges),
    }
}

/// ConsolidateMemory: batch-consolidate episodic memory entries.
fn dispatch_consolidate_memory(action: &PlannedAction, bridges: &OodaBridges) -> ActionOutcome {
    match bridges.memory.consolidate_episodes(20) {
        Ok(_) => make_outcome(action, true, "consolidated up to 20 episodes".to_string()),
        Err(e) => make_outcome(action, false, format!("consolidation failed: {e}")),
    }
}

/// ResearchQuery: list available knowledge packs.
fn dispatch_research_query(action: &PlannedAction, bridges: &OodaBridges) -> ActionOutcome {
    match bridges.knowledge.list_packs() {
        Ok(packs) => make_outcome(
            action,
            true,
            format!("found {} knowledge packs", packs.len()),
        ),
        Err(e) => make_outcome(action, false, format!("knowledge query failed: {e}")),
    }
}

/// RunImprovement: execute a full improvement cycle via the gym bridge.
///
/// Uses default improvement config (progressive suite, 2% threshold).
/// The cycle evaluates baseline, applies no changes (empty proposals),
/// and returns the analysis. A real caller would populate proposed_changes
/// from the orient/decide phases.
fn dispatch_run_improvement(action: &PlannedAction, bridges: &OodaBridges) -> ActionOutcome {
    let config = ImprovementConfig::default();
    match run_improvement_cycle(&bridges.gym, &config) {
        Ok(cycle) => {
            let summary = summarize_cycle(&cycle);
            let committed = matches!(
                cycle.decision,
                Some(crate::self_improve::ImprovementDecision::Commit { .. })
            );
            make_outcome(
                action,
                true,
                format!("improvement cycle completed (committed={committed}): {summary}"),
            )
        }
        Err(e) => make_outcome(action, false, format!("improvement cycle failed: {e}")),
    }
}

/// AdvanceGoal: progress the target goal on the board.
///
/// If the goal has a subordinate assigned, checks the subordinate's
/// heartbeat via the supervisor. If a base-type session is available
/// (e.g. RustyClawd), delegates the goal to the agent via `run_turn`
/// for real autonomous work. Otherwise, falls back to bumping the
/// progress percentage.
fn dispatch_advance_goal(
    action: &PlannedAction,
    bridges: &mut OodaBridges,
    state: &mut OodaState,
) -> ActionOutcome {
    let goal_id = match &action.goal_id {
        Some(id) => id.clone(),
        None => {
            return make_outcome(action, false, "advance-goal requires a goal_id".to_string());
        }
    };

    // Find the goal on the board.
    let goal = match state.active_goals.active.iter().find(|g| g.id == goal_id) {
        Some(g) => g.clone(),
        None => {
            return make_outcome(
                action,
                false,
                format!("goal '{goal_id}' not found on active board"),
            );
        }
    };

    // If the goal has a subordinate, check heartbeat.
    if let Some(ref sub_name) = goal.assigned_to {
        return advance_goal_with_subordinate(action, bridges, state, &goal_id, sub_name);
    }

    // Blocked and completed goals short-circuit before session dispatch.
    match &goal.status {
        GoalProgress::Blocked(reason) => {
            return make_outcome(
                action,
                false,
                format!("goal '{goal_id}' is blocked: {reason}"),
            );
        }
        GoalProgress::Completed => {
            return make_outcome(
                action,
                true,
                format!("goal '{goal_id}' is already completed"),
            );
        }
        _ => {}
    }

    // If a base-type session is available, use run_turn for real agent work.
    if let Some(ref mut session) = bridges.session {
        return advance_goal_with_session(action, session.as_mut(), state, &goal);
    }

    // Fallback: no session available — advance progress by bumping percentage.
    let new_progress = next_progress(&goal.status);

    match update_goal_progress(&mut state.active_goals, &goal_id, new_progress.clone()) {
        Ok(()) => make_outcome(
            action,
            true,
            format!("goal '{goal_id}' advanced to {new_progress}"),
        ),
        Err(e) => make_outcome(
            action,
            false,
            format!("failed to update goal '{goal_id}': {e}"),
        ),
    }
}

/// Advance a goal using a base-type session's `run_turn`.
///
/// Constructs an objective that includes environment context (git status,
/// open issues, recent commits) so the agent can make informed decisions.
/// Updates goal progress based on the turn outcome.
fn advance_goal_with_session(
    action: &PlannedAction,
    session: &mut dyn crate::base_types::BaseTypeSession,
    state: &mut OodaState,
    goal: &crate::goal_curation::ActiveGoal,
) -> ActionOutcome {
    use crate::base_types::BaseTypeTurnInput;

    let percent = match &goal.status {
        GoalProgress::InProgress { percent } => *percent,
        _ => 0,
    };

    // Gather fresh environment context so the agent sees current state.
    let env = crate::ooda_loop::gather_environment();
    let env_context = format!(
        "\n\nEnvironment context:\n- Git status: {}\n- Open issues: {}\n- Recent commits: {}",
        if env.git_status.is_empty() {
            "clean".to_string()
        } else {
            format!("{} changed files", env.git_status.lines().count())
        },
        if env.open_issues.is_empty() {
            "none".to_string()
        } else {
            env.open_issues.join("; ")
        },
        if env.recent_commits.is_empty() {
            "none".to_string()
        } else {
            env.recent_commits[..env.recent_commits.len().min(5)].join("; ")
        },
    );

    let objective = format!(
        "Goal '{}' ({}% complete): {}. Assess current progress and take one bounded action to advance this goal.{}",
        goal.id, percent, goal.description, env_context,
    );

    let input = BaseTypeTurnInput::objective_only(&objective);

    match session.run_turn(input) {
        Ok(outcome) => {
            let new_progress = next_progress(&goal.status);
            let _ = update_goal_progress(&mut state.active_goals, &goal.id, new_progress.clone());

            eprintln!(
                "[simard] OODA session result: advance-goal '{}': {}",
                goal.id, outcome.execution_summary
            );

            make_outcome(
                action,
                true,
                format!(
                    "goal '{}' advanced to {} via session (evidence={})",
                    goal.id,
                    new_progress,
                    outcome.evidence.len(),
                ),
            )
        }
        Err(e) => make_outcome(
            action,
            false,
            format!("session run_turn failed for goal '{}': {e}", goal.id),
        ),
    }
}

/// Advance a goal that has a subordinate assigned by checking heartbeat.
fn advance_goal_with_subordinate(
    action: &PlannedAction,
    bridges: &mut OodaBridges,
    state: &mut OodaState,
    goal_id: &str,
    sub_name: &str,
) -> ActionOutcome {
    // Build a minimal handle for heartbeat checking.
    let handle = crate::agent_supervisor::SubordinateHandle {
        pid: 0,
        agent_name: sub_name.to_string(),
        goal: goal_id.to_string(),
        worktree_path: std::path::PathBuf::from("."),
        spawn_time: 0,
        retry_count: 0,
        killed: false,
    };

    match check_heartbeat(&handle, &bridges.memory) {
        Ok(HeartbeatStatus::Alive { phase, .. }) => {
            // Subordinate is alive; update goal to in-progress if not already.
            let new_progress = GoalProgress::InProgress { percent: 50 };
            let _ = update_goal_progress(&mut state.active_goals, goal_id, new_progress);
            make_outcome(
                action,
                true,
                format!(
                    "subordinate '{sub_name}' alive (phase={phase}), goal '{goal_id}' in-progress"
                ),
            )
        }
        Ok(HeartbeatStatus::Stale { seconds_since }) => make_outcome(
            action,
            false,
            format!(
                "subordinate '{sub_name}' stale ({seconds_since}s), goal '{goal_id}' may need reassignment"
            ),
        ),
        Ok(HeartbeatStatus::Dead) => {
            let _ = update_goal_progress(
                &mut state.active_goals,
                goal_id,
                GoalProgress::Blocked(format!("subordinate '{sub_name}' is dead")),
            );
            make_outcome(
                action,
                false,
                format!("subordinate '{sub_name}' is dead, goal '{goal_id}' blocked"),
            )
        }
        Err(e) => make_outcome(
            action,
            false,
            format!("heartbeat check failed for subordinate '{sub_name}': {e}"),
        ),
    }
}

/// RunGymEval: run the progressive gym suite and return the score.
fn dispatch_run_gym_eval(action: &PlannedAction, bridges: &OodaBridges) -> ActionOutcome {
    match bridges.gym.run_suite("progressive") {
        Ok(result) => {
            use crate::gym_scoring::suite_score_from_result;
            let score = suite_score_from_result(&result);
            make_outcome(
                action,
                true,
                format!(
                    "gym eval: {:.1}% overall, {}/{} passed",
                    score.overall * 100.0,
                    score.scenarios_passed,
                    score.scenario_count,
                ),
            )
        }
        Err(e) => make_outcome(action, false, format!("gym eval failed: {e}")),
    }
}

/// BuildSkill: extract skill candidates from procedural memory.
fn dispatch_build_skill(action: &PlannedAction, bridges: &OodaBridges) -> ActionOutcome {
    match extract_skill_candidates(&bridges.memory, SKILL_MIN_USAGE) {
        Ok(candidates) => {
            let names: Vec<&str> = candidates.iter().map(|c| c.name.as_str()).collect();
            make_outcome(
                action,
                true,
                format!(
                    "extracted {} skill candidates: [{}]",
                    candidates.len(),
                    names.join(", ")
                ),
            )
        }
        Err(e) => make_outcome(action, false, format!("skill extraction failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::BridgeErrorPayload;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, add_active_goal};
    use crate::gym_bridge::GymBridge;
    use crate::knowledge_bridge::KnowledgeBridge;
    use crate::memory_bridge::CognitiveMemoryBridge;
    use serde_json::json;

    fn mock_memory() -> CognitiveMemoryBridge {
        CognitiveMemoryBridge::new(Box::new(InMemoryBridgeTransport::new(
            "test-mem",
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
                _ => Err(BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            },
        )))
    }

    fn mock_gym() -> GymBridge {
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

    fn mock_knowledge() -> KnowledgeBridge {
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

    fn test_bridges() -> OodaBridges {
        OodaBridges {
            memory: mock_memory(),
            knowledge: mock_knowledge(),
            gym: mock_gym(),
            session: None,
        }
    }

    fn board_with_goal(id: &str, progress: GoalProgress, assigned: Option<&str>) -> GoalBoard {
        let mut board = GoalBoard::new();
        add_active_goal(
            &mut board,
            ActiveGoal {
                id: id.to_string(),
                description: format!("Goal {id}"),
                priority: 1,
                status: progress,
                assigned_to: assigned.map(String::from),
            },
        )
        .unwrap();
        board
    }

    #[test]
    fn dispatch_run_improvement_calls_gym() {
        let mut bridges = test_bridges();
        let action = PlannedAction {
            kind: ActionKind::RunImprovement,
            goal_id: None,
            description: "test".into(),
        };
        let mut state = OodaState::new(GoalBoard::new());
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].success);
        assert!(outcomes[0].detail.contains("improvement cycle completed"));
    }

    #[test]
    fn dispatch_advance_goal_not_started_becomes_in_progress() {
        let mut bridges = test_bridges();
        let board = board_with_goal("g1", GoalProgress::NotStarted, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert!(outcomes[0].success);
        assert!(outcomes[0].detail.contains("in-progress"));
        assert!(matches!(
            state.active_goals.active[0].status,
            GoalProgress::InProgress { percent: 10 }
        ));
    }

    #[test]
    fn dispatch_advance_goal_blocked_fails() {
        let mut bridges = test_bridges();
        let board = board_with_goal("g1", GoalProgress::Blocked("waiting".into()), None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert!(!outcomes[0].success);
        assert!(outcomes[0].detail.contains("blocked"));
    }

    #[test]
    fn dispatch_advance_goal_missing_id_fails() {
        let mut bridges = test_bridges();
        let mut state = OodaState::new(GoalBoard::new());
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: None,
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert!(!outcomes[0].success);
        assert!(outcomes[0].detail.contains("requires a goal_id"));
    }

    #[test]
    fn dispatch_run_gym_eval_returns_score() {
        let mut bridges = test_bridges();
        let mut state = OodaState::new(GoalBoard::new());
        let action = PlannedAction {
            kind: ActionKind::RunGymEval,
            goal_id: None,
            description: "eval".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert!(outcomes[0].success);
        assert!(outcomes[0].detail.contains("gym eval"));
        assert!(outcomes[0].detail.contains("75.0%"));
    }

    #[test]
    fn dispatch_build_skill_extracts_candidates() {
        let mut bridges = test_bridges();
        let mut state = OodaState::new(GoalBoard::new());
        let action = PlannedAction {
            kind: ActionKind::BuildSkill,
            goal_id: None,
            description: "build".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert!(outcomes[0].success);
        assert!(outcomes[0].detail.contains("cargo-build"));
    }

    #[test]
    fn dispatch_advance_goal_with_dead_subordinate_blocks() {
        let mut bridges = test_bridges();
        let board = board_with_goal("g1", GoalProgress::NotStarted, Some("sub-1"));
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        // No progress facts in memory means Dead heartbeat.
        assert!(!outcomes[0].success);
        assert!(outcomes[0].detail.contains("dead"));
    }
}
