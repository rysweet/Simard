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

// ── BenchmarkRunReport intake tests ─────────────────────────────────

use crate::gym::BenchmarkClass;
use crate::gym::{
    BenchmarkArtifactPaths, BenchmarkHandoffReport, BenchmarkRunReport, BenchmarkRuntimeReport,
    BenchmarkScenario, BenchmarkScorecard,
};
use crate::runtime::RuntimeTopology;

#[allow(clippy::too_many_arguments)]
fn make_report(
    suite: &str,
    scenario_id: &'static str,
    passed: bool,
    checks_passed: usize,
    checks_total: usize,
    evidence: &str,
    ts_ms: u128,
    unnecessary: Option<u32>,
    retries: Option<u32>,
) -> BenchmarkRunReport {
    BenchmarkRunReport {
        suite_id: suite.to_string(),
        scenario: BenchmarkScenario {
            id: scenario_id,
            title: "Test",
            description: "desc",
            class: BenchmarkClass::RepoExploration,
            identity: "test",
            base_type: "local-harness",
            topology: RuntimeTopology::SingleProcess,
            objective: "obj",
            expected_min_runtime_evidence: 1,
        },
        session_id: format!("session-{ts_ms}"),
        run_started_at_unix_ms: ts_ms,
        passed,
        checks: vec![],
        scorecard: BenchmarkScorecard {
            task_completed: passed,
            evidence_quality: evidence.to_string(),
            correctness_checks_passed: checks_passed,
            correctness_checks_total: checks_total,
            unnecessary_action_count: unnecessary,
            retry_count: retries,
            human_review_notes: vec![],
            measurement_notes: vec![],
        },
        plan: String::new(),
        execution_summary: String::new(),
        reflection_summary: String::new(),
        benchmark_memory_key: String::new(),
        benchmark_evidence_id: String::new(),
        runtime: BenchmarkRuntimeReport {
            identity: String::new(),
            selected_base_type: String::new(),
            topology: String::new(),
            adapter_implementation: String::new(),
            topology_backend: String::new(),
            transport_backend: String::new(),
            supervisor_backend: String::new(),
            runtime_node: String::new(),
            mailbox_address: String::new(),
            snapshot_state_before_stop: String::new(),
            snapshot_state_after_stop: String::new(),
        },
        handoff: BenchmarkHandoffReport {
            exported_state: String::new(),
            exported_memory_records: 0,
            exported_evidence_records: 0,
            restored_runtime_state: String::new(),
            restored_session_phase: None,
            restored_session_objective: None,
        },
        artifacts: BenchmarkArtifactPaths {
            run_dir: String::new(),
            report_json: String::new(),
            report_txt: String::new(),
            review_json: String::new(),
        },
    }
}

#[test]
fn suite_score_from_benchmark_report_pass_rate() {
    let report = make_report("s1", "sc1", true, 7, 10, "sufficient", 1000, None, None);
    let score = suite_score_from_benchmark_report(&report);
    assert_eq!(score.suite_id, "s1");
    assert!((score.overall - 0.7).abs() < 1e-9);
    assert!((score.dimensions.factual_accuracy - 0.7).abs() < 1e-9);
    assert!((score.dimensions.specificity - 1.0).abs() < 1e-9); // sufficient
    assert_eq!(score.scenario_count, 1);
    assert_eq!(score.scenarios_passed, 1);
    assert_eq!(score.recorded_at_unix_ms, Some(1000));
}

#[test]
fn suite_score_from_benchmark_report_failed() {
    let report = make_report("s1", "sc1", false, 2, 10, "thin", 2000, None, None);
    let score = suite_score_from_benchmark_report(&report);
    assert!((score.overall - 0.2).abs() < 1e-9);
    assert!((score.dimensions.specificity - 0.5).abs() < 1e-9); // thin
    assert_eq!(score.scenarios_passed, 0);
    assert!((score.pass_rate - 0.0).abs() < 1e-9);
}

#[test]
fn suite_score_from_benchmark_report_zero_checks() {
    let report = make_report("s1", "sc1", false, 0, 0, "thin", 3000, None, None);
    let score = suite_score_from_benchmark_report(&report);
    assert_eq!(score.overall, 0.0);
}

#[test]
fn suite_score_from_benchmark_report_calibration_penalty() {
    // 3 unnecessary actions → 0.3 penalty, 2 retries → 0.2 penalty → calibration = 0.5
    let report = make_report(
        "s1",
        "sc1",
        true,
        10,
        10,
        "sufficient",
        4000,
        Some(3),
        Some(2),
    );
    let score = suite_score_from_benchmark_report(&report);
    assert!((score.dimensions.confidence_calibration - 0.5).abs() < 1e-9);
}

#[test]
fn suite_score_from_benchmark_report_calibration_clamped() {
    // 10 unnecessary actions → 1.0 penalty (capped at 0.5), 10 retries → 0.5 → calibration = 0.0
    let report = make_report(
        "s1",
        "sc1",
        true,
        10,
        10,
        "sufficient",
        5000,
        Some(10),
        Some(10),
    );
    let score = suite_score_from_benchmark_report(&report);
    assert_eq!(score.dimensions.confidence_calibration, 0.0);
}

#[test]
fn suite_score_from_benchmark_reports_empty() {
    let score = suite_score_from_benchmark_reports("empty", &[]);
    assert_eq!(score.scenario_count, 0);
    assert_eq!(score.overall, 0.0);
}

#[test]
fn suite_score_from_benchmark_reports_aggregates() {
    let reports = vec![
        make_report("s1", "sc1", true, 8, 10, "sufficient", 1000, None, None),
        make_report("s1", "sc2", true, 6, 10, "thin", 2000, None, None),
    ];
    let score = suite_score_from_benchmark_reports("s1", &reports);
    assert_eq!(score.scenario_count, 2);
    assert_eq!(score.scenarios_passed, 2);
    // overall = avg(0.8, 0.6) = 0.7
    assert!((score.overall - 0.7).abs() < 1e-9);
    // specificity = avg(1.0, 0.5) = 0.75
    assert!((score.dimensions.specificity - 0.75).abs() < 1e-9);
    assert_eq!(score.recorded_at_unix_ms, Some(2000));
}

#[test]
fn benchmark_report_flows_to_regression_detection() {
    let baseline = make_report("s1", "sc1", true, 9, 10, "sufficient", 1000, None, None);
    let current = make_report("s1", "sc1", true, 5, 10, "thin", 2000, Some(3), None);
    let baseline_score = suite_score_from_benchmark_report(&baseline);
    let current_score = suite_score_from_benchmark_report(&current);
    let regressions = detect_regression(&current_score, &baseline_score);
    assert!(
        !regressions.is_empty(),
        "should detect regressions from degraded benchmark run"
    );
    assert!(
        regressions
            .iter()
            .any(|r| r.dimension == "factual_accuracy"),
        "factual_accuracy should regress (0.5 vs 0.9)"
    );
}

#[test]
fn benchmark_report_flows_to_improvement_tracking() {
    let reports_over_time: Vec<GymSuiteScore> = (1..=5)
        .map(|i| {
            let r = make_report(
                "s1",
                "sc1",
                true,
                5 + i,
                10,
                "sufficient",
                i as u128 * 1000,
                None,
                None,
            );
            suite_score_from_benchmark_report(&r)
        })
        .collect();
    let trend = track_improvement(&reports_over_time);
    assert_eq!(trend.run_count, 5);
    assert_eq!(trend.overall_direction, TrendDirection::Improving);
}
