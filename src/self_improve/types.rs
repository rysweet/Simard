//! Types for the self-improvement loop.

use crate::gym_scoring::{GymSuiteScore, Regression};
use serde::{Deserialize, Serialize};

/// Phases of a single self-improvement cycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ImprovementPhase {
    /// Run the gym suite to establish a baseline score.
    Eval,
    /// Analyze the baseline results for weak dimensions.
    Analyze,
    /// Research possible changes that could address weaknesses.
    Research,
    /// Apply the proposed changes (in a sandbox / canary environment).
    Improve,
    /// Re-run the gym suite against the changed version.
    ReEval,
    /// Compare baseline and post-change scores and decide.
    Decide,
}

impl std::fmt::Display for ImprovementPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Eval => "eval",
            Self::Analyze => "analyze",
            Self::Research => "research",
            Self::Improve => "improve",
            Self::ReEval => "re-eval",
            Self::Decide => "decide",
        };
        f.write_str(label)
    }
}

/// A single proposed change to prompts, policies, or orchestration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProposedChange {
    /// Path to the file that would be changed.
    pub file_path: String,
    /// Human-readable description of the change.
    pub description: String,
    /// Why this change is expected to help.
    pub expected_impact: String,
}

/// A dimension that scored below the weak threshold, with its deficit.
///
/// The deficit indicates how far below the threshold the dimension scored,
/// enabling callers to prioritize improvements by severity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WeakDimension {
    /// Name of the scoring dimension (e.g. "specificity").
    pub name: String,
    /// How far below the threshold this dimension scored (always >= 0).
    pub deficit: f64,
}

/// The outcome of the decision phase.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ImprovementDecision {
    /// The changes should be committed.
    Commit {
        /// Net overall improvement as a fraction (e.g. 0.05 = 5%).
        net_improvement: f64,
    },
    /// The changes should be reverted.
    Revert {
        /// Why the changes were rejected.
        reason: String,
    },
}

/// Configuration for an improvement cycle.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImprovementConfig {
    /// The gym suite to evaluate against.
    pub suite_id: String,
    /// Minimum net improvement required to commit (fraction, e.g. 0.02 = 2%).
    pub min_net_improvement: f64,
    /// Maximum allowed regression on any single dimension (fraction, e.g. 0.05 = 5%).
    pub max_single_regression: f64,
    /// Proposed changes to evaluate.
    pub proposed_changes: Vec<ProposedChange>,
    /// Whether to auto-apply improvements via the plan+review pipeline.
    pub auto_apply: bool,
    /// Dimensions scoring below this threshold are considered "weak" (default 0.6).
    pub weak_threshold: f64,
    /// If set, focus analysis on this single dimension instead of all dimensions.
    pub target_dimension: Option<String>,
    /// Maximum number of improvement cycles to run before stopping.
    /// `None` means no limit (caller controls termination).
    #[serde(default)]
    pub max_cycles: Option<u32>,
}

impl Default for ImprovementConfig {
    fn default() -> Self {
        Self {
            suite_id: "progressive".to_string(),
            min_net_improvement: 0.02,
            max_single_regression: 0.05,
            proposed_changes: Vec::new(),
            auto_apply: false,
            weak_threshold: 0.6,
            target_dimension: None,
            max_cycles: None,
        }
    }
}

impl ImprovementConfig {
    /// Validate that config fields contain sensible values.
    ///
    /// Returns an error for empty suite IDs or thresholds outside [0.0, 1.0].
    pub fn validate(&self) -> crate::error::SimardResult<()> {
        if self.suite_id.is_empty() {
            return Err(crate::error::SimardError::InvalidImprovementRecord {
                field: "suite_id".into(),
                reason: "suite_id must not be empty".into(),
            });
        }
        if !(0.0..=1.0).contains(&self.weak_threshold) {
            return Err(crate::error::SimardError::InvalidImprovementRecord {
                field: "weak_threshold".into(),
                reason: format!(
                    "weak_threshold must be in 0.0..=1.0, got {}",
                    self.weak_threshold
                ),
            });
        }
        if !(0.0..=1.0).contains(&self.min_net_improvement) {
            return Err(crate::error::SimardError::InvalidImprovementRecord {
                field: "min_net_improvement".into(),
                reason: format!(
                    "min_net_improvement must be in 0.0..=1.0, got {}",
                    self.min_net_improvement
                ),
            });
        }
        if !(0.0..=1.0).contains(&self.max_single_regression) {
            return Err(crate::error::SimardError::InvalidImprovementRecord {
                field: "max_single_regression".into(),
                reason: format!(
                    "max_single_regression must be in 0.0..=1.0, got {}",
                    self.max_single_regression
                ),
            });
        }
        if let Some(ref dim) = self.target_dimension
            && !super::prioritization::DIMENSION_NAMES.contains(&dim.as_str())
        {
            return Err(crate::error::SimardError::InvalidImprovementRecord {
                field: "target_dimension".into(),
                reason: format!(
                    "unknown dimension '{}'; valid dimensions: {}",
                    dim,
                    super::prioritization::DIMENSION_NAMES.join(", ")
                ),
            });
        }
        if self.max_cycles == Some(0) {
            return Err(crate::error::SimardError::InvalidImprovementRecord {
                field: "max_cycles".into(),
                reason: "max_cycles must be None or > 0".into(),
            });
        }
        Ok(())
    }
}

