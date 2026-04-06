//! Improvement cycle execution, decision logic, and analysis.

use std::path::Path;

use crate::engineer_loop::RepoInspection;
use crate::error::SimardResult;
use crate::gym_bridge::GymBridge;
use crate::gym_scoring::{
    GymSuiteScore, Regression, RegressionSeverity, detect_regression, suite_score_from_result,
};
use crate::self_improve_executor::ApplyResult;

use super::types::{ImprovementConfig, ImprovementCycle, ImprovementDecision, ImprovementPhase};

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
    let weak_dimensions = find_weak_dimensions(
        &baseline,
        config.weak_threshold,
        config.target_dimension.as_deref(),
    );

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
            weak_dimensions,
            target_dimension: config.target_dimension.clone(),
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
        weak_dimensions,
        target_dimension: config.target_dimension.clone(),
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

/// Identify dimensions scoring below the given threshold.
///
/// When `target` is `Some`, only that dimension is checked. When `None`, all
/// five standard dimensions are checked.
fn find_weak_dimensions(
    score: &GymSuiteScore,
    weak_threshold: f64,
    target: Option<&str>,
) -> Vec<String> {
    let dims = &score.dimensions;
    let checks: [(&str, f64); 5] = [
        ("factual_accuracy", dims.factual_accuracy),
        ("specificity", dims.specificity),
        ("temporal_awareness", dims.temporal_awareness),
        ("source_attribution", dims.source_attribution),
        ("confidence_calibration", dims.confidence_calibration),
    ];
    let mut weak = Vec::new();
    for (name, value) in checks {
        if let Some(t) = target {
            if name != t {
                continue;
            }
        }
        if value < weak_threshold {
            weak.push(name.to_string());
        }
    }
    weak
}

