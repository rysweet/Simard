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
        plateau_dimensions: Vec::new(),
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
        plateau_dimensions: Vec::new(),
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
        plateau_dimensions: Vec::new(),
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
        plateau_dimensions: Vec::new(),
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
        plateau_dimensions: Vec::new(),
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
        plateau_dimensions: Vec::new(),
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
        plateau_dimensions: Vec::new(),
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
        weak_dimension_details: Vec::new(),
        target_dimension: None,
        plateau_dimensions: Vec::new(),
    };
    let summary = summarize_cycle(&cycle);
    assert!(summary.contains("REVERT"));
    assert!(summary.contains("-10.0%"));
}

#[test]
fn find_weak_dimensions_sorted_by_deficit() {
    let score = GymSuiteScore {
        suite_id: "test".to_string(),
        overall: 0.65,
        dimensions: ScoreDimensions {
            factual_accuracy: 0.55,       // deficit 0.05
            specificity: 0.50,            // deficit 0.10
            temporal_awareness: 0.70,     // above threshold
            source_attribution: 0.30,     // deficit 0.30 (largest)
            confidence_calibration: 0.58, // deficit 0.02
        },
        scenario_count: 6,
        scenarios_passed: 6,
        pass_rate: 1.0,
        recorded_at_unix_ms: None,
    };
    let weak = find_weak_dimensions(&score, 0.6, None);
    assert_eq!(weak.len(), 4);
    // Sorted by deficit descending
    assert_eq!(weak[0].name, "source_attribution");
    assert!((weak[0].deficit - 0.30).abs() < 1e-9);
    assert_eq!(weak[1].name, "specificity");
    assert_eq!(weak[2].name, "factual_accuracy");
    assert_eq!(weak[3].name, "confidence_calibration");
    assert!(weak[0].deficit >= weak[1].deficit);
    assert!(weak[1].deficit >= weak[2].deficit);
    assert!(weak[2].deficit >= weak[3].deficit);
}

#[test]
fn summarize_cycle_shows_deficit_when_details_present() {
    use super::types::WeakDimension;
    let cycle = ImprovementCycle {
        baseline: make_score(0.50),
        proposed_changes: vec![],
        post_score: None,
        regressions: vec![],
        decision: None,
        final_phase: ImprovementPhase::Analyze,
        weak_dimensions: vec!["source_attribution".into()],
        weak_dimension_details: vec![WeakDimension {
            name: "source_attribution".into(),
            deficit: 0.25,
        }],
        target_dimension: None,
        plateau_dimensions: Vec::new(),
    };
    let summary = summarize_cycle(&cycle);
    assert!(summary.contains("source_attribution (25.0% deficit)"));
}

