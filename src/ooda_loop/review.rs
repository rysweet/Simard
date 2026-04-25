//! Post-act review: analyze outcomes and generate improvement proposals.

use crate::goals::GoalStatus;
use crate::improvements::ImprovementDirective;

use super::ActionOutcome;

/// Threshold in seconds above which an action is considered slow.
const SLOW_ACTION_THRESHOLD_SECS: u64 = 60;

/// Analyze action outcomes from the Act phase and generate improvement
/// proposals. Three categories:
/// - **fix**: action failed with an error
/// - **optimization**: total act phase took longer than the threshold
/// - **quality**: action succeeded but the detail suggests issues
pub fn review_outcomes(
    outcomes: &[ActionOutcome],
    act_elapsed: std::time::Duration,
) -> Vec<ImprovementDirective> {
    let mut proposals = Vec::new();

    for outcome in outcomes {
        if !outcome.success {
            // Failed action → propose a fix.
            proposals.push(ImprovementDirective {
                title: format!("Fix failed {} action", outcome.action.kind),
                priority: 2,
                status: GoalStatus::Proposed,
                rationale: format!(
                    "Action '{}' failed: {}. Investigate root cause and add resilience.",
                    outcome.action.description, outcome.detail
                ),
            });
        } else if outcome.detail.to_lowercase().contains("warning")
            || outcome.detail.to_lowercase().contains("partial")
            || outcome.detail.to_lowercase().contains("degraded")
        {
            // Succeeded with quality concerns → propose quality improvement.
            proposals.push(ImprovementDirective {
                title: format!("Improve quality of {} action", outcome.action.kind),
                priority: 3,
                status: GoalStatus::Proposed,
                rationale: format!(
                    "Action '{}' succeeded but detail indicates issues: {}",
                    outcome.action.description, outcome.detail
                ),
            });
        }
    }

    // If the entire act phase was slow, propose an optimization.
    if act_elapsed.as_secs() > SLOW_ACTION_THRESHOLD_SECS {
        proposals.push(ImprovementDirective {
            title: "Optimize slow OODA act phase".to_string(),
            priority: 3,
            status: GoalStatus::Proposed,
            rationale: format!(
                "Act phase took {:.1}s (threshold: {}s). Consider parallelizing actions or reducing scope.",
                act_elapsed.as_secs_f64(),
                SLOW_ACTION_THRESHOLD_SECS
            ),
        });
    }

    proposals
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goals::GoalStatus;
    use crate::ooda_loop::{ActionKind, ActionOutcome, PlannedAction};

    #[test]
    fn review_outcomes_generates_fix_for_failed_action() {
        let outcomes = vec![ActionOutcome {
            action: PlannedAction {
                kind: ActionKind::AdvanceGoal,
                goal_id: Some("g1".to_string()),
                description: "advance g1".to_string(),
            },
            success: false,
            detail: "timeout during execution".to_string(),
        }];
        let proposals = review_outcomes(&outcomes, std::time::Duration::from_secs(5));
        assert_eq!(proposals.len(), 1);
        assert!(proposals[0].title.contains("Fix failed"));
        assert_eq!(proposals[0].priority, 2);
        assert_eq!(proposals[0].status, GoalStatus::Proposed);
    }

    #[test]
    fn review_outcomes_generates_quality_for_warning() {
        let outcomes = vec![ActionOutcome {
            action: PlannedAction {
                kind: ActionKind::RunImprovement,
                goal_id: None,
                description: "run improvement".to_string(),
            },
            success: true,
            detail: "completed with warning: unstable test".to_string(),
        }];
        let proposals = review_outcomes(&outcomes, std::time::Duration::from_secs(5));
        assert_eq!(proposals.len(), 1);
        assert!(proposals[0].title.contains("Improve quality"));
        assert_eq!(proposals[0].priority, 3);
    }

    #[test]
    fn review_outcomes_generates_quality_for_partial() {
        let outcomes = vec![ActionOutcome {
            action: PlannedAction {
                kind: ActionKind::AdvanceGoal,
                goal_id: Some("g1".to_string()),
                description: "partial work".to_string(),
            },
            success: true,
            detail: "partial success: only 2 of 5 steps completed".to_string(),
        }];
        let proposals = review_outcomes(&outcomes, std::time::Duration::from_secs(5));
        assert_eq!(proposals.len(), 1);
        assert!(proposals[0].title.contains("quality"));
    }

    #[test]
    fn review_outcomes_generates_quality_for_degraded() {
        let outcomes = vec![ActionOutcome {
            action: PlannedAction {
                kind: ActionKind::ConsolidateMemory,
                goal_id: None,
                description: "memory work".to_string(),
            },
            success: true,
            detail: "degraded performance observed".to_string(),
        }];
        let proposals = review_outcomes(&outcomes, std::time::Duration::from_secs(5));
        assert_eq!(proposals.len(), 1);
    }

    #[test]
    fn review_outcomes_generates_optimization_for_slow_phase() {
        let outcomes = vec![ActionOutcome {
            action: PlannedAction {
                kind: ActionKind::AdvanceGoal,
                goal_id: Some("g1".to_string()),
                description: "normal".to_string(),
            },
            success: true,
            detail: "done".to_string(),
        }];
        let proposals = review_outcomes(&outcomes, std::time::Duration::from_secs(120));
        assert_eq!(proposals.len(), 1);
        assert!(proposals[0].title.contains("Optimize slow"));
    }

    #[test]
    fn review_outcomes_no_proposals_for_clean_fast_success() {
        let outcomes = vec![ActionOutcome {
            action: PlannedAction {
                kind: ActionKind::AdvanceGoal,
                goal_id: Some("g1".to_string()),
                description: "advance".to_string(),
            },
            success: true,
            detail: "completed successfully".to_string(),
        }];
        let proposals = review_outcomes(&outcomes, std::time::Duration::from_secs(10));
        assert!(proposals.is_empty());
    }

    #[test]
    fn review_outcomes_empty_outcomes() {
        let proposals = review_outcomes(&[], std::time::Duration::from_secs(5));
        assert!(proposals.is_empty());
    }
}
