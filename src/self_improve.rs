//! Self-improvement loop that evaluates, analyzes, and decides on changes.
//!
//! The improvement cycle follows a disciplined sequence:
//! `Eval -> Analyze -> Research -> Improve -> ReEval -> Decide`.
//!
//! Each cycle produces a typed [`ImprovementCycle`] record with full
//! provenance so decisions are reviewable (Pillar 6). Changes are only
//! committed when the net improvement meets the threshold and no single
//! dimension regresses beyond the allowed maximum (Pillar 11).

use crate::error::SimardResult;
use crate::gym_bridge::GymBridge;
use crate::gym_scoring::{
    GymSuiteScore, Regression, RegressionSeverity, detect_regression, suite_score_from_result,
};

/// Phases of a single self-improvement cycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProposedChange {
    /// Path to the file that would be changed.
    pub file_path: String,
    /// Human-readable description of the change.
    pub description: String,
    /// Why this change is expected to help.
    pub expected_impact: String,
}

/// The outcome of the decision phase.
#[derive(Clone, Debug, PartialEq)]
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
}

impl Default for ImprovementConfig {
    fn default() -> Self {
        Self {
            suite_id: "progressive".to_string(),
            min_net_improvement: 0.02,
            max_single_regression: 0.05,
            proposed_changes: Vec::new(),
        }
    }
}

/// A complete improvement cycle record with full provenance.
#[derive(Clone, Debug)]
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
}

/// Run a full improvement cycle: Eval -> Analyze -> Research -> Improve -> ReEval -> Decide.
///
/// The cycle requires at least one proposed change in `config.proposed_changes`.
/// If no changes are proposed, the cycle stops after Analyze and returns a
/// Revert decision.
///
/// The gym bridge is called twice: once for baseline and once for re-evaluation.
/// If either call fails, the error propagates immediately (Pillar 11).
pub fn run_improvement_cycle(
    gym: &GymBridge,
    config: &ImprovementConfig,
) -> SimardResult<ImprovementCycle> {
    // Phase 1: Eval — establish baseline
    let baseline_result = gym.run_suite(&config.suite_id)?;
    let baseline = suite_score_from_result(&baseline_result);

    // Phase 2: Analyze — identify weak dimensions
    let weak_dimensions = find_weak_dimensions(&baseline);

    // Phase 3: Research — check if we have proposed changes
    if config.proposed_changes.is_empty() {
        return Ok(ImprovementCycle {
            baseline,
            proposed_changes: Vec::new(),
            post_score: None,
            regressions: Vec::new(),
            decision: Some(ImprovementDecision::Revert {
                reason: format!(
                    "no changes proposed; weak dimensions: {}",
                    if weak_dimensions.is_empty() {
                        "none".to_string()
                    } else {
                        weak_dimensions.join(", ")
                    }
                ),
            }),
            final_phase: ImprovementPhase::Analyze,
        });
    }

    // Phase 4: Improve — changes are assumed to be applied externally
    // (the caller applies the changes before calling this function in a
    // canary environment; see self_relaunch.rs for the canary flow).

    // Phase 5: ReEval — re-run the suite
    let post_result = gym.run_suite(&config.suite_id)?;
    let post_score = suite_score_from_result(&post_result);

    // Phase 6: Decide — compare and decide
    let regressions = detect_regression(&post_score, &baseline);
    let decision = decide(config, &baseline, &post_score, &regressions);

    Ok(ImprovementCycle {
        baseline,
        proposed_changes: config.proposed_changes.clone(),
        post_score: Some(post_score),
        regressions,
        decision: Some(decision),
        final_phase: ImprovementPhase::Decide,
    })
}

/// Apply the decision rule: commit if net improvement >= threshold
/// and no single dimension regresses beyond the allowed maximum.
fn decide(
    config: &ImprovementConfig,
    baseline: &GymSuiteScore,
    post: &GymSuiteScore,
    regressions: &[Regression],
) -> ImprovementDecision {
    let net = post.overall - baseline.overall;

    // Check for severe regressions first
    let worst_regression = regressions
        .iter()
        .map(|r| r.delta.abs())
        .fold(0.0_f64, f64::max);

    if worst_regression > config.max_single_regression {
        let severe: Vec<&Regression> = regressions
            .iter()
            .filter(|r| r.delta.abs() > config.max_single_regression)
            .collect();
        let detail: Vec<String> = severe
            .iter()
            .map(|r| format!("{}: {:.1}% regression", r.dimension, r.delta.abs() * 100.0))
            .collect();
        return ImprovementDecision::Revert {
            reason: format!(
                "regression exceeds max allowed ({:.1}%): {}",
                config.max_single_regression * 100.0,
                detail.join("; ")
            ),
        };
    }

    if net < config.min_net_improvement {
        return ImprovementDecision::Revert {
            reason: format!(
                "net improvement {:.1}% is below minimum threshold {:.1}%",
                net * 100.0,
                config.min_net_improvement * 100.0,
            ),
        };
    }

    ImprovementDecision::Commit {
        net_improvement: net,
    }
}

