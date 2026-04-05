//! Decide phase: select actions from priorities, capped by concurrency limit.

use crate::error::SimardResult;

use super::{ActionKind, OodaConfig, PlannedAction, Priority};

/// Decide: select actions from priorities, capped by `max_concurrent_actions`.
pub fn decide(priorities: &[Priority], config: &OodaConfig) -> SimardResult<Vec<PlannedAction>> {
    let limit = config.max_concurrent_actions as usize;
    let mut actions = Vec::with_capacity(limit);
    for priority in priorities {
        if actions.len() >= limit {
            break;
        }
        if priority.urgency < f64::EPSILON {
            continue;
        }
        let kind = match priority.goal_id.as_str() {
            "__memory__" => ActionKind::ConsolidateMemory,
            "__improvement__" => ActionKind::RunImprovement,
            _ => ActionKind::AdvanceGoal,
        };
        actions.push(PlannedAction {
            kind,
            goal_id: if priority.goal_id.starts_with("__") {
                None
            } else {
                Some(priority.goal_id.clone())
            },
            description: priority.reason.clone(),
        });
    }
    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ooda_loop::{ActionKind, OodaConfig, Priority};

    #[test]
    fn decide_respects_max_concurrent_actions() {
        let priorities = vec![
            Priority {
                goal_id: "g1".to_string(),
                urgency: 0.9,
                reason: "a".to_string(),
            },
            Priority {
                goal_id: "g2".to_string(),
                urgency: 0.8,
                reason: "b".to_string(),
            },
            Priority {
                goal_id: "g3".to_string(),
                urgency: 0.7,
                reason: "c".to_string(),
            },
            Priority {
                goal_id: "g4".to_string(),
                urgency: 0.6,
                reason: "d".to_string(),
            },
        ];
        let config = OodaConfig {
            max_concurrent_actions: 2,
            ..Default::default()
        };
        let actions = decide(&priorities, &config).unwrap();
        assert_eq!(actions.len(), 2);
    }

    #[test]
    fn decide_skips_zero_urgency_priorities() {
        let priorities = vec![
            Priority {
                goal_id: "g1".to_string(),
                urgency: 0.5,
                reason: "a".to_string(),
            },
            Priority {
                goal_id: "g2".to_string(),
                urgency: 0.0,
                reason: "done".to_string(),
            },
        ];
        let config = OodaConfig::default();
        let actions = decide(&priorities, &config).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].goal_id, Some("g1".to_string()));
    }

    #[test]
    fn decide_maps_memory_priority_to_consolidate_action() {
        let priorities = vec![Priority {
            goal_id: "__memory__".to_string(),
            urgency: 0.5,
            reason: "too many memories".to_string(),
        }];
        let config = OodaConfig::default();
        let actions = decide(&priorities, &config).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, ActionKind::ConsolidateMemory);
        assert!(actions[0].goal_id.is_none());
    }

    #[test]
    fn decide_maps_improvement_priority_to_run_improvement() {
        let priorities = vec![Priority {
            goal_id: "__improvement__".to_string(),
            urgency: 0.7,
            reason: "gym below target".to_string(),
        }];
        let config = OodaConfig::default();
        let actions = decide(&priorities, &config).unwrap();
        assert_eq!(actions[0].kind, ActionKind::RunImprovement);
        assert!(actions[0].goal_id.is_none());
    }

    #[test]
    fn decide_maps_regular_goal_to_advance_goal() {
        let priorities = vec![Priority {
            goal_id: "ship-v1".to_string(),
            urgency: 0.9,
            reason: "high priority".to_string(),
        }];
        let config = OodaConfig::default();
        let actions = decide(&priorities, &config).unwrap();
        assert_eq!(actions[0].kind, ActionKind::AdvanceGoal);
        assert_eq!(actions[0].goal_id, Some("ship-v1".to_string()));
    }

    #[test]
    fn decide_empty_priorities_returns_empty() {
        let config = OodaConfig::default();
        let actions = decide(&[], &config).unwrap();
        assert!(actions.is_empty());
    }

    #[test]
    fn decide_preserves_reason_as_description() {
        let priorities = vec![Priority {
            goal_id: "g1".to_string(),
            urgency: 0.5,
            reason: "important task".to_string(),
        }];
        let config = OodaConfig::default();
        let actions = decide(&priorities, &config).unwrap();
        assert_eq!(actions[0].description, "important task");
    }
}
