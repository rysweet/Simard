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
pub(super) fn decide(
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
pub(super) fn find_weak_dimensions(
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
        if let Some(t) = target
            && name != t
        {
            continue;
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
    use crate::gym_bridge::ScoreDimensions;

    fn make_score(overall: f64) -> GymSuiteScore {
        GymSuiteScore {
            suite_id: "test".into(),
            overall,
            dimensions: ScoreDimensions {
                factual_accuracy: overall,
                specificity: overall * 0.9,
                temporal_awareness: overall * 0.8,
                source_attribution: overall * 0.7,
                confidence_calibration: overall * 0.85,
            },
            scenario_count: 4,
            scenarios_passed: 4,
            pass_rate: 1.0,
            recorded_at_unix_ms: None,
        }
    }

    fn make_config() -> ImprovementConfig {
        ImprovementConfig::default()
    }

    // ---- decide ----

    #[test]
    fn decide_commit_when_net_improvement_above_threshold() {
        let config = make_config();
        let baseline = make_score(0.5);
        let post = make_score(0.6);
        let decision = decide(&config, &baseline, &post, &[]);
        assert!(matches!(decision, ImprovementDecision::Commit { .. }));
        if let ImprovementDecision::Commit { net_improvement } = decision {
            assert!((net_improvement - 0.1).abs() < 0.001);
        }
    }

    #[test]
    fn decide_revert_when_net_below_threshold() {
        let config = make_config();
        let baseline = make_score(0.5);
        let post = make_score(0.51);
        let decision = decide(&config, &baseline, &post, &[]);
        assert!(matches!(decision, ImprovementDecision::Revert { .. }));
    }

    #[test]
    fn decide_revert_on_severe_regression() {
        let config = make_config();
        let baseline = make_score(0.5);
        let post = make_score(0.6);
        let regressions = vec![Regression {
            dimension: "specificity".into(),
            baseline_score: 0.5,
            current_score: 0.3,
            delta: -0.2,
            severity: RegressionSeverity::Severe,
        }];
        let decision = decide(&config, &baseline, &post, &regressions);
        assert!(matches!(decision, ImprovementDecision::Revert { .. }));
    }

    #[test]
    fn decide_commit_with_minor_regression() {
        let config = make_config();
        let baseline = make_score(0.5);
        let post = make_score(0.6);
        let regressions = vec![Regression {
            dimension: "specificity".into(),
            baseline_score: 0.5,
            current_score: 0.48,
            delta: -0.02,
            severity: RegressionSeverity::Minor,
        }];
        let decision = decide(&config, &baseline, &post, &regressions);
        assert!(matches!(decision, ImprovementDecision::Commit { .. }));
    }

    // ---- find_weak_dimensions ----

    #[test]
    fn find_weak_dimensions_all_above_threshold() {
        let score = make_score(0.8);
        let weak = find_weak_dimensions(&score, 0.5, None);
        assert!(weak.is_empty());
    }

    #[test]
    fn find_weak_dimensions_some_below() {
        let score = make_score(0.5);
        let weak = find_weak_dimensions(&score, 0.45, None);
        // source_attribution = 0.5 * 0.7 = 0.35, below 0.45
        assert!(weak.contains(&"source_attribution".to_string()));
    }

    #[test]
    fn find_weak_dimensions_with_target() {
        let score = make_score(0.5);
        let weak = find_weak_dimensions(&score, 0.6, Some("factual_accuracy"));
        // factual_accuracy = 0.5, below 0.6
        assert_eq!(weak.len(), 1);
        assert_eq!(weak[0], "factual_accuracy");
    }

    #[test]
    fn find_weak_dimensions_target_above_threshold() {
        let score = make_score(0.8);
        let weak = find_weak_dimensions(&score, 0.5, Some("factual_accuracy"));
        assert!(weak.is_empty());
    }

    // ---- summarize_cycle ----

    #[test]
    fn summarize_cycle_incomplete() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.5),
            proposed_changes: Vec::new(),
            post_score: None,
            regressions: Vec::new(),
            decision: None,
            final_phase: ImprovementPhase::Analyze,
            weak_dimensions: Vec::new(),
            target_dimension: None,
        };
        let summary = summarize_cycle(&cycle);
        assert!(summary.contains("Baseline"));
        assert!(summary.contains("INCOMPLETE"));
        assert!(summary.contains("analyze"));
    }

    #[test]
    fn summarize_cycle_commit() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.5),
            proposed_changes: Vec::new(),
            post_score: Some(make_score(0.6)),
            regressions: Vec::new(),
            decision: Some(ImprovementDecision::Commit {
                net_improvement: 0.1,
            }),
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            target_dimension: None,
        };
        let summary = summarize_cycle(&cycle);
        assert!(summary.contains("COMMIT"));
        assert!(summary.contains("Post-change"));
    }

    #[test]
    fn summarize_cycle_revert() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.5),
            proposed_changes: Vec::new(),
            post_score: Some(make_score(0.51)),
            regressions: Vec::new(),
            decision: Some(ImprovementDecision::Revert {
                reason: "too small".into(),
            }),
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            target_dimension: None,
        };
        let summary = summarize_cycle(&cycle);
        assert!(summary.contains("REVERT"));
        assert!(summary.contains("too small"));
    }

    #[test]
    fn summarize_cycle_with_target_dimension() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.5),
            proposed_changes: Vec::new(),
            post_score: None,
            regressions: Vec::new(),
            decision: None,
            final_phase: ImprovementPhase::Eval,
            weak_dimensions: Vec::new(),
            target_dimension: Some("specificity".into()),
        };
        let summary = summarize_cycle(&cycle);
        assert!(summary.contains("Target dimension: specificity"));
    }

    #[test]
    fn summarize_cycle_with_regressions() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.5),
            proposed_changes: Vec::new(),
            post_score: Some(make_score(0.6)),
            regressions: vec![
                Regression {
                    dimension: "x".into(),
                    baseline_score: 0.5,
                    current_score: 0.4,
                    delta: -0.1,
                    severity: RegressionSeverity::Severe,
                },
                Regression {
                    dimension: "y".into(),
                    baseline_score: 0.5,
                    current_score: 0.48,
                    delta: -0.02,
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
}