#[test]
fn validate_rejects_unknown_target_dimension() {
    let cfg = ImprovementConfig {
        target_dimension: Some("specficity_typo".into()),
        ..Default::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(format!("{err:?}").contains("target_dimension"));
    assert!(format!("{err:?}").contains("specficity_typo"));
}

#[test]
fn validate_accepts_known_target_dimension() {
    let cfg = ImprovementConfig {
        target_dimension: Some("specificity".into()),
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn summarize_cycle_shows_plateau_dimensions() {
    let cycle = ImprovementCycle {
        baseline: make_score(0.50),
        proposed_changes: vec![],
        post_score: None,
        regressions: vec![],
        decision: None,
        final_phase: ImprovementPhase::Analyze,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
        plateau_dimensions: vec!["source_attribution".into(), "temporal_awareness".into()],
    };
    let summary = summarize_cycle(&cycle);
    assert!(
        summary.contains("Plateau dimensions (stalled)"),
        "summary should mention plateau dimensions: {summary}"
    );
    assert!(summary.contains("source_attribution"));
    assert!(summary.contains("temporal_awareness"));
}

#[test]
fn dimension_deltas_with_post_score() {
    let cycle = ImprovementCycle {
        baseline: make_score(0.50),
        proposed_changes: vec![],
        post_score: Some(make_score(0.70)),
        regressions: vec![],
        decision: Some(ImprovementDecision::Commit {
            net_improvement: 0.20,
        }),
        final_phase: ImprovementPhase::Decide,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
        plateau_dimensions: Vec::new(),
    };
    let deltas = cycle.dimension_deltas();
    assert_eq!(deltas.len(), 5);
    // factual_accuracy: 0.70 - 0.50 = 0.20
    let fa = deltas
        .iter()
        .find(|(n, _)| n == "factual_accuracy")
        .unwrap();
    assert!((fa.1 - 0.20).abs() < 1e-9);
    // source_attribution: 0.49 - 0.35 = 0.14
    let sa = deltas
        .iter()
        .find(|(n, _)| n == "source_attribution")
        .unwrap();
    assert!((sa.1 - 0.14).abs() < 1e-9);
    // sorted by delta descending
    for i in 0..deltas.len() - 1 {
        assert!(
            deltas[i].1 >= deltas[i + 1].1,
            "deltas should be sorted descending"
        );
    }
}

#[test]
fn dimension_deltas_empty_without_post_score() {
    let cycle = ImprovementCycle {
        baseline: make_score(0.50),
        proposed_changes: vec![],
        post_score: None,
        regressions: vec![],
        decision: None,
        final_phase: ImprovementPhase::Analyze,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
        plateau_dimensions: Vec::new(),
    };
    let deltas = cycle.dimension_deltas();
    assert!(deltas.is_empty());
}

#[test]
fn summarize_cycle_shows_dimension_breakdown() {
    let cycle = ImprovementCycle {
        baseline: make_score(0.50),
        proposed_changes: vec![],
        post_score: Some(make_score(0.70)),
        regressions: vec![],
        decision: Some(ImprovementDecision::Commit {
            net_improvement: 0.20,
        }),
        final_phase: ImprovementPhase::Decide,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
        plateau_dimensions: Vec::new(),
    };
    let summary = summarize_cycle(&cycle);
    assert!(
        summary.contains("Dimensions:"),
        "summary should contain per-dimension breakdown: {summary}"
    );
    assert!(summary.contains("factual_accuracy: +20.0%"));
}

#[test]
fn decide_reverts_when_target_dimension_regressed() {
    let mut cfg = ImprovementConfig::default();
    cfg.target_dimension = Some("specificity".into());
    let baseline = GymSuiteScore {
        suite_id: "test".into(),
        overall: 0.60,
        dimensions: ScoreDimensions {
            factual_accuracy: 0.60,
            specificity: 0.60,
            temporal_awareness: 0.60,
            source_attribution: 0.60,
            confidence_calibration: 0.60,
        },
        scenario_count: 4,
        scenarios_passed: 4,
        pass_rate: 1.0,
        recorded_at_unix_ms: None,
    };
    let post = GymSuiteScore {
        suite_id: "test".into(),
        overall: 0.70,
        dimensions: ScoreDimensions {
            factual_accuracy: 0.80,
            specificity: 0.55, // target regressed
            temporal_awareness: 0.75,
            source_attribution: 0.70,
            confidence_calibration: 0.70,
        },
        scenario_count: 4,
        scenarios_passed: 4,
        pass_rate: 1.0,
        recorded_at_unix_ms: None,
    };
    let d = decide(&cfg, &baseline, &post, &[]);
    match d {
        ImprovementDecision::Revert { reason } => {
            assert!(reason.contains("target dimension"));
            assert!(reason.contains("specificity"));
        }
        ImprovementDecision::Commit { .. } => {
            panic!("expected revert when target dimension regressed")
        }
    }
}

#[test]
fn decide_commits_when_target_dimension_improved() {
    let mut cfg = ImprovementConfig::default();
    cfg.target_dimension = Some("specificity".into());
    let baseline = make_score(0.50);
    let post = make_score(0.60);
    let d = decide(&cfg, &baseline, &post, &[]);
    assert!(
        matches!(d, ImprovementDecision::Commit { .. }),
        "expected commit when target dimension improved"
    );
}

#[test]
fn summarize_cycle_target_dimension_shows_delta() {
    let cycle = ImprovementCycle {
        baseline: make_score(0.50),
        proposed_changes: vec![],
        post_score: Some(make_score(0.60)),
        regressions: vec![],
        decision: Some(ImprovementDecision::Commit {
            net_improvement: 0.1,
        }),
        final_phase: ImprovementPhase::Decide,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: Some("factual_accuracy".into()),
        plateau_dimensions: Vec::new(),
    };
    let summary = summarize_cycle(&cycle);
    assert!(
        summary.contains("Target dimension: factual_accuracy (+"),
        "summary should show target dimension delta: {summary}"
    );
}

#[test]
fn summarize_cycle_target_dimension_no_post_score() {
    let cycle = ImprovementCycle {
        baseline: make_score(0.50),
        proposed_changes: vec![],
        post_score: None,
        regressions: vec![],
        decision: None,
        final_phase: ImprovementPhase::Analyze,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: Some("specificity".into()),
        plateau_dimensions: Vec::new(),
    };
    let summary = summarize_cycle(&cycle);
    // Without post_score, should show name only (no delta)
    assert!(
        summary.contains("Target dimension: specificity"),
        "summary should show target dimension name: {summary}"
    );
    assert!(
        !summary.contains("(+") && !summary.contains("(-"),
        "summary should NOT show delta without post_score: {summary}"
    );
}

// ---- CycleHistory / ConvergenceStatus tests ----

use super::types::{ConvergenceStatus, CycleHistory};

fn make_cycle_with_net(baseline_overall: f64, post_overall: f64, commit: bool) -> ImprovementCycle {
    let net = post_overall - baseline_overall;
    ImprovementCycle {
        baseline: make_score(baseline_overall),
        proposed_changes: Vec::new(),
        post_score: Some(make_score(post_overall)),
        regressions: Vec::new(),
        decision: Some(if commit {
            ImprovementDecision::Commit {
                net_improvement: net,
            }
        } else {
            ImprovementDecision::Revert {
                reason: "test".into(),
            }
        }),
        final_phase: ImprovementPhase::Decide,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
        plateau_dimensions: Vec::new(),
    }
}

#[test]
fn cycle_history_empty_velocity_is_zero() {
    let h = CycleHistory::new();
    assert!(h.is_empty());
    assert_eq!(h.len(), 0);
    assert!((h.overall_velocity()).abs() < 1e-9);
}

#[test]
fn cycle_history_single_cycle_velocity_is_zero() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.5, 0.6, true));
    assert_eq!(h.len(), 1);
    assert!((h.overall_velocity()).abs() < 1e-9);
}

