use super::cycle::*;
use super::types::{ImprovementCycle, ImprovementDecision, ImprovementPhase, ProposedChange};
use crate::gym_bridge::ScoreDimensions;
use crate::gym_scoring::GymSuiteScore;

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
fn summarize_cycle_lists_proposed_change_count() {
    // Happy path: when proposed_changes is non-empty, summary includes a count line.
    let cycle = ImprovementCycle {
        baseline: make_score(0.70),
        proposed_changes: vec![
            ProposedChange {
                file_path: "src/a.rs".into(),
                description: "tighten error handling".into(),
                expected_impact: "fewer regressions".into(),
            },
            ProposedChange {
                file_path: "src/b.rs".into(),
                description: "expand docs".into(),
                expected_impact: "clearer behaviour".into(),
            },
            ProposedChange {
                file_path: "src/c.rs".into(),
                description: "split helper".into(),
                expected_impact: "easier testing".into(),
            },
        ],
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
    assert!(
        summary.contains("Proposed changes: 3"),
        "summary should include count of proposed changes, got: {summary}"
    );
}

#[test]
fn summarize_cycle_lists_weak_dimensions_without_details() {
    // Edge case: when weak_dimension_details is empty but weak_dimensions is
    // populated (e.g. legacy cycles deserialized from JSON predating the
    // weak_dimension_details field), summary falls back to the names-only line.
    let cycle = ImprovementCycle {
        baseline: make_score(0.50),
        proposed_changes: vec![],
        post_score: None,
        regressions: vec![],
        decision: None,
        final_phase: ImprovementPhase::Analyze,
        weak_dimensions: vec!["specificity".into(), "source_attribution".into()],
        weak_dimension_details: Vec::new(),
        target_dimension: None,
    };
    let summary = summarize_cycle(&cycle);
    assert!(
        summary.contains("Weak dimensions: specificity, source_attribution"),
        "summary should fall back to the names-only weak dimensions line, got: {summary}"
    );
    // Detail format must NOT appear because details are empty.
    assert!(!summary.contains("% deficit"));
}

#[test]
fn summarize_cycle_omits_weak_dimensions_when_both_lists_empty() {
    // Edge case: when both weak_dimension_details AND weak_dimensions are empty,
    // summary must not include any "Weak dimensions:" line at all.
    let cycle = ImprovementCycle {
        baseline: make_score(0.90),
        proposed_changes: vec![],
        post_score: Some(make_score(0.92)),
        regressions: vec![],
        decision: Some(ImprovementDecision::Commit {
            net_improvement: 0.02,
        }),
        final_phase: ImprovementPhase::Decide,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
    };
    let summary = summarize_cycle(&cycle);
    assert!(
        !summary.contains("Weak dimensions"),
        "summary must not include weak dimensions line when none exist, got: {summary}"
    );
}

// Tests previously inlined in src/self_improve/cycle.rs (#1266 burndown)
mod cycle_inline {
    use super::super::cycle::*;
    use super::super::types::{
        ImprovementConfig, ImprovementCycle, ImprovementDecision, ImprovementPhase,
    };
    use crate::gym_bridge::ScoreDimensions;
    use crate::gym_scoring::{GymSuiteScore, Regression, RegressionSeverity};

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
