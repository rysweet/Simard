//! Per-cycle accumulator of prompt-driven brain judgments.
//!
//! Surfaces what each prompt-driven OODA phase decided for the user to
//! inspect via `~/.simard/cycle_reports/cycle_*.json`. The Act, Decide and
//! Orient phases each call `brain.judge_*()` (or fall back to the
//! deterministic floor). After every such call site, the wire-in pushes a
//! [`BrainJudgmentRecord`] onto the per-cycle accumulator. The cycle-report
//! writer drains the accumulator and persists the records under the new
//! `brain_judgments` field.
//!
//! Threading: the accumulator is a `thread_local!` `RefCell<Vec<...>>`. The
//! OODA cycle runs single-threaded per daemon (`run_ooda_cycle`), so a
//! thread-local is the least invasive plumbing — it avoids threading a
//! per-cycle context through every brain call site (each lives at a
//! different layer of the stack: `ooda_loop::orient`, `ooda_loop::decide`,
//! `ooda_actions::advance_goal::spawn`).
//!
//! Lifecycle is deterministic:
//! 1. `clear()` at the start of each `run_ooda_cycle`.
//! 2. `push()` after each `brain.judge_*()` call (or fallback).
//! 3. `take_all()` when assembling the final `CycleReport`.

use std::cell::RefCell;

use super::{DecideJudgment, EngineerLifecycleDecision, OrientJudgment};

/// Which OODA phase produced the judgment. Serialised as lowercase strings
/// so the cycle-report JSON consumers (dashboard, ad-hoc inspection) read
/// `"act"`, `"decide"`, `"orient"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrainPhase {
    Act,
    Decide,
    Orient,
}

/// One record of a single `brain.judge_*()` call (or its fallback). Fields
/// are intentionally simple strings/floats so the cycle-report consumer can
/// render them verbatim without re-deserialising the per-phase judgment
/// types.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BrainJudgmentRecord {
    pub phase: BrainPhase,
    /// Short summary of the inputs to the brain (truncated to ~200 chars).
    pub context_summary: String,
    /// Short label describing the variant chosen (e.g.
    /// `"reclaim_and_redispatch"`, `"advance_goal"`, `"demote"`).
    pub decision: String,
    pub rationale: String,
    /// In `[0.0, 1.0]`. Phases without a native confidence field report 1.0
    /// for deterministic outputs and a fixed lower value for fallback paths.
    pub confidence: f32,
    /// `true` when the deterministic fallback fired because the LLM brain
    /// errored or returned an invalid judgment.
    pub fallback: bool,
}

const CONTEXT_SUMMARY_MAX: usize = 200;

fn truncate(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.len() <= CONTEXT_SUMMARY_MAX {
        trimmed.to_string()
    } else {
        format!("{}…", &trimmed[..CONTEXT_SUMMARY_MAX])
    }
}

