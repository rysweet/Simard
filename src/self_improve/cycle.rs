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

    // Phase 2: Analyze — identify weak dimensions (sorted by deficit)
    let weak_details = find_weak_dimensions(
        &baseline,
        config.weak_threshold,
        config.target_dimension.as_deref(),
    );
    let weak_names: Vec<String> = weak_details.iter().map(|w| w.name.clone()).collect();

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
                    if weak_names.is_empty() {
                        "none".to_string()
                    } else {
                        weak_names.join(", ")
                    }
                ),
            }),
            final_phase: ImprovementPhase::Analyze,
            weak_dimensions: weak_names,
            weak_dimension_details: weak_details,
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
        weak_dimensions: weak_names,
        weak_dimension_details: weak_details,
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
///
/// Results are sorted by deficit (largest first) so callers can prioritize
/// the weakest dimension for improvement.
pub(super) fn find_weak_dimensions(
    score: &GymSuiteScore,
    weak_threshold: f64,
    target: Option<&str>,
) -> Vec<super::types::WeakDimension> {
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
            weak.push(super::types::WeakDimension {
                name: name.to_string(),
                deficit: weak_threshold - value,
            });
        }
    }
    weak.sort_by(|a, b| {
        b.deficit
            .partial_cmp(&a.deficit)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    weak
}

/// Summary of an improvement cycle suitable for persistence or display.
pub fn summarize_cycle(cycle: &ImprovementCycle) -> String {
    let mut lines = Vec::new();

    if let Some(ref dim) = cycle.target_dimension {
        lines.push(format!("Target dimension: {dim}"));
    }

    if !cycle.weak_dimension_details.is_empty() {
        let detail: Vec<String> = cycle
            .weak_dimension_details
            .iter()
            .map(|w| format!("{} ({:.1}% deficit)", w.name, w.deficit * 100.0))
            .collect();
        lines.push(format!("Weak dimensions: {}", detail.join(", ")));
    } else if !cycle.weak_dimensions.is_empty() {
        lines.push(format!(
            "Weak dimensions: {}",
            cycle.weak_dimensions.join(", ")
        ));
    }

    if !cycle.proposed_changes.is_empty() {
        lines.push(format!(
            "Proposed changes: {}",
            cycle.proposed_changes.len(),
        ));
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

    fn make_score(overall: f64, dims: ScoreDimensions) -> GymSuiteScore {
        GymSuiteScore {
            suite_id: "test-suite".to_string(),
            overall,
            dimensions: dims,
            scenario_count: 5,
            scenarios_passed: 5,
            pass_rate: 1.0,
            recorded_at_unix_ms: None,
        }
    }

    fn dims(fa: f64, sp: f64, ta: f64, sa: f64, cc: f64) -> ScoreDimensions {
        ScoreDimensions {
            factual_accuracy: fa,
            specificity: sp,
            temporal_awareness: ta,
            source_attribution: sa,
            confidence_calibration: cc,
        }
    }

    #[test]
    fn decide_commit_when_net_positive() {
        let config = ImprovementConfig {
            min_net_improvement: 0.01,
            max_single_regression: 0.1,
            ..ImprovementConfig::default()
        };
        let baseline = make_score(0.70, dims(0.7, 0.7, 0.7, 0.7, 0.7));
        let post = make_score(0.75, dims(0.75, 0.75, 0.75, 0.75, 0.75));
        let decision = decide(&config, &baseline, &post, &[]);
        assert!(matches!(decision, ImprovementDecision::Commit { .. }));
    }

    #[test]
    fn decide_revert_below_threshold() {
        let config = ImprovementConfig {
            min_net_improvement: 0.10,
            max_single_regression: 0.5,
            ..ImprovementConfig::default()
        };
        let baseline = make_score(0.70, dims(0.7, 0.7, 0.7, 0.7, 0.7));
        let post = make_score(0.72, dims(0.72, 0.72, 0.72, 0.72, 0.72));
        let decision = decide(&config, &baseline, &post, &[]);
        assert!(matches!(decision, ImprovementDecision::Revert { .. }));
    }

    #[test]
    fn decide_revert_on_severe_regression() {
        let config = ImprovementConfig {
            min_net_improvement: 0.01,
            max_single_regression: 0.05,
            ..ImprovementConfig::default()
        };
        let baseline = make_score(0.70, dims(0.7, 0.7, 0.7, 0.7, 0.7));
        let post = make_score(0.75, dims(0.8, 0.8, 0.8, 0.8, 0.8));
        let regression = Regression {
            dimension: "specificity".to_string(),
            baseline_score: 0.9,
            current_score: 0.7,
            delta: -0.2,
            severity: RegressionSeverity::Severe,
        };
        let decision = decide(&config, &baseline, &post, &[regression]);
        assert!(matches!(decision, ImprovementDecision::Revert { .. }));
    }

    #[test]
    fn find_weak_dimensions_below_threshold() {
        let score = make_score(0.6, dims(0.3, 0.8, 0.4, 0.9, 0.2));
        let weak = find_weak_dimensions(&score, 0.5, None);
        assert_eq!(weak.len(), 3);
        let names: Vec<&str> = weak.iter().map(|w| w.name.as_str()).collect();
        assert!(names.contains(&"factual_accuracy"));
        assert!(names.contains(&"temporal_awareness"));
        assert!(names.contains(&"confidence_calibration"));
    }

    #[test]
    fn find_weak_dimensions_with_target_filter() {
        let score = make_score(0.6, dims(0.3, 0.8, 0.4, 0.9, 0.2));
        let weak = find_weak_dimensions(&score, 0.5, Some("specificity"));
        assert!(weak.is_empty()); // specificity is 0.8, above threshold
    }

    #[test]
    fn summarize_cycle_baseline_only() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.75, ScoreDimensions::default()),
            proposed_changes: Vec::new(),
            post_score: None,
            regressions: Vec::new(),
            decision: Some(ImprovementDecision::Revert {
                reason: "no changes".to_string(),
            }),
            final_phase: ImprovementPhase::Analyze,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: None,
        };
        let summary = summarize_cycle(&cycle);
        assert!(summary.contains("Baseline:"));
        assert!(summary.contains("REVERT"));
        assert!(!summary.contains("Post-change:"));
    }

    #[test]
    fn summarize_cycle_with_post_score() {
        let cycle = ImprovementCycle {
            baseline: make_score(0.70, ScoreDimensions::default()),
            proposed_changes: vec![],
            post_score: Some(make_score(0.80, ScoreDimensions::default())),
            regressions: Vec::new(),
            decision: Some(ImprovementDecision::Commit {
                net_improvement: 0.10,
            }),
            final_phase: ImprovementPhase::Decide,
            weak_dimensions: Vec::new(),
            weak_dimension_details: Vec::new(),
            target_dimension: Some("specificity".to_string()),
        };
        let summary = summarize_cycle(&cycle);
        assert!(summary.contains("Post-change:"));
        assert!(summary.contains("COMMIT"));
        assert!(summary.contains("Target dimension: specificity"));
    }
}