#[test]
fn cycle_history_velocity_positive_when_improving() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.50, 0.55, true));
    h.push(make_cycle_with_net(0.55, 0.60, true));
    h.push(make_cycle_with_net(0.60, 0.65, true));
    let vel = h.overall_velocity();
    assert!(vel > 0.0, "velocity should be positive, got {vel}");
    assert!(h.is_converging(0.001));
}

#[test]
fn cycle_history_velocity_negative_when_diverging() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.70, 0.68, false));
    h.push(make_cycle_with_net(0.68, 0.65, false));
    let vel = h.overall_velocity();
    assert!(vel < 0.0, "velocity should be negative, got {vel}");
    assert!(!h.is_converging(0.001));
}

#[test]
fn diminishing_returns_detected() {
    let mut h = CycleHistory::new();
    // Each committed improvement is smaller than the last
    h.push(make_cycle_with_net(0.50, 0.60, true)); // +0.10
    h.push(make_cycle_with_net(0.60, 0.67, true)); // +0.07
    h.push(make_cycle_with_net(0.67, 0.70, true)); // +0.03
    assert!(h.diminishing_returns(3));
}

#[test]
fn diminishing_returns_not_detected_when_gains_increase() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.50, 0.53, true)); // +0.03
    h.push(make_cycle_with_net(0.53, 0.58, true)); // +0.05
    h.push(make_cycle_with_net(0.58, 0.66, true)); // +0.08
    assert!(!h.diminishing_returns(3));
}

#[test]
fn diminishing_returns_requires_minimum_window() {
    let h = CycleHistory::new();
    assert!(!h.diminishing_returns(2));
    assert!(!h.diminishing_returns(1));
    assert!(!h.diminishing_returns(0));
}

#[test]
fn evaluate_convergence_improving() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.50, 0.55, true));
    h.push(make_cycle_with_net(0.55, 0.62, true));
    h.push(make_cycle_with_net(0.62, 0.70, true));
    assert_eq!(
        h.evaluate_convergence(0.005, 3),
        ConvergenceStatus::Improving
    );
}

#[test]
fn evaluate_convergence_diverging() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.70, 0.68, false));
    h.push(make_cycle_with_net(0.68, 0.64, false));
    assert_eq!(
        h.evaluate_convergence(0.005, 3),
        ConvergenceStatus::Diverging
    );
}

#[test]
fn evaluate_convergence_plateau() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.65, 0.651, false));
    h.push(make_cycle_with_net(0.651, 0.652, false));
    assert_eq!(h.evaluate_convergence(0.005, 3), ConvergenceStatus::Plateau);
}