/// A complete improvement cycle record with full provenance.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImprovementCycle {
    /// The baseline score established in the Eval phase.
    pub baseline: GymSuiteScore,
    /// Changes that were proposed during the Research/Improve phases.
    pub proposed_changes: Vec<ProposedChange>,
    /// The post-change score from the ReEval phase (None if ReEval was skipped).
    pub post_score: Option<GymSuiteScore>,
    /// Regressions detected during the Decide phase.
    pub regressions: Vec<Regression>,
    /// The final decision (None if the cycle was aborted before Decide).
    pub decision: Option<ImprovementDecision>,
    /// The phase the cycle reached before completing or aborting.
    pub final_phase: ImprovementPhase,
    /// Dimensions that scored below the weak threshold during Analyze.
    pub weak_dimensions: Vec<String>,
    /// Detailed weak dimension info with deficits, sorted by severity (largest deficit first).
    #[serde(default)]
    pub weak_dimension_details: Vec<WeakDimension>,
    /// The dimension that was targeted for this cycle (if any).
    #[serde(default)]
    pub target_dimension: Option<String>,
    /// Dimensions detected as plateaued (weak for 3+ consecutive cycles with near-zero velocity).
    #[serde(default)]
    pub plateau_dimensions: Vec<String>,
}

impl ImprovementCycle {
    /// Returns `true` if the cycle decided to commit.
    pub fn is_committed(&self) -> bool {
        matches!(&self.decision, Some(ImprovementDecision::Commit { .. }))
    }

    /// Returns `true` if the cycle decided to revert.
    pub fn is_reverted(&self) -> bool {
        matches!(&self.decision, Some(ImprovementDecision::Revert { .. }))
    }

    /// Enrich the cycle with plateau detection from historical baselines.
    ///
    /// Call this after `run_improvement_cycle` when you have past cycle data
    /// available. Without history, `plateau_dimensions` remains empty.
    pub fn enrich_with_history(
        &mut self,
        weak_threshold: f64,
        past_baselines: &[crate::gym_scoring::GymSuiteScore],
    ) {
        self.plateau_dimensions = super::prioritization::detect_plateau_dimensions(
            &self.baseline,
            weak_threshold,
            past_baselines,
        );
    }

    /// Enrich the cycle with plateau detection from a [`CycleHistory`].
    ///
    /// Convenience wrapper around [`enrich_with_history`](Self::enrich_with_history)
    /// that extracts baselines from a `CycleHistory` automatically. Callers
    /// typically have a `CycleHistory` rather than a bare `&[GymSuiteScore]`,
    /// so this avoids the manual `.baselines()` call.
    pub fn enrich_from_history(&mut self, weak_threshold: f64, history: &CycleHistory) {
        self.enrich_with_history(weak_threshold, &history.baselines());
    }

    /// Returns the delta for the targeted dimension, if one was set and a
    /// post-score exists.
    ///
    /// Positive values indicate improvement; negative values indicate regression.
    pub fn target_dimension_delta(&self) -> Option<f64> {
        let target = self.target_dimension.as_deref()?;
        let post = self.post_score.as_ref()?;
        let baseline_val = super::prioritization::dimension_value(&self.baseline, target);
        let post_val = super::prioritization::dimension_value(post, target);
        Some(post_val - baseline_val)
    }

    /// Returns `true` when the targeted dimension strictly improved.
    ///
    /// Returns `false` if no target was set, no post-score exists, or the
    /// dimension stayed flat / regressed.
    pub fn target_dimension_improved(&self) -> bool {
        self.target_dimension_delta()
            .map_or(false, |delta| delta > 0.0)
    }

