use super::cycle::*;
use super::types::{ImprovementConfig, ImprovementCycle, ImprovementDecision, ImprovementPhase};
use crate::gym_bridge::ScoreDimensions;
use crate::gym_scoring::{GymSuiteScore, Regression, RegressionSeverity};

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
    };
    let summary = summarize_cycle(&cycle);
    assert!(summary.contains("source_attribution (25.0% deficit)"));
}

#[test]
fn find_weak_dimensions_all_below_preserves_deficit_sort() {
    // All five dimensions below threshold — verify sorting by deficit descending.
    let score = GymSuiteScore {
        suite_id: "test".to_string(),
        overall: 0.30,
        dimensions: ScoreDimensions {
            factual_accuracy: 0.50,       // deficit 0.10
            specificity: 0.45,            // deficit 0.15
            temporal_awareness: 0.40,     // deficit 0.20
            source_attribution: 0.30,     // deficit 0.30
            confidence_calibration: 0.55, // deficit 0.05
        },
        scenario_count: 6,
        scenarios_passed: 6,
        pass_rate: 1.0,
        recorded_at_unix_ms: None,
    };
    let weak = find_weak_dimensions(&score, 0.60, None);
    assert_eq!(weak.len(), 5);
    // Verify strictly descending deficit order
    for window in weak.windows(2) {
        assert!(
            window[0].deficit >= window[1].deficit,
            "{} deficit ({}) should be >= {} deficit ({})",
            window[0].name,
            window[0].deficit,
            window[1].name,
            window[1].deficit,
        );
    }
    assert_eq!(weak[0].name, "source_attribution");
    assert_eq!(weak[4].name, "confidence_calibration");
}

#[test]
fn find_weak_dimensions_at_exact_threshold_not_weak() {
    // Dimensions exactly at threshold should NOT be flagged as weak.
    let score = GymSuiteScore {
        suite_id: "test".to_string(),
        overall: 0.60,
        dimensions: ScoreDimensions {
            factual_accuracy: 0.60,
            specificity: 0.60,
            temporal_awareness: 0.60,
            source_attribution: 0.60,
            confidence_calibration: 0.60,
        },
        scenario_count: 6,
        scenarios_passed: 6,
        pass_rate: 1.0,
        recorded_at_unix_ms: None,
    };
    let weak = find_weak_dimensions(&score, 0.60, None);
    assert!(
        weak.is_empty(),
        "dimensions at exact threshold should not be weak"
    );
}

#[test]
fn summarize_cycle_multiple_weak_dimension_details() {
    use super::types::WeakDimension;
    let cycle = ImprovementCycle {
        baseline: make_score(0.40),
        proposed_changes: vec![],
        post_score: None,
        regressions: vec![],
        decision: None,
        final_phase: ImprovementPhase::Analyze,
        weak_dimensions: vec!["source_attribution".into(), "specificity".into()],
        weak_dimension_details: vec![
            WeakDimension {
                name: "source_attribution".into(),
                deficit: 0.30,
            },
            WeakDimension {
                name: "specificity".into(),
                deficit: 0.15,
            },
        ],
        target_dimension: None,
    };
    let summary = summarize_cycle(&cycle);
    assert!(summary.contains("source_attribution (30.0% deficit)"));
    assert!(summary.contains("specificity (15.0% deficit)"));
}
