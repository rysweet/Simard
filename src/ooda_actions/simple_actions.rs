//! Simple one-shot action dispatchers (consolidate, research, improve, gym, skill).

use crate::ooda_loop::{ActionOutcome, OodaBridges, PlannedAction};
use crate::self_improve::{ImprovementConfig, run_improvement_cycle, summarize_cycle};
use crate::skill_builder::extract_skill_candidates;

use super::{SKILL_MIN_USAGE, make_outcome};

/// ConsolidateMemory: batch-consolidate episodic memory entries.
pub(super) fn dispatch_consolidate_memory(
    action: &PlannedAction,
    bridges: &OodaBridges,
) -> ActionOutcome {
    match bridges.memory.consolidate_episodes(20) {
        Ok(_) => make_outcome(action, true, "consolidated up to 20 episodes".to_string()),
        Err(e) => make_outcome(action, false, format!("consolidation failed: {e}")),
    }
}

/// ResearchQuery: list available knowledge packs.
pub(super) fn dispatch_research_query(
    action: &PlannedAction,
    bridges: &OodaBridges,
) -> ActionOutcome {
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
pub(super) fn dispatch_run_improvement(
    action: &PlannedAction,
    bridges: &OodaBridges,
) -> ActionOutcome {
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

/// RunGymEval: run the progressive gym suite and return the score.
pub(super) fn dispatch_run_gym_eval(
    action: &PlannedAction,
    bridges: &OodaBridges,
) -> ActionOutcome {
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
pub(super) fn dispatch_build_skill(action: &PlannedAction, bridges: &OodaBridges) -> ActionOutcome {
    match extract_skill_candidates(&*bridges.memory, SKILL_MIN_USAGE) {
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

/// PollDeveloperActivity: fetch recent GitHub activity for tracked developers
/// and store noteworthy events as semantic facts in cognitive memory.
pub(super) fn dispatch_poll_developer_activity(
    action: &PlannedAction,
    bridges: &OodaBridges,
) -> ActionOutcome {
    use crate::research_tracker::{
        default_developer_watches, poll_all_developer_activity, summarize_poll_results,
    };

    let watches = default_developer_watches();
    let results = poll_all_developer_activity(&watches, &*bridges.memory, 5);
    let summary = summarize_poll_results(&results);
    let total_events: usize = results.iter().map(|r| r.events.len()).sum();

    make_outcome(
        action,
        true,
        format!("activity poll complete ({total_events} events): {summary}"),
    )
}

/// ExtractIdeas: analyse recent developer-activity facts in cognitive memory
/// and surface promising research ideas as `research:` issue proposals.
pub(super) fn dispatch_extract_ideas(
    action: &PlannedAction,
    bridges: &OodaBridges,
) -> ActionOutcome {
    use crate::research_tracker::{extract_ideas, summarize_extraction};

    match extract_ideas(&*bridges.memory) {
        Ok(result) => {
            let summary = summarize_extraction(&result);
            make_outcome(action, true, format!("idea extraction complete: {summary}"))
        }
        Err(e) => make_outcome(action, false, format!("idea extraction failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use crate::goal_curation::GoalBoard;
    use crate::ooda_actions::dispatch_actions;
    use crate::ooda_actions::test_helpers::*;
    use crate::ooda_loop::{ActionKind, OodaState, PlannedAction};

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
    fn dispatch_consolidate_memory_succeeds() {
        let mut bridges = test_bridges();
        let mut state = OodaState::new(GoalBoard::new());
        let action = PlannedAction {
            kind: ActionKind::ConsolidateMemory,
            goal_id: None,
            description: "consolidate".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].success);
        assert!(outcomes[0].detail.contains("consolidated"));
    }

    #[test]
    fn dispatch_research_query_lists_packs() {
        let mut bridges = test_bridges();
        let mut state = OodaState::new(GoalBoard::new());
        let action = PlannedAction {
            kind: ActionKind::ResearchQuery,
            goal_id: None,
            description: "research".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].success);
        assert!(outcomes[0].detail.contains("knowledge packs"));
    }

    #[test]
    fn dispatch_multiple_actions_concurrently() {
        let mut bridges = test_bridges();
        let mut state = OodaState::new(GoalBoard::new());
        let actions = vec![
            PlannedAction {
                kind: ActionKind::ConsolidateMemory,
                goal_id: None,
                description: "consolidate".into(),
            },
            PlannedAction {
                kind: ActionKind::ResearchQuery,
                goal_id: None,
                description: "research".into(),
            },
            PlannedAction {
                kind: ActionKind::BuildSkill,
                goal_id: None,
                description: "build".into(),
            },
        ];
        let outcomes = dispatch_actions(&actions, &mut bridges, &mut state).unwrap();
        assert_eq!(outcomes.len(), 3);
        assert!(outcomes.iter().all(|o| o.success));
    }

    #[test]
    fn dispatch_empty_actions_returns_empty() {
        let mut bridges = test_bridges();
        let mut state = OodaState::new(GoalBoard::new());
        let outcomes = dispatch_actions(&[], &mut bridges, &mut state).unwrap();
        assert!(outcomes.is_empty());
    }

    #[test]
    fn make_outcome_preserves_action_fields() {
        use super::make_outcome;
        let action = PlannedAction {
            kind: ActionKind::ResearchQuery,
            goal_id: Some("g-42".into()),
            description: "test action".into(),
        };
        let outcome = make_outcome(&action, true, "details".into());
        assert_eq!(outcome.action.kind, ActionKind::ResearchQuery);
        assert_eq!(outcome.action.goal_id.as_deref(), Some("g-42"));
        assert_eq!(outcome.action.description, "test action");
        assert!(outcome.success);
        assert_eq!(outcome.detail, "details");
    }

    #[test]
    fn make_outcome_failure() {
        use super::make_outcome;
        let action = PlannedAction {
            kind: ActionKind::ConsolidateMemory,
            goal_id: None,
            description: "fail test".into(),
        };
        let outcome = make_outcome(&action, false, "error reason".into());
        assert!(!outcome.success);
        assert_eq!(outcome.detail, "error reason");
    }

    #[test]
    fn skill_min_usage_is_reasonable() {
        const {
            assert!(
                super::SKILL_MIN_USAGE >= 2,
                "skill extraction needs meaningful usage count"
            )
        };
        const {
            assert!(
                super::SKILL_MIN_USAGE <= 10,
                "threshold should not be unreasonably high"
            )
        };
    }
}