/// Identify dimensions scoring below 0.6 (the "weak" threshold).
fn find_weak_dimensions(score: &GymSuiteScore) -> Vec<String> {
    const WEAK_THRESHOLD: f64 = 0.6;
    let dims = &score.dimensions;
    let mut weak = Vec::new();
    let checks: [(&str, f64); 5] = [
        ("factual_accuracy", dims.factual_accuracy),
        ("specificity", dims.specificity),
        ("temporal_awareness", dims.temporal_awareness),
        ("source_attribution", dims.source_attribution),
        ("confidence_calibration", dims.confidence_calibration),
    ];
    for (name, value) in checks {
        if value < WEAK_THRESHOLD {
            weak.push(name.to_string());
        }
    }
    weak
}

/// Summary of an improvement cycle suitable for persistence or display.
pub fn summarize_cycle(cycle: &ImprovementCycle) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Baseline: {:.1}% overall ({} scenarios)",
        cycle.baseline.overall * 100.0,
        cycle.baseline.scenario_count,
    ));

    if let Some(ref post) = cycle.post_score {
        let net = post.overall - cycle.baseline.overall;
        lines.push(format!(
            "Post-change: {:.1}% overall (net {}{:.1}%)",
            post.overall * 100.0,
            if net >= 0.0 { "+" } else { "" },
            net * 100.0,
        ));
    }

    if !cycle.regressions.is_empty() {
        let severe_count = cycle
            .regressions
            .iter()
            .filter(|r| r.severity == RegressionSeverity::Severe)
            .count();
        lines.push(format!(
            "Regressions: {} total ({} severe)",
            cycle.regressions.len(),
            severe_count,
        ));
    }

    match &cycle.decision {
        Some(ImprovementDecision::Commit { net_improvement }) => {
            lines.push(format!(
                "Decision: COMMIT (net +{:.1}%)",
                net_improvement * 100.0
            ));
        }
        Some(ImprovementDecision::Revert { reason }) => {
            lines.push(format!("Decision: REVERT ({reason})"));
        }
        None => {
            lines.push(format!(
                "Decision: INCOMPLETE (stopped at {})",
                cycle.final_phase
            ));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn improvement_phase_display() {
        assert_eq!(ImprovementPhase::Eval.to_string(), "eval");
        assert_eq!(ImprovementPhase::ReEval.to_string(), "re-eval");
        assert_eq!(ImprovementPhase::Decide.to_string(), "decide");
    }

    #[test]
    fn default_config_thresholds() {
        let cfg = ImprovementConfig::default();
        assert!((cfg.min_net_improvement - 0.02).abs() < 1e-9);
        assert!((cfg.max_single_regression - 0.05).abs() < 1e-9);
    }

    #[test]
    fn decide_commits_when_improvement_sufficient() {
        let cfg = ImprovementConfig::default();
        let baseline = make_score(0.70);
        let post = make_score(0.75);
        let regressions = vec![];
        let d = decide(&cfg, &baseline, &post, &regressions);
        match d {
            ImprovementDecision::Commit { net_improvement } => {
                assert!((net_improvement - 0.05).abs() < 1e-9);
            }
            ImprovementDecision::Revert { .. } => panic!("expected commit"),
        }
    }

    #[test]
    fn decide_reverts_when_below_threshold() {
        let cfg = ImprovementConfig::default();
        let baseline = make_score(0.70);
        let post = make_score(0.71);
        let regressions = vec![];
        let d = decide(&cfg, &baseline, &post, &regressions);
        assert!(matches!(d, ImprovementDecision::Revert { .. }));
    }

    #[test]
    fn decide_reverts_on_severe_regression() {
        let cfg = ImprovementConfig::default();
        let baseline = make_score(0.70);
        let post = make_score(0.80);
        let regressions = vec![Regression {
            dimension: "specificity".to_string(),
            baseline_score: 0.7,
            current_score: 0.6,
            delta: -0.10,
            severity: RegressionSeverity::Moderate,
        }];
        let d = decide(&cfg, &baseline, &post, &regressions);
        assert!(matches!(d, ImprovementDecision::Revert { .. }));
    }

    #[test]
    fn find_weak_dimensions_identifies_low_scores() {
        let score = make_score(0.50);
        let weak = find_weak_dimensions(&score);
        // At 0.50 overall, all dimension values (0.50, 0.45, 0.40, 0.35, 0.425)
        // are below 0.6
        assert!(!weak.is_empty());
    }

    #[test]
    fn find_weak_dimensions_empty_when_strong() {
        let score = make_score(0.90);
        let weak = find_weak_dimensions(&score);
        assert!(weak.is_empty());
    }

    fn make_score(v: f64) -> GymSuiteScore {
        use crate::gym_bridge::ScoreDimensions;
        GymSuiteScore {
            suite_id: "test".to_string(),
            overall: v,
            dimensions: ScoreDimensions {
                factual_accuracy: v,
                specificity: v * 0.9,
                temporal_awareness: v * 0.8,
                source_attribution: v * 0.7,
                confidence_calibration: v * 0.85,
            },
            scenario_count: 6,
            scenarios_passed: 6,
            pass_rate: 1.0,
            recorded_at_unix_ms: None,
        }
    }
}
