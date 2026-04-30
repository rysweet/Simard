//! Decide phase: select actions from priorities, capped by concurrency limit.
//!
//! The action-kind selection (which kind of [`ActionKind`] each priority maps
//! to) is delegated to a prompt-driven brain — see
//! `prompt_assets/simard/ooda_decide.md`. The default entrypoint
//! ([`decide`]) wires in [`DeterministicFallbackDecideBrain`], which preserves
//! the pre-#1458 prefix-mapping bit-for-bit so the daemon never depends on
//! LLM availability for Decide. Callers that have an LLM-backed brain can
//! invoke [`decide_with_brain`] directly.

use crate::error::SimardResult;
use crate::ooda_brain::{
    BrainJudgmentRecord, DecideContext, DeterministicFallbackDecideBrain, OodaDecideBrain,
    push_brain_judgment,
};

use super::{OodaConfig, PlannedAction, Priority};

/// Decide using the deterministic fallback brain. This is the entrypoint
/// the daemon's Act phase calls today; it preserves the pre-#1458 routing
/// bit-for-bit (no LLM dependency).
#[tracing::instrument(skip_all)]
pub fn decide(priorities: &[Priority], config: &OodaConfig) -> SimardResult<Vec<PlannedAction>> {
    let brain = DeterministicFallbackDecideBrain;
    decide_with_brain(priorities, config, &brain)
}

/// Decide using a caller-supplied brain. Used by tests and (in a future
/// wire-in) by the daemon when an LLM-backed brain is configured. On any
/// brain error for an individual priority, falls back to the deterministic
/// mapping for that priority so a transient adapter failure cannot stall
/// the cycle.
#[tracing::instrument(skip_all)]
pub fn decide_with_brain(
    priorities: &[Priority],
    config: &OodaConfig,
    brain: &dyn OodaDecideBrain,
) -> SimardResult<Vec<PlannedAction>> {
    let limit = config.max_concurrent_actions as usize;
    let fallback = DeterministicFallbackDecideBrain;
    let mut actions = Vec::with_capacity(limit);
    for priority in priorities {
        if actions.len() >= limit {
            break;
        }
        if priority.urgency < f64::EPSILON {
            continue;
        }
        let ctx = DecideContext {
            goal_id: priority.goal_id.clone(),
            urgency: priority.urgency,
            reason: priority.reason.clone(),
        };
        let judgment = match brain.judge_decision(&ctx) {
            Ok(j) => {
                push_brain_judgment(BrainJudgmentRecord::from_decide(
                    &priority.goal_id,
                    priority.urgency,
                    &j,
                    false,
                ));
                j
            }
            Err(_) => {
                let j = fallback.judge_decision(&ctx)?;
                push_brain_judgment(BrainJudgmentRecord::from_decide(
                    &priority.goal_id,
                    priority.urgency,
                    &j,
                    true,
                ));
                j
            }
        };
        actions.push(PlannedAction {
            kind: judgment.action_kind(),
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
    use crate::ooda_brain::{DecideContext, DecideJudgment, OodaDecideBrain};
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

    #[test]
    fn decide_maps_extract_ideas_priority() {
        let priorities = vec![Priority {
            goal_id: "__extract_ideas__".to_string(),
            urgency: 0.6,
            reason: "surface research ideas from activity".to_string(),
        }];
        let config = OodaConfig::default();
        let actions = decide(&priorities, &config).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, ActionKind::ExtractIdeas);
        assert!(actions[0].goal_id.is_none());
    }

    // -----------------------------------------------------------------------
    // Brain wire-in tests: prove the brain's choice flows through and that
    // a brain error transparently falls back to the deterministic mapping.
    // -----------------------------------------------------------------------

    #[test]
    fn decide_with_brain_uses_brain_judgment_for_action_kind() {
        struct AlwaysGymBrain;
        impl OodaDecideBrain for AlwaysGymBrain {
            fn judge_decision(
                &self,
                _ctx: &DecideContext,
            ) -> crate::error::SimardResult<DecideJudgment> {
                Ok(DecideJudgment::RunGymEval {
                    rationale: "stub".to_string(),
                })
            }
        }
        let priorities = vec![Priority {
            goal_id: "ship-v1".to_string(),
            urgency: 0.9,
            reason: "test".to_string(),
        }];
        let config = OodaConfig::default();
        let actions = decide_with_brain(&priorities, &config, &AlwaysGymBrain).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, ActionKind::RunGymEval);
    }

    #[test]
    fn decide_with_brain_falls_back_on_brain_error() {
        struct AlwaysErrBrain;
        impl OodaDecideBrain for AlwaysErrBrain {
            fn judge_decision(
                &self,
                _ctx: &DecideContext,
            ) -> crate::error::SimardResult<DecideJudgment> {
                Err(crate::error::SimardError::AdapterInvocationFailed {
                    base_type: "test".to_string(),
                    reason: "boom".to_string(),
                })
            }
        }
        let priorities = vec![Priority {
            goal_id: "__memory__".to_string(),
            urgency: 0.5,
            reason: "fallback expected".to_string(),
        }];
        let config = OodaConfig::default();
        let actions = decide_with_brain(&priorities, &config, &AlwaysErrBrain).unwrap();
        // Fallback maps __memory__ → ConsolidateMemory, preserving pre-#1458 behaviour.
        assert_eq!(actions[0].kind, ActionKind::ConsolidateMemory);
    }
}