    /// Compute per-dimension deltas between baseline and post-change scores.
    ///
    /// Returns a vec of `(dimension_name, delta)` pairs sorted by delta
    /// (largest improvement first). Returns empty if no post-score exists.
    pub fn dimension_deltas(&self) -> Vec<(String, f64)> {
        let post = match &self.post_score {
            Some(p) => p,
            None => return Vec::new(),
        };
        let mut deltas: Vec<(String, f64)> = super::prioritization::DIMENSION_NAMES
            .iter()
            .map(|&name| {
                let baseline_val = super::prioritization::dimension_value(&self.baseline, name);
                let post_val = super::prioritization::dimension_value(post, name);
                (name.to_string(), post_val - baseline_val)
            })
            .collect();
        deltas.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        deltas
    }
}

impl std::fmt::Display for ImprovementCycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&super::cycle::summarize_cycle(self))
    }
}

/// The convergence status of a multi-cycle improvement run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ConvergenceStatus {
    /// Overall scores are still improving meaningfully.
    Improving,
    /// Scores are flat — changes produce negligible movement.
    Plateau,
    /// Each successive cycle yields a smaller improvement than the last.
    DiminishingReturns,
    /// Overall scores are getting worse.
    Diverging,
}

impl std::fmt::Display for ConvergenceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Improving => "improving",
            Self::Plateau => "plateau",
            Self::DiminishingReturns => "diminishing-returns",
            Self::Diverging => "diverging",
        };
        f.write_str(label)
    }
}

/// Aggregates a sequence of [`ImprovementCycle`] records for multi-cycle analysis.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CycleHistory {
    /// Cycles in chronological order (oldest first).
    pub cycles: Vec<ImprovementCycle>,
}

impl CycleHistory {
    /// Create a new empty history.
    pub fn new() -> Self {
        Self { cycles: Vec::new() }
    }

    /// Push a completed cycle.
    pub fn push(&mut self, cycle: ImprovementCycle) {
        self.cycles.push(cycle);
    }

    /// Number of recorded cycles.
    pub fn len(&self) -> usize {
        self.cycles.len()
    }

    /// Returns `true` when no cycles have been recorded.
    pub fn is_empty(&self) -> bool {
        self.cycles.is_empty()
    }

    /// Overall-score velocity: rate of change per cycle.
    ///
    /// Computed as `(last_overall - first_overall) / (n - 1)`. Returns `0.0`
    /// when fewer than 2 cycles exist.
    pub fn overall_velocity(&self) -> f64 {
        if self.cycles.len() < 2 {
            return 0.0;
        }
        let first = self.cycles[0].baseline.overall;
        let last = self.cycles.last().map_or(first, |c| {
            c.post_score
                .as_ref()
                .map_or(c.baseline.overall, |p| p.overall)
        });
        let intervals = (self.cycles.len() - 1) as f64;
        let vel = (last - first) / intervals;
        if vel.is_finite() { vel } else { 0.0 }
    }

    /// Returns `true` when overall velocity is positive and exceeds `epsilon`.
    pub fn is_converging(&self, epsilon: f64) -> bool {
        self.overall_velocity() > epsilon
    }

    /// Returns `true` when the last `window` committed improvements each yield
    /// a smaller net gain than their predecessor.
    ///
    /// Requires at least `window` committed cycles. Returns `false` otherwise.
    pub fn diminishing_returns(&self, window: usize) -> bool {
        if window < 2 {
            return false;
        }
        let gains: Vec<f64> = self
            .cycles
            .iter()
            .filter_map(|c| {
                if let Some(ImprovementDecision::Commit { net_improvement }) = &c.decision {
                    Some(*net_improvement)
                } else {
                    None
                }
            })
            .collect();
        if gains.len() < window {
            return false;
        }
        let tail = &gains[gains.len() - window..];
        tail.windows(2).all(|pair| pair[1] < pair[0])
    }

    /// Returns a reference to the most recently committed cycle, or `None` if
    /// no cycle has been committed yet.
    pub fn last_committed(&self) -> Option<&ImprovementCycle> {
        self.cycles
            .iter()
            .rev()
            .find(|c| matches!(c.decision, Some(ImprovementDecision::Commit { .. })))
    }

    /// Fraction of cycles that resulted in a commit.
    ///
    /// Returns `0.0` when the history is empty.
    pub fn commit_rate(&self) -> f64 {
        if self.cycles.is_empty() {
            return 0.0;
        }
        let committed = self
            .cycles
            .iter()
            .filter(|c| matches!(c.decision, Some(ImprovementDecision::Commit { .. })))
            .count();
        committed as f64 / self.cycles.len() as f64
    }