#[test]
fn evaluate_convergence_diminishing_returns() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.50, 0.60, true)); // +0.10
    h.push(make_cycle_with_net(0.60, 0.67, true)); // +0.07
    h.push(make_cycle_with_net(0.67, 0.70, true)); // +0.03
    assert_eq!(
        h.evaluate_convergence(0.005, 3),
        ConvergenceStatus::DiminishingReturns
    );
}

#[test]
fn evaluate_convergence_single_cycle_improving() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.50, 0.60, true));
    // Fewer than 2 cycles -> defaults to Improving
    assert_eq!(
        h.evaluate_convergence(0.005, 3),
        ConvergenceStatus::Improving
    );
}

#[test]
fn convergence_status_display() {
    assert_eq!(ConvergenceStatus::Improving.to_string(), "improving");
    assert_eq!(ConvergenceStatus::Plateau.to_string(), "plateau");
    assert_eq!(
        ConvergenceStatus::DiminishingReturns.to_string(),
        "diminishing-returns"
    );
    assert_eq!(ConvergenceStatus::Diverging.to_string(), "diverging");
}

#[test]
fn serde_round_trip_improvement_config() {
    let config = ImprovementConfig {
        suite_id: "progressive".into(),
        min_net_improvement: 0.03,
        max_single_regression: 0.04,
        proposed_changes: Vec::new(),
        auto_apply: true,
        weak_threshold: 0.55,
        target_dimension: Some("specificity".into()),
        max_cycles: Some(5),
    };
    let json = serde_json::to_string(&config).expect("serialize config");
    let deserialized: ImprovementConfig = serde_json::from_str(&json).expect("deserialize config");
    assert_eq!(deserialized.suite_id, config.suite_id);
    assert!((deserialized.min_net_improvement - config.min_net_improvement).abs() < 1e-9);
    assert!((deserialized.weak_threshold - config.weak_threshold).abs() < 1e-9);
    assert_eq!(deserialized.target_dimension, config.target_dimension);
    assert_eq!(deserialized.max_cycles, config.max_cycles);
    assert_eq!(deserialized.auto_apply, config.auto_apply);
}

#[test]
fn serde_round_trip_cycle_history() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.50, 0.60, true));
    h.push(make_cycle_with_net(0.60, 0.65, true));
    let json = serde_json::to_string(&h).expect("serialize history");
    let deserialized: CycleHistory = serde_json::from_str(&json).expect("deserialize history");
    assert_eq!(deserialized.len(), 2);
    let vel_diff = (deserialized.overall_velocity() - h.overall_velocity()).abs();
    assert!(vel_diff < 1e-9);
}

// ---- CycleHistory::last_committed ----

#[test]
fn last_committed_empty_history_returns_none() {
    let h = CycleHistory::new();
    assert!(h.last_committed().is_none());
}

#[test]
fn last_committed_returns_most_recent_commit() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.50, 0.60, true)); // commit, net +0.10
    h.push(make_cycle_with_net(0.60, 0.67, true)); // commit, net +0.07
    h.push(make_cycle_with_net(0.67, 0.68, false)); // revert
    let last = h.last_committed().expect("should find a committed cycle");
    // The most recent committed cycle has baseline 0.60 -> post 0.67
    let net = match &last.decision {
        Some(ImprovementDecision::Commit { net_improvement }) => *net_improvement,
        _ => panic!("expected commit"),
    };
    assert!((net - 0.07).abs() < 1e-9, "expected net +0.07, got {net}");
}

#[test]
fn last_committed_none_when_all_reverted() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.60, 0.61, false));
    h.push(make_cycle_with_net(0.61, 0.62, false));
    assert!(h.last_committed().is_none());
}

// ---- CycleHistory::commit_rate ----

#[test]
fn commit_rate_empty_history_is_zero() {
    let h = CycleHistory::new();
    assert!((h.commit_rate() - 0.0).abs() < 1e-9);
}

#[test]
fn commit_rate_all_committed() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.50, 0.60, true));
    h.push(make_cycle_with_net(0.60, 0.65, true));
    assert!((h.commit_rate() - 1.0).abs() < 1e-9);
}

#[test]
fn commit_rate_none_committed() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.60, 0.61, false));
    h.push(make_cycle_with_net(0.61, 0.62, false));
    assert!((h.commit_rate() - 0.0).abs() < 1e-9);
}

