use super::cycle::*;
use super::types::{ImprovementConfig, ImprovementCycle, ImprovementDecision, ImprovementPhase};
use crate::gym_bridge::ScoreDimensions;
use crate::gym_scoring::{GymSuiteScore, Regression, RegressionSeverity};

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
        weak_dimension_details: Vec::new(),
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
        weak_dimension_details: Vec::new(),
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
        weak_dimension_details: Vec::new(),
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
        weak_dimension_details: Vec::new(),
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
    // Sorted by deficit: source_attribution (0.2 deficit) before specificity (0.1 deficit)
    assert_eq!(weak[0].name, "source_attribution");
    assert!((weak[0].deficit - 0.2).abs() < 1e-9);
    assert_eq!(weak[1].name, "specificity");
    assert!((weak[1].deficit - 0.1).abs() < 1e-9);
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
        weak_dimension_details: Vec::new(),
        target_dimension: None,
    };
    let summary = summarize_cycle(&cycle);
    assert!(summary.contains("Regressions: 2 total (1 severe)"));
}

fn make_score(v: f64) -> GymSuiteScore {
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
    assert_eq!(weak.len(), 1);
    assert_eq!(weak[0].name, "specificity");
    assert!((weak[0].deficit - 0.1).abs() < 1e-9);

    // Target factual_accuracy (strong) — should return empty
    let weak = find_weak_dimensions(&score, 0.6, Some("factual_accuracy"));
    assert!(weak.is_empty());

    // Target source_attribution (weak) — should return it
    let weak = find_weak_dimensions(&score, 0.6, Some("source_attribution"));
    assert_eq!(weak.len(), 1);
    assert_eq!(weak[0].name, "source_attribution");
    assert!((weak[0].deficit - 0.2).abs() < 1e-9);
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
        weak_dimension_details: Vec::new(),
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
        weak_dimension_details: Vec::new(),
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
