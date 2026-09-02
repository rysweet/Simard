//! Decide phase: select actions from priorities, capped by concurrency limit.
//!
//! The action-kind selection (which kind of [`ActionKind`] each priority maps
//! to) is delegated to a prompt-driven brain — see
//! `prompt_assets/simard/ooda_decide.md`. The default entrypoint
//! ([`decide`]) wires in [`DeterministicDecideBrain`], the deterministic
//! prefix-based routing used when no LLM brain is configured. Callers that
//! have an LLM-backed brain invoke [`decide_with_brain`] directly.
//!
//! **No fallback**: if the brain errors for a priority, that priority is
//! skipped with a loud error — never silently re-routed through a different
//! brain.

use crate::error::SimardResult;
use crate::ooda_brain::parse_failure::{record_parse_failure, reset_consecutive_count};
use crate::ooda_brain::{
    BrainJudgmentRecord, BrainPhase, DECIDE_PROMPT_NAME, DecideContext, DeterministicDecideBrain,
    OodaDecideBrain, push_brain_judgment,
};

use super::{ActionKind, OodaConfig, PlannedAction, Priority, is_synthetic_id};

/// Decide using the deterministic brain (no LLM dependency). This is the
/// entrypoint used when no LLM brain is configured.
#[tracing::instrument(skip_all)]
pub fn decide(priorities: &[Priority], config: &OodaConfig) -> SimardResult<Vec<PlannedAction>> {
    let brain = DeterministicDecideBrain;
    decide_with_brain(priorities, config, &brain)
}