#[test]
fn commit_rate_partial() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.50, 0.60, true)); // commit
    h.push(make_cycle_with_net(0.60, 0.61, false)); // revert
    h.push(make_cycle_with_net(0.61, 0.65, true)); // commit
    h.push(make_cycle_with_net(0.65, 0.66, false)); // revert
    // 2 of 4 committed
    assert!((h.commit_rate() - 0.5).abs() < 1e-9);
}

// ---- CycleHistory::best_cycle ----

#[test]
fn best_cycle_empty_history_returns_none() {
    let h = CycleHistory::new();
    assert!(h.best_cycle().is_none());
}

#[test]
fn best_cycle_returns_highest_net_improvement() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.50, 0.55, true)); // +0.05
    h.push(make_cycle_with_net(0.55, 0.65, true)); // +0.10 (best)
    h.push(make_cycle_with_net(0.65, 0.68, true)); // +0.03
    let best = h.best_cycle().expect("should have a best cycle");
    let net = match &best.decision {
        Some(ImprovementDecision::Commit { net_improvement }) => *net_improvement,
        _ => panic!("expected commit"),
    };
    assert!(
        (net - 0.10).abs() < 1e-9,
        "expected best net +0.10, got {net}"
    );
}

#[test]
fn best_cycle_ignores_reverts() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.50, 0.80, false)); // large apparent gain but reverted
    h.push(make_cycle_with_net(0.50, 0.53, true)); // small commit — only committed
    let best = h.best_cycle().expect("should find committed cycle");
    let net = match &best.decision {
        Some(ImprovementDecision::Commit { net_improvement }) => *net_improvement,
        _ => panic!("expected commit"),
    };
    assert!((net - 0.03).abs() < 1e-9);
}

#[test]
fn best_cycle_none_when_all_reverted() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.60, 0.70, false));
    assert!(h.best_cycle().is_none());
}

// ---- ImprovementConfig::validate — upper-bound checks ----