impl BrainJudgmentRecord {
    /// Build an Act-phase record from the engineer-lifecycle decision. The
    /// `EngineerLifecycleDecision` enum carries the rationale per variant.
    pub fn from_engineer_lifecycle(
        goal_id: &str,
        decision: &EngineerLifecycleDecision,
        fallback: bool,
    ) -> Self {
        let (label, rationale) = match decision {
            EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                ("continue_skipping", rationale.as_str())
            }
            EngineerLifecycleDecision::ReclaimAndRedispatch { rationale, .. } => {
                ("reclaim_and_redispatch", rationale.as_str())
            }
            EngineerLifecycleDecision::Deprioritize { rationale } => {
                ("deprioritize", rationale.as_str())
            }
            EngineerLifecycleDecision::OpenTrackingIssue { rationale, .. } => {
                ("open_tracking_issue", rationale.as_str())
            }
            EngineerLifecycleDecision::MarkGoalBlocked { rationale, .. } => {
                ("mark_goal_blocked", rationale.as_str())
            }
        };
        Self {
            phase: BrainPhase::Act,
            context_summary: truncate(&format!("engineer-lifecycle goal_id={goal_id}")),
            decision: label.to_string(),
            rationale: rationale.to_string(),
            confidence: if fallback { 0.5 } else { 1.0 },
            fallback,
        }
    }

    /// Build a Decide-phase record from the action-kind judgment + the
    /// priority that drove it.
    pub fn from_decide(
        goal_id: &str,
        urgency: f64,
        judgment: &DecideJudgment,
        fallback: bool,
    ) -> Self {
        let label = match judgment {
            DecideJudgment::AdvanceGoal { .. } => "advance_goal",
            DecideJudgment::RunImprovement { .. } => "run_improvement",
            DecideJudgment::ConsolidateMemory { .. } => "consolidate_memory",
            DecideJudgment::ResearchQuery { .. } => "research_query",
            DecideJudgment::RunGymEval { .. } => "run_gym_eval",
            DecideJudgment::BuildSkill { .. } => "build_skill",
            DecideJudgment::LaunchSession { .. } => "launch_session",
            DecideJudgment::PollDeveloperActivity { .. } => "poll_developer_activity",
            DecideJudgment::ExtractIdeas { .. } => "extract_ideas",
        };
        Self {
            phase: BrainPhase::Decide,
            context_summary: truncate(&format!("goal_id={goal_id} urgency={urgency:.3}")),
            decision: label.to_string(),
            rationale: judgment.rationale().to_string(),
            confidence: if fallback { 0.5 } else { 1.0 },
            fallback,
        }
    }

    /// Build an Orient-phase record from the demotion judgment + originating
    /// goal context.
    pub fn from_orient(
        goal_id: &str,
        base_urgency: f64,
        failure_count: u32,
        judgment: &OrientJudgment,
        fallback: bool,
    ) -> Self {
        let label = if judgment.demotion_applied > 0.0 {
            "demote"
        } else {
            "no_demotion"
        };
        Self {
            phase: BrainPhase::Orient,
            context_summary: truncate(&format!(
                "goal_id={goal_id} base_urgency={base_urgency:.3} failures={failure_count}"
            )),
            decision: label.to_string(),
            rationale: judgment.rationale.clone(),
            confidence: judgment.confidence as f32,
            fallback,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-cycle thread-local accumulator
// ---------------------------------------------------------------------------

thread_local! {
    static BRAIN_JUDGMENTS: RefCell<Vec<BrainJudgmentRecord>> = const { RefCell::new(Vec::new()) };
}

/// Append one judgment to the current cycle's accumulator.
pub fn push(record: BrainJudgmentRecord) {
    BRAIN_JUDGMENTS.with(|cell| cell.borrow_mut().push(record));
}

/// Drain and return all records accumulated so far. Called at the end of
/// `run_ooda_cycle` to attach the records to the [`CycleReport`].
pub fn take_all() -> Vec<BrainJudgmentRecord> {
    BRAIN_JUDGMENTS.with(|cell| std::mem::take(&mut *cell.borrow_mut()))
}

/// Reset the accumulator (drops any leftover records). Called at the start
/// of `run_ooda_cycle` so a previous cycle (or test) cannot leak entries
/// into the next one.
pub fn clear() {
    BRAIN_JUDGMENTS.with(|cell| cell.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ooda_brain::{DecideJudgment, EngineerLifecycleDecision, OrientJudgment};

    fn sample_orient_judgment() -> OrientJudgment {
        OrientJudgment {
            adjusted_urgency: 0.4,
            rationale: "demoted".to_string(),
            confidence: 0.9,
            demotion_applied: 0.2,
        }
    }

    #[test]
    fn record_round_trips_through_json() {
        let rec = BrainJudgmentRecord {
            phase: BrainPhase::Decide,
            context_summary: "ctx".to_string(),
            decision: "advance_goal".to_string(),
            rationale: "because".to_string(),
            confidence: 0.75,
            fallback: false,
        };
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("\"phase\":\"decide\""));
        let back: BrainJudgmentRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(rec, back);
    }

    #[test]
    fn from_engineer_lifecycle_picks_variant_label() {
        let dec = EngineerLifecycleDecision::ReclaimAndRedispatch {
            rationale: "stuck".to_string(),
            redispatch_context: "ctx".to_string(),
        };
        let rec = BrainJudgmentRecord::from_engineer_lifecycle("g1", &dec, false);
        assert_eq!(rec.phase, BrainPhase::Act);
        assert_eq!(rec.decision, "reclaim_and_redispatch");
        assert_eq!(rec.rationale, "stuck");
        assert!(!rec.fallback);
    }

    #[test]
    fn from_decide_uses_judgment_rationale() {
        let j = DecideJudgment::RunGymEval {
            rationale: "gym slipped".to_string(),
        };
        let rec = BrainJudgmentRecord::from_decide("__improvement__", 0.7, &j, true);
        assert_eq!(rec.phase, BrainPhase::Decide);
        assert_eq!(rec.decision, "run_gym_eval");
        assert!(rec.fallback);
        assert!((rec.confidence - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn from_orient_labels_demotion_vs_no_op() {
        let demoted = sample_orient_judgment();
        let rec = BrainJudgmentRecord::from_orient("g1", 0.6, 1, &demoted, false);
        assert_eq!(rec.decision, "demote");

        let untouched = OrientJudgment {
            adjusted_urgency: 0.6,
            rationale: "no change".to_string(),
            confidence: 1.0,
            demotion_applied: 0.0,
        };
        let rec = BrainJudgmentRecord::from_orient("g1", 0.6, 0, &untouched, false);
        assert_eq!(rec.decision, "no_demotion");
    }

    #[test]
    fn truncate_long_context_summary() {
        let long = "x".repeat(500);
        let dec = EngineerLifecycleDecision::ContinueSkipping {
            rationale: long.clone(),
        };
        let rec = BrainJudgmentRecord::from_engineer_lifecycle(&long, &dec, false);
        assert!(rec.context_summary.len() <= CONTEXT_SUMMARY_MAX + 4);
    }

    #[test]
    fn accumulator_push_take_clear_isolation() {
        // Use a fresh thread so the thread-local starts empty regardless of
        // any leftovers from other tests in the same module.
        let handle = std::thread::spawn(|| {
            clear();
            assert!(take_all().is_empty());

            push(BrainJudgmentRecord {
                phase: BrainPhase::Act,
                context_summary: "a".to_string(),
                decision: "x".to_string(),
                rationale: "r".to_string(),
                confidence: 1.0,
                fallback: false,
            });
            push(BrainJudgmentRecord {
                phase: BrainPhase::Decide,
                context_summary: "b".to_string(),
                decision: "y".to_string(),
                rationale: "r".to_string(),
                confidence: 1.0,
                fallback: false,
            });

            let drained = take_all();
            assert_eq!(drained.len(), 2);
            // Second drain is empty — take_all() consumes.
            assert!(take_all().is_empty());

            // clear() also wipes after pushes.
            push(BrainJudgmentRecord {
                phase: BrainPhase::Orient,
                context_summary: "c".to_string(),
                decision: "z".to_string(),
                rationale: "r".to_string(),
                confidence: 1.0,
                fallback: false,
            });
            clear();
            assert!(take_all().is_empty());
        });
        handle.join().unwrap();
    }

    #[test]
    fn accumulator_isolates_across_threads() {
        // Each thread has its own thread-local — pushes in one thread must
        // not appear in another.
        let t1 = std::thread::spawn(|| {
            clear();
            push(BrainJudgmentRecord {
                phase: BrainPhase::Act,
                context_summary: "t1".to_string(),
                decision: "x".to_string(),
                rationale: "r".to_string(),
                confidence: 1.0,
                fallback: false,
            });
            take_all()
        });
        let t2 = std::thread::spawn(|| {
            clear();
            take_all()
        });
        let v1 = t1.join().unwrap();
        let v2 = t2.join().unwrap();
        assert_eq!(v1.len(), 1);
        assert!(v2.is_empty());
    }
}