    /// Return the baseline scores for all recorded cycles, in chronological order.
    ///
    /// Useful for feeding into [`prioritize_dimensions`](super::prioritization::prioritize_dimensions)
    /// or [`ImprovementCycle::enrich_with_history`] without manually iterating
    /// over `self.cycles`.
    pub fn baselines(&self) -> Vec<crate::gym_scoring::GymSuiteScore> {
        self.cycles.iter().map(|c| c.baseline.clone()).collect()
    }

    /// Return dimensions that appear in `plateau_dimensions` in at least
    /// `min_occurrences` of the last `window` cycles.
    ///
    /// This identifies dimensions that are *persistently* stuck rather than
    /// transiently plateaued in a single cycle, enabling stagnation-break
    /// rotation. A `window` of 3 with `min_occurrences` of 2 means: "has been
    /// plateaued in at least 2 of the last 3 cycles."
    ///
    /// Returns an empty vec when the history is shorter than `window`.
    pub fn persistent_plateau_dimensions(
        &self,
        window: usize,
        min_occurrences: usize,
    ) -> Vec<String> {
        if window == 0 || min_occurrences == 0 || self.cycles.len() < window {
            return Vec::new();
        }
        let recent = &self.cycles[self.cycles.len() - window..];
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for cycle in recent {
            for dim in &cycle.plateau_dimensions {
                *counts.entry(dim.as_str()).or_insert(0) += 1;
            }
        }
        let mut persistent: Vec<String> = counts
            .into_iter()
            .filter(|(_, count)| *count >= min_occurrences)
            .map(|(name, _)| name.to_string())
            .collect();
        persistent.sort();
        persistent
    }

    /// Suggest the next dimension to target, rotating away from persistently
    /// plateaued dimensions.
    ///
    /// Looks at the last `window` cycles to find dimensions that appear stuck
    /// in at least half of those cycles, then calls
    /// [`suggest_next_target_excluding`](super::prioritization::suggest_next_target_excluding)
    /// to pick the best non-stuck candidate for the next cycle.
    ///
    /// This is the primary entry-point for multi-cycle stagnation-break logic.
    /// Returns `None` when no dimension is currently weak.
    pub fn suggest_rotation_target(
        &self,
        current_score: &crate::gym_scoring::GymSuiteScore,
        weak_threshold: f64,
        window: usize,
    ) -> Option<super::prioritization::PrioritizedDimension> {
        let past = self.baselines();
        // Dimensions that have been persistently plateaued in at least half the
        // recent window should be skipped; we need at least 1 occurrence.
        let min_occ = (window / 2).max(1);
        let stuck = self.persistent_plateau_dimensions(window, min_occ);
        let exclude: Vec<&str> = stuck.iter().map(String::as_str).collect();
        super::prioritization::suggest_next_target_excluding(
            current_score,
            weak_threshold,
            &past,
            &exclude,
        )
    }

    /// Count the current streak of consecutively committed cycles from the tail.
    ///
    /// Returns 0 if the most recent cycle was reverted or the history is empty.
    /// A long streak signals that the improvement loop is "on a roll" and the
    /// current approach is working well; callers may use this to raise confidence
    /// thresholds or reduce exploration.
    pub fn commit_streak(&self) -> usize {
        self.cycles
            .iter()
            .rev()
            .take_while(|c| matches!(c.decision, Some(ImprovementDecision::Commit { .. })))
            .count()
    }

    /// Suggest the highest-priority weak dimension to target next, drawing on
    /// the full recorded history as context.
    ///
    /// Delegates to [`suggest_next_target`](super::prioritization::suggest_next_target)
    /// using the most recent cycle's baseline as the current score and all
    /// recorded baselines as historical context.
    ///
    /// Returns `None` when the history is empty or every dimension is already
    /// above `weak_threshold`.
    pub fn suggest_next_target(
        &self,
        weak_threshold: f64,
    ) -> Option<super::prioritization::PrioritizedDimension> {
        let current = self.cycles.last()?;
        let past = self.baselines();
        super::prioritization::suggest_next_target(&current.baseline, weak_threshold, &past)
    }

