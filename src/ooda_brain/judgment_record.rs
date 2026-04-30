//! Per-cycle accumulator of prompt-driven brain judgments.
//!
//! Surfaces what each prompt-driven OODA phase decided for the user to
//! inspect via `~/.simard/cycle_reports/cycle_*.json`. The Act, Decide and
//! Orient phases each call `brain.judge_*()` (or fall back to the
//! deterministic floor); each call site pushes a [`BrainJudgmentRecord`]
//! onto the per-cycle accumulator, which the cycle-report writer drains.
//!
//! Threading: the accumulator is a [`tokio::task_local!`], installed by
//! [`with_cycle_scope`] (a thin wrapper around `LocalKey::sync_scope`) at
//! the top of `run_ooda_cycle`. A previous implementation used
//! `thread_local!`, but brain LLM calls drive Tokio worker threads via the
//! session adapter, so pushes could land on a different OS thread than the
//! eventual `take_all()` — producing empty `brain_judgments` arrays in
//! cycle reports (PR #1472, daemon `d69c411c52f1` cycle_2 had
//! `planned_actions: 3` but `brain_judgments: []`).
//!
//! Outside any scope (e.g., unit tests that don't establish one), `push()`
//! silently no-ops and `take_all()` returns an empty vec.

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
    /// 12-char sha256 prefix of the prompt-asset content that produced this
    /// judgment (see [`crate::ooda_brain::prompt_store::prompt_version`]).
    /// Empty for fallback / deterministic phases that don't read a prompt
    /// file — which observers can read as "no prompt was involved".
    /// Default-skipped on serialise so older cycle reports stay readable.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt_version: String,
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
        prompt_version: impl Into<String>,
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
            prompt_version: prompt_version.into(),
        }
    }

    /// Build a Decide-phase record from the action-kind judgment + the
    /// priority that drove it.
    pub fn from_decide(
        goal_id: &str,
        urgency: f64,
        judgment: &DecideJudgment,
        fallback: bool,
        prompt_version: impl Into<String>,
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
            prompt_version: prompt_version.into(),
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
        prompt_version: impl Into<String>,
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
            prompt_version: prompt_version.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-cycle task-local accumulator
// ---------------------------------------------------------------------------

tokio::task_local! {
    /// Per-cycle accumulator. Installed by [`with_cycle_scope`].
    static BRAIN_JUDGMENTS: RefCell<Vec<BrainJudgmentRecord>>;
}

/// Run a closure (one OODA cycle's body) inside a fresh
/// [`BRAIN_JUDGMENTS`] task-local scope. Every `push()` invoked
/// transitively by `f` — including from Tokio worker threads driven by
/// brain LLM calls — observes the same accumulator. Called exactly once
/// per cycle from `ooda_loop::cycle::run_ooda_cycle`.
pub fn with_cycle_scope<R>(f: impl FnOnce() -> R) -> R {
    BRAIN_JUDGMENTS.sync_scope(RefCell::new(Vec::new()), f)
}

/// Append one judgment to the current cycle's accumulator. Silently no-ops
/// outside a [`with_cycle_scope`] (e.g., in tests with no scope set up).
pub fn push(record: BrainJudgmentRecord) {
    let _ = BRAIN_JUDGMENTS.try_with(|cell| cell.borrow_mut().push(record));
}

/// Drain and return all records accumulated so far. Returns `vec![]`
/// outside a scope.
pub fn take_all() -> Vec<BrainJudgmentRecord> {
    BRAIN_JUDGMENTS
        .try_with(|cell| std::mem::take(&mut *cell.borrow_mut()))
        .unwrap_or_default()
}

/// Reset the accumulator (drops any leftover records). Retained for the
/// `clear_brain_judgments()` re-export. No-ops outside a scope.
pub fn clear() {
    let _ = BRAIN_JUDGMENTS.try_with(|cell| cell.borrow_mut().clear());
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
            prompt_version: "deadbeef1234".to_string(),
        };
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("\"phase\":\"decide\""));
        assert!(json.contains("\"prompt_version\":\"deadbeef1234\""));
        let back: BrainJudgmentRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(rec, back);
    }

    #[test]
    fn empty_prompt_version_is_omitted_from_json() {
        // Older cycle reports / deterministic-only phases serialise without
        // the field, so observers can read "no field" === "no prompt".
        let rec = BrainJudgmentRecord {
            phase: BrainPhase::Act,
            context_summary: "ctx".to_string(),
            decision: "continue_skipping".to_string(),
            rationale: "r".to_string(),
            confidence: 0.5,
            fallback: true,
            prompt_version: String::new(),
        };
        let json = serde_json::to_string(&rec).unwrap();
        assert!(
            !json.contains("prompt_version"),
            "expected empty prompt_version to be skipped: {json}"
        );
    }

    #[test]
    fn from_engineer_lifecycle_picks_variant_label() {
        let dec = EngineerLifecycleDecision::ReclaimAndRedispatch {
            rationale: "stuck".to_string(),
            redispatch_context: "ctx".to_string(),
        };
        let rec = BrainJudgmentRecord::from_engineer_lifecycle("g1", &dec, false, "abc123def456");
        assert_eq!(rec.phase, BrainPhase::Act);
        assert_eq!(rec.decision, "reclaim_and_redispatch");
        assert_eq!(rec.rationale, "stuck");
        assert!(!rec.fallback);
        assert_eq!(rec.prompt_version, "abc123def456");
    }

    #[test]
    fn from_decide_uses_judgment_rationale() {
        let j = DecideJudgment::RunGymEval {
            rationale: "gym slipped".to_string(),
        };
        let rec = BrainJudgmentRecord::from_decide("__improvement__", 0.7, &j, true, "");
        assert_eq!(rec.phase, BrainPhase::Decide);
        assert_eq!(rec.decision, "run_gym_eval");
        assert!(rec.fallback);
        assert!((rec.confidence - 0.5).abs() < f32::EPSILON);
        assert!(rec.prompt_version.is_empty());
    }

    #[test]
    fn from_orient_labels_demotion_vs_no_op() {
        let demoted = sample_orient_judgment();
        let rec = BrainJudgmentRecord::from_orient("g1", 0.6, 1, &demoted, false, "v1");
        assert_eq!(rec.decision, "demote");
        assert_eq!(rec.prompt_version, "v1");

        let untouched = OrientJudgment {
            adjusted_urgency: 0.6,
            rationale: "no change".to_string(),
            confidence: 1.0,
            demotion_applied: 0.0,
        };
        let rec = BrainJudgmentRecord::from_orient("g1", 0.6, 0, &untouched, false, "");
        assert_eq!(rec.decision, "no_demotion");
    }

    #[test]
    fn truncate_long_context_summary() {
        let long = "x".repeat(500);
        let dec = EngineerLifecycleDecision::ContinueSkipping {
            rationale: long.clone(),
        };
        let rec = BrainJudgmentRecord::from_engineer_lifecycle(&long, &dec, false, "");
        assert!(rec.context_summary.len() <= CONTEXT_SUMMARY_MAX + 4);
    }

    #[tokio::test]
    async fn accumulator_push_take_clear_isolation() {
        BRAIN_JUDGMENTS
            .scope(RefCell::new(Vec::new()), async {
                assert!(take_all().is_empty());

                push(BrainJudgmentRecord {
                    phase: BrainPhase::Act,
                    context_summary: "a".to_string(),
                    decision: "x".to_string(),
                    rationale: "r".to_string(),
                    confidence: 1.0,
                    fallback: false,
                    prompt_version: String::new(),
                });
                push(BrainJudgmentRecord {
                    phase: BrainPhase::Decide,
                    context_summary: "b".to_string(),
                    decision: "y".to_string(),
                    rationale: "r".to_string(),
                    confidence: 1.0,
                    fallback: false,
                    prompt_version: String::new(),
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
                    prompt_version: String::new(),
                });
                clear();
                assert!(take_all().is_empty());
            })
            .await;
    }

    #[tokio::test]
    async fn push_outside_scope_is_silent_noop() {
        push(BrainJudgmentRecord {
            phase: BrainPhase::Act,
            context_summary: "orphan".to_string(),
            decision: "x".to_string(),
            rationale: "r".to_string(),
            confidence: 1.0,
            fallback: false,
            prompt_version: String::new(),
        });
        assert!(take_all().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn accumulator_isolates_across_concurrent_scopes() {
        // Each spawned task installs its own scope. Pushes in one task must
        // not appear in another, even when both run on the same worker pool.
        let t1 = tokio::spawn(with_cycle_scope_async(async {
            push(BrainJudgmentRecord {
                phase: BrainPhase::Act,
                context_summary: "t1".to_string(),
                decision: "x".to_string(),
                rationale: "r".to_string(),
                confidence: 1.0,
                fallback: false,
                prompt_version: String::new(),
            });
            take_all()
        }));
        let t2 = tokio::spawn(with_cycle_scope_async(async { take_all() }));
        assert_eq!(t1.await.unwrap().len(), 1);
        assert!(t2.await.unwrap().is_empty());
    }

    #[test]
    fn survives_multi_thread_runtime_with_yields() {
        // Regression for PR #1472: under a multi-thread runtime, pushes that
        // happen after `.await` points (which may migrate the task to a
        // different worker thread) must still be observed by `take_all`.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();

        let collected: Vec<BrainJudgmentRecord> = rt.block_on(with_cycle_scope_async(async {
            for i in 0..3 {
                tokio::task::yield_now().await;
                push(BrainJudgmentRecord {
                    phase: BrainPhase::Decide,
                    context_summary: format!("ctx-{i}"),
                    decision: "advance_goal".to_string(),
                    rationale: "r".to_string(),
                    confidence: 1.0,
                    fallback: false,
                    prompt_version: String::new(),
                });
            }
            tokio::task::yield_now().await;
            take_all()
        }));

        // All 3 pushes observed regardless of which worker executed each.
        assert_eq!(collected.len(), 3);
    }

    /// Test helper: async equivalent of [`with_cycle_scope`].
    fn with_cycle_scope_async<F: std::future::Future>(
        fut: F,
    ) -> impl std::future::Future<Output = F::Output> {
        BRAIN_JUDGMENTS.scope(RefCell::new(Vec::new()), fut)
    }
}
