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
use crate::ooda_brain::parse_failure::{record_parse_failure, reset_consecutive_count};
use crate::ooda_brain::{
    BrainJudgmentRecord, BrainPhase, DECIDE_PROMPT_NAME, DecideContext,
    DeterministicFallbackDecideBrain, OodaDecideBrain, push_brain_judgment,
};

use super::{OodaConfig, PlannedAction, Priority, is_synthetic_id};

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
/// the cycle — but the failure is recorded LOUDLY (issue #1890) via
/// `ParseFailureRecord` so the silent-fallback regression cannot recur.
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
                // Healthy parse — reset the (Decide, goal_id) counter so a
                // recovery cancels any pending gh-issue escalation.
                reset_consecutive_count(BrainPhase::Decide, &priority.goal_id);
                push_brain_judgment(BrainJudgmentRecord::from_decide(
                    &priority.goal_id,
                    priority.urgency,
                    &j,
                    false,
                    crate::ooda_brain::prompt_store::current_version(DECIDE_PROMPT_NAME),
                ));
                j
            }
            Err(e) => {
                // Issue #1890: surface the parse failure on all four
                // visibility channels (tracing, metric, cycle JSON,
                // throttled gh issue at >= 3 consecutive). Cycle still
                // continues via the deterministic fallback action so a
                // transient adapter hiccup cannot stall the loop.
                let raw_response = extract_raw_response(&e);
                let pf = record_parse_failure(
                    BrainPhase::Decide,
                    &priority.goal_id,
                    &e,
                    &raw_response,
                    DECIDE_PROMPT_NAME,
                    crate::ooda_brain::prompt_store::current_version(DECIDE_PROMPT_NAME),
                );
                let j = fallback.judge_decision(&ctx)?;
                let mut rec = BrainJudgmentRecord::from_decide(
                    &priority.goal_id,
                    priority.urgency,
                    &j,
                    true,
                    String::new(),
                );
                rec.parse_failure = Some(pf);
                push_brain_judgment(rec);
                j
            }
        };
        actions.push(PlannedAction {
            kind: judgment.action_kind(),
            goal_id: if is_synthetic_id(&priority.goal_id) {
                None
            } else {
                Some(priority.goal_id.clone())
            },
            description: priority.reason.clone(),
        });
    }
    Ok(actions)
}

/// Recover the raw model response from a brain error message.
///
/// Brain parsers embed the model body in the error reason as
/// `raw_response={:?}` (Debug-format). We extract everything after the
/// first `raw_response=` marker, then strip the surrounding double-quotes
/// best-effort. If the marker is absent (non-parse error variants), we
/// return the full error string so the operator still gets context.
fn extract_raw_response(err: &crate::error::SimardError) -> String {
    let msg = err.to_string();
    if let Some(start) = msg.find("raw_response=") {
        let tail = &msg[start + "raw_response=".len()..];
        let tail = tail.trim_start();
        if let Some(rest) = tail.strip_prefix('"') {
            // Trim a trailing `")` or `"` (rustyclawd uses `({:?})` shape).
            let body = rest
                .strip_suffix("\")")
                .or_else(|| rest.strip_suffix('"'))
                .unwrap_or(rest);
            return body.to_string();
        }
        return tail.to_string();
    }
    msg
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

    #[test]
    fn decide_maps_safe_update_priority() {
        let priorities = vec![Priority {
            goal_id: "__safe_update__".to_string(),
            urgency: 0.8,
            reason: "binary 5 commits behind, conditions met".to_string(),
        }];
        let config = OodaConfig::default();
        let actions = decide(&priorities, &config).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, ActionKind::SafeUpdate);
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
    fn decide_with_brain_records_brain_rationale_not_fallback_marker() {
        // Wiring test: when an LLM-backed brain is provided, the rationale
        // recorded on the per-cycle BrainJudgmentRecord must be the brain's
        // own rationale, NOT the deterministic-fallback's
        // `"fallback-brain: prefix-routed"` marker. This proves the
        // daemon's #1469 wire-up actually fires the LLM brain.
        struct LlmStubBrain;
        impl OodaDecideBrain for LlmStubBrain {
            fn judge_decision(
                &self,
                _ctx: &DecideContext,
            ) -> crate::error::SimardResult<DecideJudgment> {
                Ok(DecideJudgment::AdvanceGoal {
                    rationale: "llm-brain: high-leverage progress".to_string(),
                })
            }
        }
        let priorities = vec![Priority {
            goal_id: "ship-v1".to_string(),
            urgency: 0.9,
            reason: "test".to_string(),
        }];
        let config = OodaConfig::default();
        let records = crate::ooda_brain::with_brain_judgment_scope(|| {
            crate::ooda_brain::clear_brain_judgments();
            decide_with_brain(&priorities, &config, &LlmStubBrain).unwrap();
            crate::ooda_brain::take_brain_judgments()
        });
        assert_eq!(records.len(), 1);
        assert!(
            !records[0].rationale.contains("fallback-brain"),
            "expected LLM-brain rationale, got fallback marker: {}",
            records[0].rationale,
        );
        assert_eq!(records[0].rationale, "llm-brain: high-leverage progress");
        assert!(!records[0].fallback);
    }
}