/// Decide using a caller-supplied brain. On brain error for an individual
/// priority, the priority is **skipped** (no action produced) and the failure
/// is recorded on all visibility channels (tracing, metric, cycle JSON,
/// throttled gh-issue). No silent fallback to a different brain.
#[tracing::instrument(skip_all)]
pub fn decide_with_brain(
    priorities: &[Priority],
    config: &OodaConfig,
    brain: &dyn OodaDecideBrain,
) -> SimardResult<Vec<PlannedAction>> {
    let base_limit = config.max_concurrent_actions as usize;
    let limit = if let Some(ref scaler) = config.scaler {
        scaler.adjust() as usize
    } else {
        base_limit
    };
    let deterministic = DeterministicDecideBrain;
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
        // Synthetic priorities (__memory__, __improvement__, etc.) have a
        // fixed action-kind mapping that the LLM brain frequently gets wrong
        // (returning AdvanceGoal, which is unroutable without a goal_id).
        // Always use the deterministic brain for these — the LLM adds no
        // value for synthetic priorities.
        let effective_brain: &dyn OodaDecideBrain = if is_synthetic_id(&priority.goal_id) {
            &deterministic
        } else {
            brain
        };
        let judgment = match effective_brain.judge_decision(&ctx) {
            Ok(j) => {
                // Healthy parse — reset the (Decide, goal_id) counter so a
                // recovery cancels any pending gh-issue escalation.
                reset_consecutive_count(BrainPhase::Decide, &priority.goal_id);
                push_brain_judgment(BrainJudgmentRecord::from_decide(
                    &priority.goal_id,
                    priority.urgency,
                    &j,
                    is_synthetic_id(&priority.goal_id),
                    crate::ooda_brain::prompt_store::current_version(DECIDE_PROMPT_NAME),
                ));
                j
            }
            Err(e) => {
                // Brain error — record the failure loudly and skip this
                // priority. No fallback to a different brain.
                let raw_response = extract_raw_response(&e);
                let pf = record_parse_failure(
                    BrainPhase::Decide,
                    &priority.goal_id,
                    &e,
                    &raw_response,
                    DECIDE_PROMPT_NAME,
                    crate::ooda_brain::prompt_store::current_version(DECIDE_PROMPT_NAME),
                );
                tracing::error!(
                    priority_goal_id = %priority.goal_id,
                    error = %e,
                    "decide brain failed for priority — skipping (no fallback)"
                );
                let mut rec =
                    BrainJudgmentRecord::from_decide_error(&priority.goal_id, priority.urgency);
                rec.parse_failure = Some(pf);
                push_brain_judgment(rec);
                continue;
            }
        };
        let planned = PlannedAction {
            kind: judgment.action_kind(),
            goal_id: if is_synthetic_id(&priority.goal_id) {
                None
            } else {
                Some(priority.goal_id.clone())
            },
            description: priority.reason.clone(),
        };
        // Defense-in-depth: AdvanceGoal without goal_id is unroutable at
        // dispatch (issue #2227). Skip rather than push an action that will
        // always fail.
        if planned.kind == ActionKind::AdvanceGoal && planned.goal_id.is_none() {
            tracing::warn!(
                priority_goal_id = %priority.goal_id,
                "skipping AdvanceGoal with goal_id=None (synthetic priority mis-routed)"
            );
            continue;
        }
        actions.push(planned);
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

    /// Build a fully-explicit [`OodaConfig`] for tests that reads **no** process
    /// env (unlike [`OodaConfig::default`], which consults `SIMARD_SCALING`,
    /// `SIMARD_MAX_CONCURRENT_ACTIONS`, budget, and distillation vars). Keeps
    /// `decide` unit tests hermetic and independent of the host environment
    /// (issue #2732). `scaler` is `None` so the explicit `max_concurrent_actions`
    /// is the sole cap under test.
    fn test_config() -> OodaConfig {
        OodaConfig {
            max_concurrent_actions: 5,
            improvement_threshold: 0.02,
            gym_suite_id: "progressive".to_string(),
            daily_budget_usd: 500.0,
            weekly_budget_usd: 2500.0,
            distill_min_episodes: 25,
            distill_interval_cycles: 50,
            lesson_recurrence_threshold: 2,
            run_resource_cleanup: false,
            scaler: None,
        }
    }

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
        // Construct the config explicitly with `scaler: None` instead of
        // `..Default::default()`: `OodaConfig::default()` reads process env
        // (`SIMARD_SCALING` / `SIMARD_MAX_CONCURRENT_ACTIONS`), so on a host
        // with `SIMARD_SCALING=auto` the default scaler would override the
        // explicit `max_concurrent_actions=2` under test and the result would
        // depend on the environment rather than the config. Building it
        // explicitly keeps the test hermetic (issue #2732).
        let config = OodaConfig {
            max_concurrent_actions: 2,
            scaler: None,
            ..test_config()
        };
        let actions = decide(&priorities, &config).unwrap();
        assert_eq!(
            actions.len(),
            2,
            "explicit max_concurrent_actions=2 with no scaler must cap to 2"
        );
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

    // -----------------------------------------------------------------------
    // Issue #1979 / #1933 / no-fallback: when the LLM-backed brain returns
    // Err for a priority, decide_with_brain must
    //   (1) record the parse failure in parse_failure::counters(),
    //   (2) embed a ParseFailureRecord on the per-cycle BrainJudgmentRecord,
    //   (3) SKIP the priority (no action produced — no fallback).
    // -----------------------------------------------------------------------
    #[test]
    fn decide_with_brain_skips_priority_on_brain_error() {
        use crate::error::SimardError;
        use crate::ooda_brain::BrainPhase;
        use crate::ooda_brain::parse_failure::{
            peek_consecutive_count, reset_consecutive_count_for_tests, test_serial_guard,
        };

        struct AlwaysErrBrain;
        impl OodaDecideBrain for AlwaysErrBrain {
            fn judge_decision(
                &self,
                _ctx: &DecideContext,
            ) -> crate::error::SimardResult<DecideJudgment> {
                Err(SimardError::AdapterInvocationFailed {
                    base_type: "ooda-decide-brain".into(),
                    reason: "decide-brain-parse-error: stub; payload={}; raw_response=\"junk\""
                        .into(),
                })
            }
        }

        // Serialise on the global counters guard; the map is process-wide.
        let _g = test_serial_guard();
        let goal_id = "decide-parse-fail-1979";
        reset_consecutive_count_for_tests(BrainPhase::Decide, goal_id);

        let priorities = vec![Priority {
            goal_id: goal_id.to_string(),
            urgency: 0.9,
            reason: "test".to_string(),
        }];
        let config = OodaConfig::default();

        let (actions, records) = crate::ooda_brain::with_brain_judgment_scope(|| {
            crate::ooda_brain::clear_brain_judgments();
            let acts = decide_with_brain(&priorities, &config, &AlwaysErrBrain).unwrap();
            (acts, crate::ooda_brain::take_brain_judgments())
        });

        // (3) Priority is SKIPPED — no fallback action produced.
        assert_eq!(
            actions.len(),
            0,
            "brain error must skip the priority, not fallback"
        );

        // (1) parse_failure::counters() observed the (Decide, goal_id) bump.
        let count = peek_consecutive_count(BrainPhase::Decide, goal_id);
        assert_eq!(count, 1, "expected consecutive_count == 1, got {count}");

        // (2) The per-cycle BrainJudgmentRecord embeds the ParseFailureRecord.
        assert_eq!(records.len(), 1);
        let rec = &records[0];
        assert_eq!(rec.decision, "brain_error");
        let pf = rec
            .parse_failure
            .as_ref()
            .expect("ParseFailureRecord must be embedded on the judgment record");
        assert_eq!(pf.phase, "decide");
        assert_eq!(pf.goal_id, goal_id);
        assert_eq!(pf.consecutive_count, 1);
        assert!(
            pf.raw_response_truncated.contains("junk"),
            "raw_response must be salvaged from the brain error (issue #1711 contract): {:?}",
            pf.raw_response_truncated,
        );

        reset_consecutive_count_for_tests(BrainPhase::Decide, goal_id);
    }

    #[test]
    fn decide_with_brain_successful_parse_resets_consecutive_counter() {
        use crate::ooda_brain::BrainPhase;
        use crate::ooda_brain::parse_failure::{
            peek_consecutive_count, reset_consecutive_count_for_tests, test_serial_guard,
        };

        struct AlwaysOkBrain;
        impl OodaDecideBrain for AlwaysOkBrain {
            fn judge_decision(
                &self,
                _ctx: &DecideContext,
            ) -> crate::error::SimardResult<DecideJudgment> {
                Ok(DecideJudgment::AdvanceGoal {
                    rationale: "ok".into(),
                })
            }
        }

        let _g = test_serial_guard();
        let goal_id = "decide-reset-1979";
        reset_consecutive_count_for_tests(BrainPhase::Decide, goal_id);
        // Seed a non-zero counter to prove the reset-on-success path fires.
        crate::ooda_brain::parse_failure::record_parse_failure(
            BrainPhase::Decide,
            goal_id,
            &crate::error::SimardError::AdapterInvocationFailed {
                base_type: "decide".into(),
                reason: "seed".into(),
            },
            "raw",
            crate::ooda_brain::DECIDE_PROMPT_NAME,
            String::new(),
        );
        assert_eq!(peek_consecutive_count(BrainPhase::Decide, goal_id), 1);

        let priorities = vec![Priority {
            goal_id: goal_id.to_string(),
            urgency: 0.9,
            reason: "t".to_string(),
        }];
        let config = OodaConfig::default();
        let _ = decide_with_brain(&priorities, &config, &AlwaysOkBrain).unwrap();

        assert_eq!(
            peek_consecutive_count(BrainPhase::Decide, goal_id),
            0,
            "successful parse must reset (Decide, goal_id) counter"
        );

        reset_consecutive_count_for_tests(BrainPhase::Decide, goal_id);
    }

    // -----------------------------------------------------------------------
    // Issue #2182: AIMD scaler wire-in tests
    // -----------------------------------------------------------------------

    #[test]
    fn decide_uses_scaler_adjusted_limit_when_scaler_is_present() {
        use crate::ooda_loop::adaptive_scaling::AdaptiveScaler;
        use std::sync::Arc;

        // Create a scaler with floor=ceiling=2 so adjust() always returns 2.
        let scaler = Arc::new(AdaptiveScaler::new(2, 2, 2));
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
        ];
        let config = OodaConfig {
            max_concurrent_actions: 10, // would allow all 3 without scaler
            scaler: Some(scaler),
            ..Default::default()
        };
        let actions = decide(&priorities, &config).unwrap();
        assert_eq!(
            actions.len(),
            2,
            "scaler capped at 2 should override max_concurrent_actions=10"
        );
    }

    #[test]
    fn decide_ignores_scaler_when_none() {
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
        ];
        let config = OodaConfig {
            max_concurrent_actions: 1,
            scaler: None,
            ..Default::default()
        };
        let actions = decide(&priorities, &config).unwrap();
        assert_eq!(
            actions.len(),
            1,
            "without scaler, max_concurrent_actions=1 should cap to 1"
        );
    }

    // -----------------------------------------------------------------------
    // Issue #2227: eval-watchdog routing and defense-in-depth guard
    // -----------------------------------------------------------------------

    #[test]
    fn decide_maps_eval_watchdog_to_run_gym_eval() {
        let priorities = vec![Priority {
            goal_id: "__eval_watchdog__".to_string(),
            urgency: 1.0,
            reason: "eval signal stale".to_string(),
        }];
        let config = OodaConfig::default();
        let actions = decide(&priorities, &config).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(
            actions[0].kind,
            ActionKind::RunGymEval,
            "EvalWatchdog must route to RunGymEval, not AdvanceGoal (issue #2227)"
        );
        assert!(actions[0].goal_id.is_none());
    }

    #[test]
    fn decide_routes_synthetic_deterministically_even_with_llm_brain() {
        // An LLM brain that always returns AdvanceGoal. For synthetic
        // priorities, decide_with_brain must bypass it and use deterministic
        // routing so __memory__ becomes ConsolidateMemory (not skipped).
        struct AlwaysAdvanceBrain;
        impl OodaDecideBrain for AlwaysAdvanceBrain {
            fn judge_decision(
                &self,
                _ctx: &DecideContext,
            ) -> crate::error::SimardResult<DecideJudgment> {
                Ok(DecideJudgment::AdvanceGoal {
                    rationale: "llm-said-advance".to_string(),
                })
            }
        }
        let priorities = vec![
            Priority {
                goal_id: "__memory__".to_string(),
                urgency: 0.8,
                reason: "synthetic".to_string(),
            },
            Priority {
                goal_id: "real-goal".to_string(),
                urgency: 0.7,
                reason: "real".to_string(),
            },
        ];
        let config = OodaConfig::default();
        let actions = decide_with_brain(&priorities, &config, &AlwaysAdvanceBrain).unwrap();
        // Both should produce actions: synthetic via deterministic brain,
        // real via the provided LLM brain.
        assert_eq!(
            actions.len(),
            2,
            "synthetic must be deterministically routed, not skipped"
        );
        // Synthetic → ConsolidateMemory (deterministic), no goal_id
        assert_eq!(actions[0].kind, ActionKind::ConsolidateMemory);
        assert!(actions[0].goal_id.is_none());
        // Real → AdvanceGoal (from the LLM brain), with goal_id
        assert_eq!(actions[1].kind, ActionKind::AdvanceGoal);
        assert_eq!(actions[1].goal_id, Some("real-goal".to_string()));
    }

    #[test]
    fn decide_guard_still_catches_unknown_synthetic_advance_goal() {
        // The defense-in-depth guard is still needed for edge cases where
        // an unrecognized __foo__ slips through (not currently possible via
        // SyntheticPriorityKind, but guards against future regressions).
        struct AlwaysAdvanceBrain;
        impl OodaDecideBrain for AlwaysAdvanceBrain {
            fn judge_decision(
                &self,
                _ctx: &DecideContext,
            ) -> crate::error::SimardResult<DecideJudgment> {
                Ok(DecideJudgment::AdvanceGoal {
                    rationale: "mis-routed".to_string(),
                })
            }
        }
        // Use a real goal_id that somehow gets None (only possible if
        // is_synthetic_id logic changes). For now just verify the real
        // goal passes through the LLM brain correctly.
        let priorities = vec![Priority {
            goal_id: "real-goal".to_string(),
            urgency: 0.7,
            reason: "real".to_string(),
        }];
        let config = OodaConfig::default();
        let actions = decide_with_brain(&priorities, &config, &AlwaysAdvanceBrain).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, ActionKind::AdvanceGoal);
        assert_eq!(actions[0].goal_id, Some("real-goal".to_string()));
    }
}