/// Summary of an improvement cycle suitable for persistence or display.
pub fn summarize_cycle(cycle: &ImprovementCycle) -> String {
    let mut lines = Vec::new();

    if let Some(ref dim) = cycle.target_dimension {
        lines.push(format!("Target dimension: {dim}"));
    }

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

/// Apply improvement proposals autonomously via the plan+review pipeline.
///
/// Delegates to [`crate::self_improve_executor::run_autonomous_improvement`].
/// Each proposal is planned, executed, reviewed, and committed or rolled back.
pub fn apply_improvements(
    proposals: &[String],
    workspace: &Path,
    inspection: &RepoInspection,
) -> Vec<ApplyResult> {
    crate::self_improve_executor::run_autonomous_improvement(proposals, workspace, inspection)
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
        let weak = find_weak_dimensions(&score, 0.6, None);
        // At 0.50 overall, all dimension values (0.50, 0.45, 0.40, 0.35, 0.425)
        // are below 0.6
        assert!(!weak.is_empty());
    }

    #[test]
    fn find_weak_dimensions_empty_when_strong() {
        let score = make_score(0.90);
        let weak = find_weak_dimensions(&score, 0.6, None);
        assert!(weak.is_empty());
    }

    #[test]
    fn find_weak_dimensions_custom_threshold() {
        let score = make_score(0.50);
        let weak = find_weak_dimensions(&score, 0.1, None);
        assert!(weak.is_empty());
    }

    #[test]
    fn summarize_cycle_commit() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.70),
            proposed_changes: vec![],
            post_score: Some(make_score(0.75)),
            regressions: vec![],
            decision: Some(ImprovementDecision::Commit {
                net_improvement: 0.05,
            }),
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            target_dimension: None,
        };
        let summary = summarize_cycle(&cycle);
        assert!(summary.contains("COMMIT"));
        assert!(summary.contains("+5.0%"));
    }

    #[test]
    fn summarize_cycle_revert() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.70),
            proposed_changes: vec![],
            post_score: Some(make_score(0.71)),
            regressions: vec![],
            decision: Some(ImprovementDecision::Revert {
                reason: "below threshold".into(),
            }),
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            target_dimension: None,
        };
        let summary = summarize_cycle(&cycle);
        assert!(summary.contains("REVERT"));
        assert!(summary.contains("below threshold"));
    }

    #[test]
    fn summarize_cycle_incomplete() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.70),
            proposed_changes: vec![],
            post_score: None,
            regressions: vec![],
            decision: None,
            final_phase: ImprovementPhase::Analyze,
            weak_dimensions: Vec::new(),
            target_dimension: None,
        };
        let summary = summarize_cycle(&cycle);
        assert!(summary.contains("INCOMPLETE"));
        assert!(summary.contains("analyze"));
    }

    #[test]
    fn improvement_cycle_display_delegates_to_summarize() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.70),
            proposed_changes: vec![],
            post_score: Some(make_score(0.75)),
            regressions: vec![],
            decision: Some(ImprovementDecision::Commit {
                net_improvement: 0.05,
            }),
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            target_dimension: None,
        };
        assert_eq!(cycle.to_string(), summarize_cycle(&cycle));
    }

    #[test]
    fn decide_reverts_on_negative_net() {
        let cfg = ImprovementConfig::default();
        let baseline = make_score(0.75);
        let post = make_score(0.70);
        let d = decide(&cfg, &baseline, &post, &[]);
        match d {
            ImprovementDecision::Revert { reason } => {
                assert!(reason.contains("below minimum threshold"));
            }
            ImprovementDecision::Commit { .. } => panic!("expected revert on negative net"),
        }
    }

    #[test]
    fn decide_commits_at_exact_threshold() {
        let cfg = ImprovementConfig::default(); // min_net_improvement = 0.02
        let baseline = make_score(0.70);
        let post = make_score(0.72); // net = 0.02, exactly at threshold
        let d = decide(&cfg, &baseline, &post, &[]);
        assert!(
            matches!(d, ImprovementDecision::Commit { .. }),
            "expected commit at exact threshold, got revert"
        );
    }

    #[test]
    fn decide_regression_at_exact_max_commits() {
        let cfg = ImprovementConfig::default(); // max_single_regression = 0.05
        let baseline = make_score(0.70);
        let post = make_score(0.80);
        let regressions = vec![Regression {
            dimension: "specificity".to_string(),
            baseline_score: 0.7,
            current_score: 0.65,
            delta: -0.05, // exactly at max — should NOT trigger revert
            severity: RegressionSeverity::Minor,
        }];
        let d = decide(&cfg, &baseline, &post, &regressions);
        assert!(
            matches!(d, ImprovementDecision::Commit { .. }),
            "expected commit when regression is exactly at max, got revert"
        );
    }

    #[test]
    fn find_weak_dimensions_mixed() {
        // Build a score where some dimensions are above and some below 0.6.
        use crate::gym_bridge::ScoreDimensions;
        let score = GymSuiteScore {
            suite_id: "test".to_string(),
            overall: 0.65,
            dimensions: ScoreDimensions {
                factual_accuracy: 0.80,       // above
                specificity: 0.50,            // below
                temporal_awareness: 0.70,     // above
                source_attribution: 0.40,     // below
                confidence_calibration: 0.90, // above
            },
            scenario_count: 6,
            scenarios_passed: 6,
            pass_rate: 1.0,
            recorded_at_unix_ms: None,
        };
        let weak = find_weak_dimensions(&score, 0.6, None);
        assert_eq!(weak.len(), 2);
        assert!(weak.contains(&"specificity".to_string()));
        assert!(weak.contains(&"source_attribution".to_string()));
    }

    #[test]
    fn summarize_cycle_with_regressions() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.70),
            proposed_changes: vec![],
            post_score: Some(make_score(0.75)),
            regressions: vec![
                Regression {
                    dimension: "specificity".to_string(),
                    baseline_score: 0.7,
                    current_score: 0.6,
                    delta: -0.10,
                    severity: RegressionSeverity::Severe,
                },
                Regression {
                    dimension: "temporal_awareness".to_string(),
                    baseline_score: 0.6,
                    current_score: 0.55,
                    delta: -0.05,
                    severity: RegressionSeverity::Minor,
                },
            ],
            decision: Some(ImprovementDecision::Revert {
                reason: "regression".into(),
            }),
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            target_dimension: None,
        };
        let summary = summarize_cycle(&cycle);
        assert!(summary.contains("Regressions: 2 total (1 severe)"));
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

    #[test]
    fn find_weak_dimensions_with_target_filters_single() {
        use crate::gym_bridge::ScoreDimensions;
        let score = GymSuiteScore {
            suite_id: "test".to_string(),
            overall: 0.65,
            dimensions: ScoreDimensions {
                factual_accuracy: 0.80,
                specificity: 0.50,
                temporal_awareness: 0.70,
                source_attribution: 0.40,
                confidence_calibration: 0.90,
            },
            scenario_count: 6,
            scenarios_passed: 6,
            pass_rate: 1.0,
            recorded_at_unix_ms: None,
        };
        // Target specificity (weak) — should return it
        let weak = find_weak_dimensions(&score, 0.6, Some("specificity"));
        assert_eq!(weak, vec!["specificity"]);

        // Target factual_accuracy (strong) — should return empty
        let weak = find_weak_dimensions(&score, 0.6, Some("factual_accuracy"));
        assert!(weak.is_empty());

        // Target source_attribution (weak) — should return it
        let weak = find_weak_dimensions(&score, 0.6, Some("source_attribution"));
        assert_eq!(weak, vec!["source_attribution"]);
    }

    #[test]
    fn find_weak_dimensions_unknown_target_returns_empty() {
        let score = make_score(0.50);
        let weak = find_weak_dimensions(&score, 0.6, Some("nonexistent_dim"));
        assert!(weak.is_empty());
    }

    #[test]
    fn summarize_cycle_shows_target_dimension() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.70),
            proposed_changes: vec![],
            post_score: Some(make_score(0.75)),
            regressions: vec![],
            decision: Some(ImprovementDecision::Commit {
                net_improvement: 0.05,
            }),
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            target_dimension: Some("specificity".to_string()),
        };
        let summary = summarize_cycle(&cycle);
        assert!(summary.contains("Target dimension: specificity"));
        assert!(summary.contains("COMMIT"));
    }

    #[test]
    fn summarize_cycle_omits_target_when_none() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.70),
            proposed_changes: vec![],
            post_score: Some(make_score(0.75)),
            regressions: vec![],
            decision: Some(ImprovementDecision::Commit {
                net_improvement: 0.05,
            }),
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            target_dimension: None,
        };
        let summary = summarize_cycle(&cycle);
        assert!(!summary.contains("Target dimension"));
    }

    #[test]
    fn default_config_has_no_target_dimension() {
        let cfg = ImprovementConfig::default();
        assert!(cfg.target_dimension.is_none());
    }

    #[test]
    fn decide_with_zero_scores() {
        let cfg = ImprovementConfig::default();
        let baseline = make_score(0.0);
        let post = make_score(0.0);
        let d = decide(&cfg, &baseline, &post, &[]);
        assert!(
            matches!(d, ImprovementDecision::Revert { .. }),
            "zero net improvement should revert"
        );
    }

    #[test]
    fn decide_with_multiple_severe_regressions() {
        let cfg = ImprovementConfig::default();
        let baseline = make_score(0.70);
        let post = make_score(0.80);
        let regressions = vec![
            Regression {
                dimension: "specificity".into(),
                baseline_score: 0.7,
                current_score: 0.5,
                delta: -0.20,
                severity: RegressionSeverity::Severe,
            },
            Regression {
                dimension: "temporal_awareness".into(),
                baseline_score: 0.6,
                current_score: 0.4,
                delta: -0.20,
                severity: RegressionSeverity::Severe,
            },
        ];
        let d = decide(&cfg, &baseline, &post, &regressions);
        match d {
            ImprovementDecision::Revert { reason } => {
                assert!(reason.contains("specificity"));
                assert!(reason.contains("temporal_awareness"));
            }
            ImprovementDecision::Commit { .. } => {
                panic!("expected revert with multiple severe regressions")
            }
        }
    }

    #[test]
    fn find_weak_dimensions_threshold_zero_all_pass() {
        let score = make_score(0.01);
        let weak = find_weak_dimensions(&score, 0.0, None);
        assert!(weak.is_empty(), "threshold 0.0 should pass all dimensions");
    }

    #[test]
    fn find_weak_dimensions_threshold_one_all_fail() {
        let score = make_score(0.99);
        let weak = find_weak_dimensions(&score, 1.0, None);
        assert_eq!(weak.len(), 5, "threshold 1.0 should flag all dimensions");
    }

    #[test]
    fn summarize_cycle_negative_improvement() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.80),
            proposed_changes: vec![],
            post_score: Some(make_score(0.70)),
            regressions: vec![],
            decision: Some(ImprovementDecision::Revert {
                reason: "net negative".into(),
            }),
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            target_dimension: None,
        };
        let summary = summarize_cycle(&cycle);
        assert!(summary.contains("REVERT"));
        assert!(summary.contains("-10.0%"));
    }
}