#[test]
fn validate_min_net_improvement_above_one_rejected() {
    let cfg = ImprovementConfig {
        min_net_improvement: 1.01,
        ..Default::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(
        format!("{err:?}").contains("min_net_improvement"),
        "error should mention field: {err:?}"
    );
}

#[test]
fn validate_max_single_regression_above_one_rejected() {
    let cfg = ImprovementConfig {
        max_single_regression: 1.5,
        ..Default::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(
        format!("{err:?}").contains("max_single_regression"),
        "error should mention field: {err:?}"
    );
}

#[test]
fn validate_min_net_improvement_exactly_one_accepted() {
    let cfg = ImprovementConfig {
        min_net_improvement: 1.0,
        ..Default::default()
    };
    assert!(cfg.validate().is_ok(), "1.0 is a valid fraction");
}

#[test]
fn validate_max_single_regression_exactly_one_accepted() {
    let cfg = ImprovementConfig {
        max_single_regression: 1.0,
        ..Default::default()
    };
    assert!(cfg.validate().is_ok(), "1.0 is a valid fraction");
}

// ---- CycleHistory::overall_velocity with missing post_score ----

#[test]
fn cycle_history_velocity_uses_baseline_when_no_post_score() {
    // When a cycle has no post_score (aborted before ReEval), overall_velocity
    // should fall back to that cycle's baseline for the "last" value.
    let mut h = CycleHistory::new();
    // First cycle: baseline 0.50 -> post 0.55 (committed)
    h.push(make_cycle_with_net(0.50, 0.55, true));
    // Second cycle: no post_score — create manually
    let no_post = ImprovementCycle {
        baseline: make_score(0.55),
        proposed_changes: Vec::new(),
        post_score: None,
        regressions: Vec::new(),
        decision: Some(ImprovementDecision::Revert {
            reason: "aborted".into(),
        }),
        final_phase: ImprovementPhase::Analyze,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
        plateau_dimensions: Vec::new(),
    };
    h.push(no_post);

    // overall_velocity: (last_effective_score - first_baseline) / intervals
    // last effective = no_post.baseline.overall = 0.55 (no post_score fallback)
    // first = first cycle baseline = 0.50
    // intervals = 1
    // velocity = (0.55 - 0.50) / 1 = 0.05
    let vel = h.overall_velocity();
    assert!(
        vel.is_finite(),
        "velocity should be finite when last cycle has no post_score"
    );
    assert!(
        vel >= 0.0,
        "velocity should be non-negative when scores are flat or improving"
    );
}

// ---- ImprovementConfig serde backward compat for max_cycles ----

#[test]
fn improvement_config_deserialize_without_max_cycles() {
    // Older serialized configs lack max_cycles; Default provides None.
    let json = r#"{
        "suite_id": "progressive",
        "min_net_improvement": 0.02,
        "max_single_regression": 0.05,
        "proposed_changes": [],
        "auto_apply": false,
        "weak_threshold": 0.6
    }"#;
    let cfg: ImprovementConfig =
        serde_json::from_str(json).expect("should deserialize config without max_cycles");
    assert!(
        cfg.max_cycles.is_none(),
        "missing max_cycles should default to None"
    );
    assert!(cfg.validate().is_ok());
}

// ---- CycleHistory::baselines() ----

#[test]
fn cycle_history_baselines_empty() {
    let h = CycleHistory::new();
    assert!(h.baselines().is_empty());
}

#[test]
fn cycle_history_baselines_returns_all_in_order() {
    let mut h = CycleHistory::new();
    h.push(make_cycle_with_net(0.50, 0.55, true));
    h.push(make_cycle_with_net(0.55, 0.62, true));
    h.push(make_cycle_with_net(0.62, 0.68, false));
    let baselines = h.baselines();
    assert_eq!(baselines.len(), 3);
    assert!((baselines[0].overall - 0.50).abs() < 1e-9);
    assert!((baselines[1].overall - 0.55).abs() < 1e-9);
    assert!((baselines[2].overall - 0.62).abs() < 1e-9);
}

#[test]
fn cycle_history_baselines_count_matches_len() {
    let mut h = CycleHistory::new();
    for i in 0..5 {
        h.push(make_cycle_with_net(
            0.50 + i as f64 * 0.05,
            0.55 + i as f64 * 0.05,
            true,
        ));
    }
    assert_eq!(h.baselines().len(), h.len());
}

// ---- ImprovementCycle::enrich_from_history ----

#[test]
fn enrich_from_history_populates_plateau_dimensions() {
    let mut cycle = ImprovementCycle {
        baseline: make_score(0.5),
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
    let mut history = CycleHistory::new();
    for _ in 0..4 {
        history.push(make_cycle_with_net(0.5, 0.5, false));
    }
    cycle.enrich_from_history(0.6, &history);
    assert!(
        !cycle.plateau_dimensions.is_empty(),
        "enrich_from_history should populate plateau_dimensions from history"
    );
    assert!(
        cycle
            .plateau_dimensions
            .contains(&"source_attribution".to_string()),
        "source_attribution should be plateaued"
    );
}

#[test]
fn enrich_from_history_empty_history_leaves_plateau_empty() {
    let mut cycle = ImprovementCycle {
        baseline: make_score(0.5),
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
    let empty_history = CycleHistory::new();
    cycle.enrich_from_history(0.6, &empty_history);
    assert!(
        cycle.plateau_dimensions.is_empty(),
        "empty history should leave plateau_dimensions empty"
    );
}

#[test]
fn enrich_from_history_replaces_stale_plateau_dimensions() {
    let mut cycle = ImprovementCycle {
        baseline: make_score(0.9), // all dimensions strong
        proposed_changes: Vec::new(),
        post_score: None,
        regressions: Vec::new(),
        decision: None,
        final_phase: ImprovementPhase::Analyze,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
        plateau_dimensions: vec!["stale_entry".to_string()],
    };
    let mut history = CycleHistory::new();
    // Past with strong scores — no plateaus expected
    for _ in 0..4 {
        history.push(make_cycle_with_net(0.9, 0.9, true));
    }
    cycle.enrich_from_history(0.6, &history);
    assert!(
        !cycle
            .plateau_dimensions
            .contains(&"stale_entry".to_string()),
        "enrich_from_history should replace, not accumulate, plateau_dimensions"
    );
    assert!(
        cycle.plateau_dimensions.is_empty(),
        "all-strong history should yield no plateau dimensions"
    );
}

#[test]
fn enrich_from_history_and_enrich_with_history_agree() {
    let mut cycle_a = ImprovementCycle {
        baseline: make_score(0.5),
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
    let mut cycle_b = cycle_a.clone();

    let mut history = CycleHistory::new();
    for _ in 0..4 {
        history.push(make_cycle_with_net(0.5, 0.5, false));
    }
    let baselines = history.baselines();

    cycle_a.enrich_from_history(0.6, &history);
    cycle_b.enrich_with_history(0.6, &baselines);

    assert_eq!(
        cycle_a.plateau_dimensions, cycle_b.plateau_dimensions,
        "enrich_from_history and enrich_with_history should produce identical results"
    );
}
