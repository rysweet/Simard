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
#[derive(Clone, Debug)]
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
        if self.min_net_improvement < 0.0 {
            return Err(crate::error::SimardError::InvalidImprovementRecord {
                field: "min_net_improvement".into(),
                reason: format!(
                    "min_net_improvement must be >= 0.0, got {}",
                    self.min_net_improvement
                ),
            });
        }
        if self.max_single_regression < 0.0 {
            return Err(crate::error::SimardError::InvalidImprovementRecord {
                field: "max_single_regression".into(),
                reason: format!(
                    "max_single_regression must be >= 0.0, got {}",
                    self.max_single_regression
                ),
            });
        }
        if let Some(ref dim) = self.target_dimension {
            if !super::prioritization::DIMENSION_NAMES.contains(&dim.as_str()) {
                return Err(crate::error::SimardError::InvalidImprovementRecord {
                    field: "target_dimension".into(),
                    reason: format!(
                        "unknown dimension '{}'; valid dimensions: {}",
                        dim,
                        super::prioritization::DIMENSION_NAMES.join(", ")
                    ),
                });
            }
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
}
