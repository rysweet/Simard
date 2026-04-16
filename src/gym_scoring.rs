//! Score aggregation, regression detection, and improvement tracking for gym results.

use serde::{Deserialize, Serialize};

use crate::gym_bridge::{GymScenarioResult, GymSuiteResult, ScoreDimensions};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GymSuiteScore {
    pub suite_id: String,
    pub overall: f64,
    pub dimensions: ScoreDimensions,
    pub scenario_count: usize,
    pub scenarios_passed: usize,
    pub pass_rate: f64,
    pub recorded_at_unix_ms: Option<u128>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Regression {
    pub dimension: String,
    pub baseline_score: f64,
    pub current_score: f64,
    pub delta: f64,
    pub severity: RegressionSeverity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegressionSeverity {
    Minor,
    Moderate,
    Severe,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrendDirection {
    Improving,
    Stable,
    Declining,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DimensionTrend {
    pub dimension: String,
    pub direction: TrendDirection,
    pub total_delta: f64,
    pub average: f64,
    pub history: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImprovementTrend {
    pub run_count: usize,
    pub overall_direction: TrendDirection,
    pub overall_delta: f64,
    pub dimension_trends: Vec<DimensionTrend>,
}

/// Aggregate scenario results into a suite-level score. Empty input yields zeroed score.
pub fn aggregate_suite_scores(suite_id: &str, results: &[GymScenarioResult]) -> GymSuiteScore {
    if results.is_empty() {
        return GymSuiteScore {
            suite_id: suite_id.to_string(),
            overall: 0.0,
            dimensions: ScoreDimensions::default(),
            scenario_count: 0,
            scenarios_passed: 0,
            pass_rate: 0.0,
            recorded_at_unix_ms: None,
        };
    }

    let n = results.len() as f64;
    let passed = results.iter().filter(|r| r.success).count();
    let avg = |f: fn(&ScoreDimensions) -> f64| -> f64 {
        results.iter().map(|r| f(&r.dimensions)).sum::<f64>() / n
    };
    let dims = ScoreDimensions {
        factual_accuracy: avg(|d| d.factual_accuracy),
        specificity: avg(|d| d.specificity),
        temporal_awareness: avg(|d| d.temporal_awareness),
        source_attribution: avg(|d| d.source_attribution),
        confidence_calibration: avg(|d| d.confidence_calibration),
    };
    let overall = results.iter().map(|r| r.score).sum::<f64>() / n;

    GymSuiteScore {
        suite_id: suite_id.to_string(),
        overall,
        dimensions: dims,
        scenario_count: results.len(),
        scenarios_passed: passed,
        pass_rate: passed as f64 / n,
        recorded_at_unix_ms: None,
    }
}

/// Build a [`GymSuiteScore`] from a [`GymSuiteResult`], preferring suite-level values.
pub fn suite_score_from_result(result: &GymSuiteResult) -> GymSuiteScore {
    let mut score = aggregate_suite_scores(&result.suite_id, &result.scenario_results);
    // Prefer the suite-level values when present since they may differ from
    // a naive average of scenario results (e.g. weighted scoring).
    score.overall = result.overall_score;
    score.dimensions = result.dimensions.clone();
    score
}

/// Return regressions where a dimension dropped by more than 0.01 vs baseline.
pub fn detect_regression(current: &GymSuiteScore, baseline: &GymSuiteScore) -> Vec<Regression> {
    const THRESHOLD: f64 = 0.01;
    let c = &current.dimensions;
    let b = &baseline.dimensions;
    let pairs: [(&str, f64, f64); 6] = [
        ("factual_accuracy", c.factual_accuracy, b.factual_accuracy),
        ("specificity", c.specificity, b.specificity),
        (
            "temporal_awareness",
            c.temporal_awareness,
            b.temporal_awareness,
        ),
        (
            "source_attribution",
            c.source_attribution,
            b.source_attribution,
        ),
        (
            "confidence_calibration",
            c.confidence_calibration,
            b.confidence_calibration,
        ),
        ("overall", current.overall, baseline.overall),
    ];

    pairs
        .into_iter()
        .filter_map(|(name, curr, base)| {
            let delta = curr - base;
            if delta < -THRESHOLD {
                let severity = if delta.abs() > 0.15 {
                    RegressionSeverity::Severe
                } else if delta.abs() > 0.05 {
                    RegressionSeverity::Moderate
                } else {
                    RegressionSeverity::Minor
                };
                Some(Regression {
                    dimension: name.to_string(),
                    baseline_score: base,
                    current_score: curr,
                    delta,
                    severity,
                })
            } else {
                None
            }
        })
        .collect()
}

/// Analyze a chronological series of suite scores. Requires >= 2 entries for a trend.
pub fn track_improvement(history: &[GymSuiteScore]) -> ImprovementTrend {
    if history.len() < 2 {
        return ImprovementTrend {
            run_count: history.len(),
            overall_direction: TrendDirection::Stable,
            overall_delta: 0.0,
            dimension_trends: Vec::new(),
        };
    }

    let extract_dim = |name: &str, getter: fn(&ScoreDimensions) -> f64| -> DimensionTrend {
        let scores: Vec<f64> = history.iter().map(|s| getter(&s.dimensions)).collect();
        let total_delta = scores.last().unwrap_or(&0.0) - scores.first().unwrap_or(&0.0);
        let average = scores.iter().sum::<f64>() / scores.len() as f64;
        let direction = classify_trend(total_delta);
        DimensionTrend {
            dimension: name.to_string(),
            direction,
            total_delta,
            average,
            history: scores,
        }
    };

    let dimension_trends = vec![
        extract_dim("factual_accuracy", |d| d.factual_accuracy),
        extract_dim("specificity", |d| d.specificity),
        extract_dim("temporal_awareness", |d| d.temporal_awareness),
        extract_dim("source_attribution", |d| d.source_attribution),
        extract_dim("confidence_calibration", |d| d.confidence_calibration),
    ];

    let overall_scores: Vec<f64> = history.iter().map(|s| s.overall).collect();
    let overall_delta =
        overall_scores.last().unwrap_or(&0.0) - overall_scores.first().unwrap_or(&0.0);

    ImprovementTrend {
        run_count: history.len(),
        overall_direction: classify_trend(overall_delta),
        overall_delta,
        dimension_trends,
    }
}

fn classify_trend(delta: f64) -> TrendDirection {
    const STABILITY_BAND: f64 = 0.02;
    if delta > STABILITY_BAND {
        TrendDirection::Improving
    } else if delta < -STABILITY_BAND {
        TrendDirection::Declining
    } else {
        TrendDirection::Stable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dims(v: f64) -> ScoreDimensions {
        ScoreDimensions {
            factual_accuracy: v,
            specificity: v * 0.9,
            temporal_awareness: v * 0.8,
            source_attribution: v * 0.7,
            confidence_calibration: v * 0.85,
        }
    }

    fn sr(id: &str, s: f64, ok: bool) -> GymScenarioResult {
        GymScenarioResult {
            scenario_id: id.into(),
            success: ok,
            score: s,
            dimensions: dims(s),
            question_count: 5,
            questions_answered: if ok { 5 } else { 0 },
            error_message: None,
            degraded_sources: vec![],
        }
    }

    fn ss(v: f64) -> GymSuiteScore {
        GymSuiteScore {
            suite_id: "s".into(),
            overall: v,
            dimensions: dims(v),
            scenario_count: 6,
            scenarios_passed: 6,
            pass_rate: 1.0,
            recorded_at_unix_ms: None,
        }
    }

    #[test]
    fn aggregate_and_regression_and_trend() {
        // Aggregate: empty
        assert_eq!(aggregate_suite_scores("t", &[]).scenario_count, 0);
        // Aggregate: averages
        let r = vec![
            sr("L1", 0.8, true),
            sr("L2", 0.6, true),
            sr("L3", 0.0, false),
        ];
        let s = aggregate_suite_scores("p", &r);
        assert!((s.overall - (0.8 + 0.6) / 3.0).abs() < 1e-9);
        assert_eq!(s.scenarios_passed, 2);
        // Regression: improved = empty
        assert!(detect_regression(&ss(0.9), &ss(0.5)).is_empty());
        // Regression: severe
        assert!(
            detect_regression(&ss(0.5), &ss(0.8))
                .iter()
                .any(|r| r.severity == RegressionSeverity::Severe)
        );
        // Regression: minor
        assert!(
            detect_regression(&ss(0.77), &ss(0.8))
                .iter()
                .any(|r| r.severity == RegressionSeverity::Minor)
        );
        // Regression: below threshold
        assert!(detect_regression(&ss(0.795), &ss(0.8)).is_empty());
        // Trend: single = stable
        assert_eq!(
            track_improvement(&[ss(0.7)]).overall_direction,
            TrendDirection::Stable
        );
        // Trend: improving
        let t = track_improvement(&[ss(0.5), ss(0.6), ss(0.8)]);
        assert_eq!(t.overall_direction, TrendDirection::Improving);
        // Trend: declining
        assert_eq!(
            track_improvement(&[ss(0.9), ss(0.7), ss(0.5)]).overall_direction,
            TrendDirection::Declining
        );
    }

    #[test]
    fn aggregate_empty_returns_zeroed_score() {
        let score = aggregate_suite_scores("empty-suite", &[]);
        assert_eq!(score.suite_id, "empty-suite");
        assert_eq!(score.overall, 0.0);
        assert_eq!(score.scenario_count, 0);
        assert_eq!(score.scenarios_passed, 0);
        assert_eq!(score.pass_rate, 0.0);
        assert!(score.recorded_at_unix_ms.is_none());
    }

    #[test]
    fn aggregate_single_result_uses_that_score() {
        let results = vec![sr("only", 0.75, true)];
        let score = aggregate_suite_scores("single", &results);
        assert_eq!(score.scenario_count, 1);
        assert_eq!(score.scenarios_passed, 1);
        assert!((score.pass_rate - 1.0).abs() < 1e-9);
        assert!((score.overall - 0.75).abs() < 1e-9);
    }

    #[test]
    fn aggregate_dimensions_are_averaged() {
        let results = vec![sr("a", 0.8, true), sr("b", 0.4, true)];
        let score = aggregate_suite_scores("avg", &results);
        let expected_fa = (dims(0.8).factual_accuracy + dims(0.4).factual_accuracy) / 2.0;
        assert!((score.dimensions.factual_accuracy - expected_fa).abs() < 1e-9);
        let expected_spec = (dims(0.8).specificity + dims(0.4).specificity) / 2.0;
        assert!((score.dimensions.specificity - expected_spec).abs() < 1e-9);
    }

    #[test]
    fn aggregate_pass_rate_with_mixed_results() {
        let results = vec![
            sr("a", 0.9, true),
            sr("b", 0.1, false),
            sr("c", 0.7, true),
            sr("d", 0.2, false),
        ];
        let score = aggregate_suite_scores("mixed", &results);
        assert_eq!(score.scenarios_passed, 2);
        assert!((score.pass_rate - 0.5).abs() < 1e-9);
    }

    #[test]
    fn suite_score_from_result_prefers_suite_level_values() {
        let scenario_results = vec![sr("a", 0.5, true), sr("b", 0.5, true)];
        let suite_result = GymSuiteResult {
            suite_id: "override".into(),
            success: true,
            overall_score: 0.99,
            dimensions: dims(0.88),
            scenario_results,
            scenarios_passed: 2,
            scenarios_total: 2,
            error_message: None,
            degraded_sources: vec![],
        };
        let score = suite_score_from_result(&suite_result);
        assert!((score.overall - 0.99).abs() < 1e-9);
        assert!((score.dimensions.factual_accuracy - dims(0.88).factual_accuracy).abs() < 1e-9);
        assert_eq!(score.scenario_count, 2);
    }

    #[test]
    fn regression_moderate_severity_band() {
        // delta of ~0.08 in overall -> moderate (> 0.05 but <= 0.15)
        let current = ss(0.72);
        let baseline = ss(0.80);
        let regs = detect_regression(&current, &baseline);
        let overall_reg = regs.iter().find(|r| r.dimension == "overall");
        assert!(overall_reg.is_some(), "should detect overall regression");
        assert_eq!(overall_reg.unwrap().severity, RegressionSeverity::Moderate);
    }

    #[test]
    fn regression_detects_each_dimension_independently() {
        let mut current = ss(0.8);
        current.dimensions.factual_accuracy = 0.3; // severe drop
        current.dimensions.specificity = 0.8 * 0.9; // unchanged
        let baseline = ss(0.8);
        let regs = detect_regression(&current, &baseline);
        assert!(
            regs.iter().any(|r| r.dimension == "factual_accuracy"),
            "factual_accuracy should regress"
        );
        assert!(
            !regs.iter().any(|r| r.dimension == "specificity"),
            "specificity should not regress"
        );
    }

    #[test]
    fn trend_empty_history_is_stable() {
        let trend = track_improvement(&[]);
        assert_eq!(trend.run_count, 0);
        assert_eq!(trend.overall_direction, TrendDirection::Stable);
        assert!(trend.dimension_trends.is_empty());
    }

    #[test]
    fn trend_two_entries_computes_dimension_trends() {
        let trend = track_improvement(&[ss(0.4), ss(0.7)]);
        assert_eq!(trend.run_count, 2);
        assert_eq!(trend.overall_direction, TrendDirection::Improving);
        assert_eq!(trend.dimension_trends.len(), 5);
        for dt in &trend.dimension_trends {
            assert_eq!(dt.history.len(), 2);
            assert!(dt.total_delta > 0.0);
        }
    }

    #[test]
    fn classify_trend_boundary_values() {
        // Exactly at the stability band boundary (0.02) should be Stable
        assert_eq!(classify_trend(0.02), TrendDirection::Stable);
        assert_eq!(classify_trend(-0.02), TrendDirection::Stable);
        // Just beyond
        assert_eq!(classify_trend(0.021), TrendDirection::Improving);
        assert_eq!(classify_trend(-0.021), TrendDirection::Declining);
        assert_eq!(classify_trend(0.0), TrendDirection::Stable);
    }

    #[test]
    fn aggregate_all_failed_scenarios() {
        let results = vec![sr("a", 0.1, false), sr("b", 0.2, false)];
        let score = aggregate_suite_scores("fail-suite", &results);
        assert_eq!(score.scenarios_passed, 0);
        assert!((score.pass_rate - 0.0).abs() < 1e-9);
        assert_eq!(score.scenario_count, 2);
    }

    #[test]
    fn aggregate_preserves_suite_id() {
        let score = aggregate_suite_scores("my-unique-id", &[sr("x", 0.5, true)]);
        assert_eq!(score.suite_id, "my-unique-id");
    }

    #[test]
    fn regression_identical_scores_empty() {
        let score = ss(0.75);
        assert!(
            detect_regression(&score, &score.clone()).is_empty(),
            "identical scores should produce no regressions"
        );
    }

    #[test]
    fn regression_within_threshold_no_regression() {
        // delta of -0.005 is well within threshold (0.01), no regression
        let current = ss(0.795);
        let baseline = ss(0.80);
        let regs = detect_regression(&current, &baseline);
        assert!(
            regs.is_empty(),
            "delta within threshold should produce no regressions"
        );
    }

    #[test]
    fn regression_severity_bands() {
        let baseline = ss(0.80);
        // Minor: delta abs between 0.01 and 0.05
        let minor = detect_regression(&ss(0.76), &baseline);
        let overall_minor = minor.iter().find(|r| r.dimension == "overall").unwrap();
        assert_eq!(overall_minor.severity, RegressionSeverity::Minor);

        // Severe: delta abs > 0.15 (use 0.60 for clear separation)
        let severe = detect_regression(&ss(0.60), &baseline);
        let overall_severe = severe.iter().find(|r| r.dimension == "overall").unwrap();
        assert_eq!(overall_severe.severity, RegressionSeverity::Severe);
    }

    #[test]
    fn regression_records_delta_and_scores() {
        let current = ss(0.5);
        let baseline = ss(0.8);
        let regs = detect_regression(&current, &baseline);
        let overall = regs.iter().find(|r| r.dimension == "overall").unwrap();
        assert!((overall.baseline_score - 0.8).abs() < 1e-9);
        assert!((overall.current_score - 0.5).abs() < 1e-9);
        assert!((overall.delta - (-0.3)).abs() < 1e-9);
    }

    #[test]
    fn suite_score_from_result_preserves_scenario_counts() {
        let scenario_results = vec![sr("a", 0.9, true), sr("b", 0.1, false), sr("c", 0.5, true)];
        let suite_result = GymSuiteResult {
            suite_id: "counts".into(),
            success: true,
            overall_score: 0.5,
            dimensions: dims(0.5),
            scenario_results,
            scenarios_passed: 2,
            scenarios_total: 3,
            error_message: None,
            degraded_sources: vec![],
        };
        let score = suite_score_from_result(&suite_result);
        assert_eq!(score.scenario_count, 3);
        assert_eq!(score.scenarios_passed, 2);
    }

    #[test]
    fn trend_long_history_tracks_overall_delta() {
        let scores: Vec<GymSuiteScore> = (0..10).map(|i| ss(0.3 + i as f64 * 0.05)).collect();
        let trend = track_improvement(&scores);
        assert_eq!(trend.run_count, 10);
        assert_eq!(trend.overall_direction, TrendDirection::Improving);
        assert!((trend.overall_delta - 0.45).abs() < 1e-9);
        assert_eq!(trend.dimension_trends.len(), 5);
        for dt in &trend.dimension_trends {
            assert_eq!(dt.history.len(), 10);
        }
    }
}