    /// Returns a reference to the committed cycle with the highest `net_improvement`.
    ///
    /// Returns `None` when no committed cycles exist.
    pub fn best_cycle(&self) -> Option<&ImprovementCycle> {
        self.cycles
            .iter()
            .filter_map(|c| {
                if let Some(ImprovementDecision::Commit { net_improvement }) = &c.decision {
                    Some((c, *net_improvement))
                } else {
                    None
                }
            })
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(c, _)| c)
    }

    /// Evaluate the convergence status of this history.
    ///
    /// - `epsilon`: minimum velocity to count as "improving" (e.g. 0.005).
    /// - `diminishing_window`: how many recent committed cycles to check for
    ///   diminishing returns (e.g. 3).
    pub fn evaluate_convergence(
        &self,
        epsilon: f64,
        diminishing_window: usize,
    ) -> ConvergenceStatus {
        if self.cycles.len() < 2 {
            return ConvergenceStatus::Improving;
        }
        let vel = self.overall_velocity();
        if vel < -epsilon {
            return ConvergenceStatus::Diverging;
        }
        if self.diminishing_returns(diminishing_window) {
            return ConvergenceStatus::DiminishingReturns;
        }
        if vel.abs() <= epsilon {
            return ConvergenceStatus::Plateau;
        }
        ConvergenceStatus::Improving
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gym_bridge::ScoreDimensions;

    fn make_score(v: f64) -> GymSuiteScore {
        GymSuiteScore {
            suite_id: "test".into(),
            overall: v,
            dimensions: ScoreDimensions {
                factual_accuracy: v,
                specificity: v * 0.9,
                temporal_awareness: v * 0.8,
                source_attribution: v * 0.7,
                confidence_calibration: v * 0.85,
            },
            scenario_count: 4,
            scenarios_passed: 4,
            pass_rate: 1.0,
            recorded_at_unix_ms: None,
        }
    }

    #[test]
    fn improvement_phase_display_all_variants() {
        assert_eq!(ImprovementPhase::Eval.to_string(), "eval");
        assert_eq!(ImprovementPhase::Analyze.to_string(), "analyze");
        assert_eq!(ImprovementPhase::Research.to_string(), "research");
        assert_eq!(ImprovementPhase::Improve.to_string(), "improve");
        assert_eq!(ImprovementPhase::ReEval.to_string(), "re-eval");
        assert_eq!(ImprovementPhase::Decide.to_string(), "decide");
    }

    #[test]
    fn improvement_phase_clone_and_eq() {
        let phase = ImprovementPhase::Research;
        let cloned = phase;
        assert_eq!(phase, cloned);
        assert_ne!(ImprovementPhase::Eval, ImprovementPhase::Decide);
    }

    #[test]
    fn proposed_change_construction() {
        let change = ProposedChange {
            file_path: "src/lib.rs".into(),
            description: "refactor error handling".into(),
            expected_impact: "reduce .expect() calls".into(),
        };
        assert_eq!(change.file_path, "src/lib.rs");
        assert!(!change.description.is_empty());
        assert!(!change.expected_impact.is_empty());
    }

    #[test]
    fn proposed_change_clone_and_eq() {
        let change = ProposedChange {
            file_path: "a.rs".into(),
            description: "d".into(),
            expected_impact: "e".into(),
        };
        let cloned = change.clone();
        assert_eq!(change, cloned);
    }

    #[test]
    fn improvement_decision_commit() {
        let d = ImprovementDecision::Commit {
            net_improvement: 0.05,
        };
        match &d {
            ImprovementDecision::Commit { net_improvement } => {
                assert!((net_improvement - 0.05).abs() < 1e-9);
            }
            _ => panic!("expected Commit"),
        }
    }

    #[test]
    fn improvement_decision_revert() {
        let d = ImprovementDecision::Revert {
            reason: "regression too large".into(),
        };
        match &d {
            ImprovementDecision::Revert { reason } => {
                assert!(reason.contains("regression"));
            }
            _ => panic!("expected Revert"),
        }
    }

    #[test]
    fn improvement_config_default_all_fields() {
        let cfg = ImprovementConfig::default();
        assert_eq!(cfg.suite_id, "progressive");
        assert!((cfg.min_net_improvement - 0.02).abs() < 1e-9);
        assert!((cfg.max_single_regression - 0.05).abs() < 1e-9);
        assert!(cfg.proposed_changes.is_empty());
        assert!(!cfg.auto_apply);
        assert!((cfg.weak_threshold - 0.6).abs() < 1e-9);
        assert!(cfg.target_dimension.is_none());
    }

    #[test]
    fn improvement_config_custom_target_dimension() {
        let cfg = ImprovementConfig {
            target_dimension: Some("specificity".into()),
            ..Default::default()
        };
        assert_eq!(cfg.target_dimension.as_deref(), Some("specificity"));
    }

    #[test]
    fn improvement_cycle_minimal() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.5),
            proposed_changes: Vec::new(),
            post_score: None,
            regressions: Vec::new(),
            decision: None,
            final_phase: ImprovementPhase::Eval,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: None,
            plateau_dimensions: Vec::new(),
        };
        assert!(cycle.proposed_changes.is_empty());
        assert!(cycle.post_score.is_none());
        assert!(cycle.decision.is_none());
        assert_eq!(cycle.final_phase, ImprovementPhase::Eval);
    }

    #[test]
    fn improvement_cycle_with_target_dimension() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.5),
            proposed_changes: vec![ProposedChange {
                file_path: "src/a.rs".into(),
                description: "improve specificity".into(),
                expected_impact: "better scores".into(),
            }],
            post_score: Some(make_score(0.7)),
            regressions: Vec::new(),
            decision: Some(ImprovementDecision::Commit {
                net_improvement: 0.2,
            }),
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: vec!["specificity".into()],
            weak_dimension_details: Vec::new(),
            target_dimension: Some("specificity".into()),
            plateau_dimensions: Vec::new(),
        };
        assert_eq!(cycle.target_dimension.as_deref(), Some("specificity"));
        assert_eq!(cycle.proposed_changes.len(), 1);
        assert_eq!(cycle.weak_dimensions.len(), 1);
    }

    #[test]
    fn improvement_cycle_display_contains_baseline() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.7),
            proposed_changes: Vec::new(),
            post_score: None,
            regressions: Vec::new(),
            decision: None,
            final_phase: ImprovementPhase::Analyze,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: None,
            plateau_dimensions: Vec::new(),
        };
        let display = cycle.to_string();
        assert!(display.contains("Baseline"));
        assert!(display.contains("70.0%"));
    }

    #[test]
    fn is_committed_true_for_commit_decision() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.7),
            proposed_changes: Vec::new(),
            post_score: Some(make_score(0.8)),
            regressions: Vec::new(),
            decision: Some(ImprovementDecision::Commit {
                net_improvement: 0.1,
            }),
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: None,
            plateau_dimensions: Vec::new(),
        };
        assert!(cycle.is_committed());
        assert!(!cycle.is_reverted());
    }

    #[test]
    fn is_reverted_true_for_revert_decision() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.7),
            proposed_changes: Vec::new(),
            post_score: None,
            regressions: Vec::new(),
            decision: Some(ImprovementDecision::Revert {
                reason: "test".into(),
            }),
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: None,
            plateau_dimensions: Vec::new(),
        };
        assert!(cycle.is_reverted());
        assert!(!cycle.is_committed());
    }

    #[test]
    fn is_committed_and_reverted_false_when_no_decision() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.7),
            proposed_changes: Vec::new(),
            post_score: None,
            regressions: Vec::new(),
            decision: None,
            final_phase: ImprovementPhase::Eval,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: None,
            plateau_dimensions: Vec::new(),
        };
        assert!(!cycle.is_committed());
        assert!(!cycle.is_reverted());
    }

    #[test]
    fn improvement_cycle_deserialize_without_target_dimension() {
        // Older JSON payloads may lack target_dimension; #[serde(default)] handles this.
        let json = r#"{
            "baseline": {"suite_id":"s","overall":0.5,"dimensions":{"factual_accuracy":0.5,"specificity":0.45,"temporal_awareness":0.4,"source_attribution":0.35,"confidence_calibration":0.42},"scenario_count":1,"scenarios_passed":1,"pass_rate":1.0,"recorded_at_unix_ms":null},
            "proposed_changes": [],
            "post_score": null,
            "regressions": [],
            "decision": null,
            "final_phase": "Eval",
            "weak_dimensions": []
        }"#;
        let cycle: ImprovementCycle =
            serde_json::from_str(json).expect("should deserialize without target_dimension");
        assert!(cycle.target_dimension.is_none());
        assert!(cycle.weak_dimension_details.is_empty());
    }

    #[test]
    fn validate_default_config_ok() {
        assert!(ImprovementConfig::default().validate().is_ok());
    }

    #[test]
    fn validate_empty_suite_id() {
        let cfg = ImprovementConfig {
            suite_id: String::new(),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(format!("{err:?}").contains("suite_id"));
    }

    #[test]
    fn validate_weak_threshold_out_of_range() {
        let above = ImprovementConfig {
            weak_threshold: 1.5,
            ..Default::default()
        };
        assert!(above.validate().is_err());

        let below = ImprovementConfig {
            weak_threshold: -0.1,
            ..Default::default()
        };
        assert!(below.validate().is_err());
    }

    #[test]
    fn validate_negative_min_net_improvement() {
        let cfg = ImprovementConfig {
            min_net_improvement: -0.01,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_negative_max_single_regression() {
        let cfg = ImprovementConfig {
            max_single_regression: -0.01,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_boundary_values_ok() {
        let zeros = ImprovementConfig {
            weak_threshold: 0.0,
            min_net_improvement: 0.0,
            max_single_regression: 0.0,
            ..Default::default()
        };
        assert!(zeros.validate().is_ok());

        let threshold_one = ImprovementConfig {
            weak_threshold: 1.0,
            ..Default::default()
        };
        assert!(threshold_one.validate().is_ok());
    }

    #[test]
    fn validate_unknown_target_dimension_rejected() {
        let cfg = ImprovementConfig {
            target_dimension: Some("not_a_real_dimension".into()),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(format!("{err:?}").contains("target_dimension"));
    }

    #[test]
    fn validate_valid_target_dimension_accepted() {
        for dim in &[
            "factual_accuracy",
            "specificity",
            "temporal_awareness",
            "source_attribution",
            "confidence_calibration",
        ] {
            let cfg = ImprovementConfig {
                target_dimension: Some(dim.to_string()),
                ..Default::default()
            };
            assert!(cfg.validate().is_ok(), "dimension '{dim}' should be valid");
        }
    }

    #[test]
    fn validate_none_target_dimension_accepted() {
        let cfg = ImprovementConfig {
            target_dimension: None,
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn enrich_with_history_fills_plateau_dimensions() {
        let mut cycle = ImprovementCycle {
            baseline: make_score(0.5),
            proposed_changes: Vec::new(),
            post_score: None,
            regressions: Vec::new(),
            decision: None,
            final_phase: ImprovementPhase::Eval,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: None,
            plateau_dimensions: Vec::new(),
        };
        let past = vec![make_score(0.5), make_score(0.5), make_score(0.5)];
        cycle.enrich_with_history(0.6, &past);
        assert!(!cycle.plateau_dimensions.is_empty());
    }

    #[test]
    fn validate_max_cycles_zero_rejected() {
        let cfg = ImprovementConfig {
            max_cycles: Some(0),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(format!("{err:?}").contains("max_cycles"));
    }

    #[test]
    fn validate_max_cycles_positive_accepted() {
        let cfg = ImprovementConfig {
            max_cycles: Some(5),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_max_cycles_none_accepted() {
        let cfg = ImprovementConfig {
            max_cycles: None,
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn dimension_deltas_with_nan_scores() {
        let baseline = make_score(0.5);
        let post = GymSuiteScore {
            suite_id: "test".into(),
            overall: f64::NAN,
            dimensions: ScoreDimensions {
                factual_accuracy: f64::NAN,
                specificity: f64::NAN,
                temporal_awareness: f64::NAN,
                source_attribution: f64::NAN,
                confidence_calibration: f64::NAN,
            },
            scenario_count: 0,
            scenarios_passed: 0,
            pass_rate: 0.0,
            recorded_at_unix_ms: None,
        };
        let cycle = ImprovementCycle {
            baseline,
            proposed_changes: Vec::new(),
            post_score: Some(post),
            regressions: Vec::new(),
            decision: None,
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: None,
            plateau_dimensions: Vec::new(),
        };
        let deltas = cycle.dimension_deltas();
        assert_eq!(deltas.len(), 5);
    }

    #[test]
    fn target_dimension_delta_returns_none_without_target() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.5),
            proposed_changes: Vec::new(),
            post_score: Some(make_score(0.6)),
            regressions: Vec::new(),
            decision: None,
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: None,
            plateau_dimensions: Vec::new(),
        };
        assert!(cycle.target_dimension_delta().is_none());
        assert!(!cycle.target_dimension_improved());
    }

    #[test]
    fn target_dimension_delta_returns_none_without_post_score() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.5),
            proposed_changes: Vec::new(),
            post_score: None,
            regressions: Vec::new(),
            decision: None,
            final_phase: ImprovementPhase::Analyze,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: Some("specificity".into()),
            plateau_dimensions: Vec::new(),
        };
        assert!(cycle.target_dimension_delta().is_none());
        assert!(!cycle.target_dimension_improved());
    }

    #[test]
    fn target_dimension_delta_positive_when_improved() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.5),
            proposed_changes: Vec::new(),
            post_score: Some(make_score(0.7)),
            regressions: Vec::new(),
            decision: None,
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: Some("factual_accuracy".into()),
            plateau_dimensions: Vec::new(),
        };
        let delta = cycle.target_dimension_delta().expect("should have delta");
        assert!((delta - 0.2).abs() < 1e-9);
        assert!(cycle.target_dimension_improved());
    }

    #[test]
    fn target_dimension_delta_negative_when_regressed() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.7),
            proposed_changes: Vec::new(),
            post_score: Some(make_score(0.5)),
            regressions: Vec::new(),
            decision: None,
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: Some("specificity".into()),
            plateau_dimensions: Vec::new(),
        };
        let delta = cycle.target_dimension_delta().expect("should have delta");
        // specificity = overall * 0.9, so 0.5*0.9 - 0.7*0.9 = -0.18
        assert!(delta < 0.0);
        assert!(!cycle.target_dimension_improved());
    }

    #[test]
    fn target_dimension_improved_false_when_flat() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.5),
            proposed_changes: Vec::new(),
            post_score: Some(make_score(0.5)),
            regressions: Vec::new(),
            decision: None,
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: Some("factual_accuracy".into()),
            plateau_dimensions: Vec::new(),
        };
        assert!(!cycle.target_dimension_improved());
    }

    fn make_commit_cycle(overall: f64, net: f64) -> ImprovementCycle {
        ImprovementCycle {
            baseline: make_score(overall),
            proposed_changes: Vec::new(),
            post_score: Some(make_score(overall + net)),
            regressions: Vec::new(),
            decision: Some(ImprovementDecision::Commit {
                net_improvement: net,
            }),
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: None,
            plateau_dimensions: Vec::new(),
        }
    }

    fn make_revert_cycle(overall: f64) -> ImprovementCycle {
        ImprovementCycle {
            baseline: make_score(overall),
            proposed_changes: Vec::new(),
            post_score: None,
            regressions: Vec::new(),
            decision: Some(ImprovementDecision::Revert {
                reason: "test".into(),
            }),
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: None,
            plateau_dimensions: Vec::new(),
        }
    }

    // ---- CycleHistory::commit_streak ----

    #[test]
    fn commit_streak_empty_history_is_zero() {
        let history = CycleHistory::new();
        assert_eq!(history.commit_streak(), 0);
    }

    #[test]
    fn commit_streak_all_commits() {
        let mut history = CycleHistory::new();
        history.push(make_commit_cycle(0.5, 0.05));
        history.push(make_commit_cycle(0.55, 0.05));
        history.push(make_commit_cycle(0.60, 0.05));
        assert_eq!(history.commit_streak(), 3);
    }

    #[test]
    fn commit_streak_revert_breaks_streak() {
        let mut history = CycleHistory::new();
        history.push(make_commit_cycle(0.5, 0.05));
        history.push(make_commit_cycle(0.55, 0.05));
        history.push(make_revert_cycle(0.60));
        assert_eq!(history.commit_streak(), 0);
    }

    #[test]
    fn commit_streak_counts_only_trailing_commits() {
        let mut history = CycleHistory::new();
        history.push(make_revert_cycle(0.5));
        history.push(make_commit_cycle(0.5, 0.05));
        history.push(make_commit_cycle(0.55, 0.05));
        assert_eq!(history.commit_streak(), 2);
    }

    // ---- CycleHistory::suggest_next_target ----

    #[test]
    fn suggest_next_target_empty_history_returns_none() {
        let history = CycleHistory::new();
        assert!(history.suggest_next_target(0.6).is_none());
    }

    #[test]
    fn suggest_next_target_all_strong_returns_none() {
        let mut history = CycleHistory::new();
        history.push(make_commit_cycle(0.9, 0.02));
        // All dimensions are above 0.6 when overall = 0.9
        assert!(history.suggest_next_target(0.6).is_none());
    }

    #[test]
    fn suggest_next_target_returns_highest_priority_weak_dim() {
        let mut history = CycleHistory::new();
        history.push(make_commit_cycle(0.5, 0.02));
        history.push(make_commit_cycle(0.52, 0.02));
        history.push(make_commit_cycle(0.54, 0.02));
        // source_attribution = overall * 0.7 ≈ 0.35–0.38, highest deficit
        let suggestion = history
            .suggest_next_target(0.6)
            .expect("should suggest a target");
        assert_eq!(suggestion.name, "source_attribution");
        assert!(suggestion.current_deficit > 0.0);
    }
}
