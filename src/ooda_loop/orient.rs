//! Orient phase: rank goals by urgency, informed by environment context.

use std::collections::{HashMap, HashSet};

use crate::error::SimardResult;
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};
use crate::ooda_brain::{
    BrainJudgmentRecord, DeterministicFallbackOrientBrain, OodaOrientBrain, OrientContext,
    push_brain_judgment,
};

use super::{Observation, Priority};

/// Orient: rank goals by urgency, informed by environment context.
///
/// The per-failure penalty constant lives on the deterministic-fallback
/// brain ([`crate::ooda_brain::FAILURE_PENALTY_PER_CONSECUTIVE`]) so prompt
/// + code stay in sync.
///
/// Default entrypoint: wires in [`DeterministicFallbackOrientBrain`] for
/// the failure-penalty demotion judgment so the daemon never depends on
/// LLM availability for Orient. Callers with an LLM-backed brain can use
/// [`orient_with_brain`].
pub fn orient(
    observation: &Observation,
    goals: &GoalBoard,
    failure_counts: &HashMap<String, u32>,
) -> SimardResult<Vec<Priority>> {
    let brain = DeterministicFallbackOrientBrain;
    orient_with_brain(observation, goals, failure_counts, &brain)
}

/// Orient using a caller-supplied brain for the failure-penalty demotion
/// judgment. On any brain error or invalid judgment for an individual goal,
/// falls back to the deterministic floor for that goal so a transient
/// adapter failure cannot stall the cycle or invert priorities.
///
/// Base urgency: Blocked > not-started > in-progress > completed.
/// Environment signals (dirty working tree, open issues mentioning a goal)
/// can boost a goal's urgency so the OODA loop prioritises actionable work.
/// Goals with consecutive failures are demoted by the brain (or by the
/// deterministic floor `FAILURE_PENALTY_PER_CONSECUTIVE * count`, clamped to
/// ≥0) so the daemon stops burning budget retrying the same broken target.
pub fn orient_with_brain(
    observation: &Observation,
    goals: &GoalBoard,
    failure_counts: &HashMap<String, u32>,
    brain: &dyn OodaOrientBrain,
) -> SimardResult<Vec<Priority>> {
    let env = &observation.environment;
    let has_dirty_tree = !env.git_status.is_empty();

    let mut priorities: Vec<Priority> = goals
        .active
        .iter()
        .map(|g| {
            let (mut urgency, mut reason) = match &g.status {
                GoalProgress::Blocked(r) => (1.0, format!("blocked: {r}")),
                GoalProgress::NotStarted => (0.8, "not yet started".to_string()),
                GoalProgress::InProgress { percent } => (
                    0.6 * (1.0 - (*percent as f64 / 100.0)),
                    format!("{percent}% complete"),
                ),
                GoalProgress::Completed => (0.0, "completed".to_string()),
            };

            // Boost urgency if an open issue mentions this goal.
            let mentioned_in_issues = env
                .open_issues
                .iter()
                .any(|title| title.to_lowercase().contains(&g.id.to_lowercase()));
            if mentioned_in_issues {
                urgency = (urgency + 0.1).min(1.0);
                reason = format!("{reason}; mentioned in open issue");
            }

            // Slight boost for in-progress goals when the tree is dirty
            // (indicates active development that may relate to this goal).
            if has_dirty_tree && matches!(g.status, GoalProgress::InProgress { .. }) {
                urgency = (urgency + 0.05).min(1.0);
                reason = format!("{reason}; dirty working tree");
            }

            // Demote chronically failing goals — prompt-driven judgment via
            // the OodaOrientBrain (PR #1469's third-of-three OODA brain).
            // Per-call fallback to the deterministic floor on any brain
            // error or invalid judgment so the cycle never stalls and the
            // brain can never escalate a failing goal above its base.
            if let Some(&count) = failure_counts.get(&g.id)
                && count > 0
            {
                let ctx = OrientContext {
                    goal_id: g.id.clone(),
                    base_urgency: urgency,
                    base_reason: reason.clone(),
                    failure_count: count,
                };
                let (judgment, fallback_used) = match brain.judge_orientation(&ctx) {
                    Ok(j) if j.validate(ctx.base_urgency).is_ok() => (j, false),
                    _ => (DeterministicFallbackOrientBrain::compute(&ctx), true),
                };
                push_brain_judgment(BrainJudgmentRecord::from_orient(
                    &g.id,
                    ctx.base_urgency,
                    count,
                    &judgment,
                    fallback_used,
                    if fallback_used {
                        String::new()
                    } else {
                        crate::ooda_brain::prompt_store::current_version(
                            crate::ooda_brain::ORIENT_PROMPT_NAME,
                        )
                    },
                ));
                reason = format!("{reason}; {}", judgment.rationale);
                urgency = judgment.adjusted_urgency;
            }

            Priority {
                goal_id: g.id.clone(),
                urgency,
                reason,
            }
        })
        .collect();

    filter_hallucinated_priorities(&mut priorities, &goals.active);

    if observation.memory_stats.episodic_count > 100 {
        priorities.push(Priority {
            goal_id: "__memory__".to_string(),
            urgency: 0.5,
            reason: format!(
                "episodic memory has {} entries, consolidation needed",
                observation.memory_stats.episodic_count
            ),
        });
    }

    if let Some(ref score) = observation.gym_health
        && score.overall < 0.7
    {
        priorities.push(Priority {
            goal_id: "__improvement__".to_string(),
            urgency: 0.7,
            reason: format!("gym overall {:.1}% below 70% target", score.overall * 100.0),
        });
    }

    // ── Eval watchdog override ──────────────────────────────────────
    // If the watchdog tripped in observe(), nothing else matters this
    // cycle. Push a synthetic priority with urgency 1.0 (above any
    // real goal) so decide() routes to it. This is the loop's "stop
    // and ring the alarm" path — kept alongside other priorities so
    // the existing ranking/filing/dashboard infrastructure picks it up
    // for free, but with enough urgency that it preempts ordinary work.
    if let Some(ref reason) = observation.eval_watchdog {
        priorities.push(Priority {
            goal_id: "__eval_watchdog__".to_string(),
            urgency: 1.0,
            reason: format!("EVAL WATCHDOG: {reason}"),
        });
    }

    priorities.sort_by(|a, b| {
        b.urgency
            .partial_cmp(&a.urgency)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(priorities)
}

/// Drop any priorities whose `goal_id` is neither in `active_goals` nor a
/// synthetic (`__`-prefix). Called after building the priorities vec from
/// `goals.active` and before appending synthetic priorities, so synthetics
/// are never filtered.
pub(crate) fn filter_hallucinated_priorities(
    priorities: &mut Vec<Priority>,
    active_goals: &[ActiveGoal],
) {
    let active_ids: HashSet<&str> = active_goals.iter().map(|g| g.id.as_str()).collect();
    priorities.retain(|p| {
        if p.goal_id.starts_with("__") || active_ids.contains(p.goal_id.as_str()) {
            true
        } else {
            eprintln!(
                "[simard] OODA orient: dropping hallucinated goal_id '{}' — not on active board",
                p.goal_id
            );
            false
        }
    });
}

#[cfg(test)]
mod wire_in_tests {
    use super::*;
    use crate::goal_curation::{ActiveGoal, GoalProgress};
    use crate::memory_cognitive::CognitiveStatistics;
    use crate::ooda_brain::OrientJudgment;
    use crate::ooda_loop::EnvironmentSnapshot;

    /// Wiring test (companion to PR completing #1469 + #1471 wire-up):
    /// when an LLM-backed orient brain is provided, the per-cycle
    /// `BrainJudgmentRecord.rationale` for the orient phase must be the
    /// brain's own rationale — NOT the deterministic fallback marker. This
    /// mirrors the analogous test for decide-brain wiring.
    #[test]
    fn orient_with_brain_records_brain_rationale_not_fallback_marker() {
        struct LlmStubOrientBrain;
        impl OodaOrientBrain for LlmStubOrientBrain {
            fn judge_orientation(&self, ctx: &OrientContext) -> SimardResult<OrientJudgment> {
                Ok(OrientJudgment {
                    adjusted_urgency: (ctx.base_urgency - 0.05).max(0.0),
                    rationale: "llm-orient-brain: light demotion".to_string(),
                    confidence: 0.9,
                    demotion_applied: 0.05,
                })
            }
        }

        let mut board = GoalBoard::default();
        board.active.push(ActiveGoal {
            id: "ship-v1".to_string(),
            description: "ship v1".to_string(),
            priority: 1,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
        });
        let obs = Observation {
            goal_statuses: Vec::new(),
            gym_health: None,
            memory_stats: CognitiveStatistics::default(),
            pending_improvements: Vec::new(),
            environment: EnvironmentSnapshot::default(),
            eval_watchdog: None,
        };
        let mut failures = HashMap::new();
        failures.insert("ship-v1".to_string(), 1);

        let records = crate::ooda_brain::with_brain_judgment_scope(|| {
            crate::ooda_brain::clear_brain_judgments();
            orient_with_brain(&obs, &board, &failures, &LlmStubOrientBrain).unwrap();
            crate::ooda_brain::take_brain_judgments()
        });
        assert_eq!(records.len(), 1);
        assert!(
            !records[0].rationale.contains("fallback-brain"),
            "expected LLM-orient-brain rationale, got fallback marker: {}",
            records[0].rationale,
        );
        assert_eq!(records[0].rationale, "llm-orient-brain: light demotion");
        assert!(!records[0].fallback);
    }
}

#[cfg(test)]
mod hallucination_filter_tests {
    use super::*;
    use crate::goal_curation::{ActiveGoal, GoalProgress};
    use crate::ooda_loop::Priority;

    fn active(id: &str) -> ActiveGoal {
        ActiveGoal {
            id: id.to_string(),
            description: format!("desc {id}"),
            priority: 1,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
        }
    }

    fn priority(id: &str) -> Priority {
        Priority {
            goal_id: id.to_string(),
            urgency: 0.5,
            reason: "test".to_string(),
        }
    }

    #[test]
    fn retains_priorities_present_in_active_goals() {
        let goals = vec![active("ship-v1"), active("fix-db")];
        let mut priorities = vec![priority("ship-v1"), priority("fix-db")];
        filter_hallucinated_priorities(&mut priorities, &goals);
        assert_eq!(priorities.len(), 2);
    }

    #[test]
    fn drops_priorities_absent_from_active_goals() {
        let goals = vec![active("ship-v1")];
        let mut priorities = vec![priority("ship-v1"), priority("g1")];
        filter_hallucinated_priorities(&mut priorities, &goals);
        assert_eq!(priorities.len(), 1);
        assert_eq!(priorities[0].goal_id, "ship-v1");
    }

    #[test]
    fn retains_synthetic_double_underscore_priorities() {
        let goals: Vec<ActiveGoal> = vec![];
        let mut priorities = vec![
            priority("__memory__"),
            priority("__improvement__"),
            priority("__eval_watchdog__"),
        ];
        filter_hallucinated_priorities(&mut priorities, &goals);
        assert_eq!(priorities.len(), 3);
    }

    #[test]
    fn empty_active_goals_drops_non_synthetic() {
        let goals: Vec<ActiveGoal> = vec![];
        let mut priorities = vec![priority("orphan-goal"), priority("__memory__")];
        filter_hallucinated_priorities(&mut priorities, &goals);
        assert_eq!(priorities.len(), 1);
        assert_eq!(priorities[0].goal_id, "__memory__");
    }
}
