//! Integration tests for gym evaluation bridge and scoring.
//!
//! These tests use an in-memory bridge transport to validate the full
//! pipeline: bridge call -> deserialization -> scoring -> regression
//! detection -> improvement tracking, without requiring a running Python
//! bridge server.

use simard::bridge::BridgeErrorPayload;
use simard::bridge_subprocess::InMemoryBridgeTransport;
use simard::gym_bridge::{GymBridge, GymScenarioResult, GymSuiteResult, ScoreDimensions};
use simard::gym_scoring::{
    GymSuiteScore, RegressionSeverity, TrendDirection, aggregate_suite_scores, detect_regression,
    suite_score_from_result, track_improvement,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn scenario_result(id: &str, score: f64, success: bool) -> GymScenarioResult {
    GymScenarioResult {
        scenario_id: id.to_string(),
        success,
        score,
        dimensions: ScoreDimensions {
            factual_accuracy: score,
            specificity: score * 0.9,
            temporal_awareness: score * 0.8,
            source_attribution: score * 0.7,
            confidence_calibration: score * 0.85,
        },
        question_count: 5,
        questions_answered: if success { 5 } else { 0 },
        error_message: if success {
            None
        } else {
            Some("test failure".to_string())
        },
        degraded_sources: vec![],
    }
}

fn suite_score(overall: f64, accuracy: f64) -> GymSuiteScore {
    GymSuiteScore {
        suite_id: "test".to_string(),
        overall,
        dimensions: ScoreDimensions {
            factual_accuracy: accuracy,
            specificity: overall * 0.9,
            temporal_awareness: overall * 0.8,
            source_attribution: overall * 0.7,
            confidence_calibration: overall * 0.85,
        },
        scenario_count: 6,
        scenarios_passed: 6,
        pass_rate: 1.0,
        recorded_at_unix_ms: None,
    }
}

fn mock_bridge_with_scenarios() -> GymBridge {
    let transport = InMemoryBridgeTransport::new("gym-eval", |method, params| match method {
        "gym.list_scenarios" => Ok(serde_json::json!([
            {
                "id": "L1",
                "name": "Single source direct recall",
                "description": "Baseline recall test",
                "level": "L1",
                "question_count": 5,
                "article_count": 1
            },
            {
                "id": "L2",
                "name": "Multi-source synthesis",
                "description": "Cross-source reasoning",
                "level": "L2",
                "question_count": 8,
                "article_count": 3
            }
        ])),
        "gym.run_scenario" => {
            let scenario_id = params
                .get("scenario_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            Ok(serde_json::json!({
                "scenario_id": scenario_id,
                "success": true,
                "score": 0.85,
                "dimensions": {
                    "factual_accuracy": 0.9,
                    "specificity": 0.8,
                    "temporal_awareness": 0.85,
                    "source_attribution": 0.75,
                    "confidence_calibration": 0.8
                },
                "question_count": 5,
                "questions_answered": 5,
                "degraded_sources": []
            }))
        }
        "gym.run_suite" => Ok(serde_json::json!({
            "suite_id": "progressive",
            "success": true,
            "overall_score": 0.78,
            "dimensions": {
                "factual_accuracy": 0.85,
                "specificity": 0.75,
                "temporal_awareness": 0.72,
                "source_attribution": 0.7,
                "confidence_calibration": 0.78
            },
            "scenario_results": [
                {
                    "scenario_id": "L1",
                    "success": true,
                    "score": 0.9,
                    "dimensions": {
                        "factual_accuracy": 0.95,
                        "specificity": 0.85,
                        "temporal_awareness": 0.9,
                        "source_attribution": 0.8,
                        "confidence_calibration": 0.85
                    },
                    "question_count": 5,
                    "questions_answered": 5,
                    "degraded_sources": []
                },
                {
                    "scenario_id": "L2",
                    "success": true,
                    "score": 0.66,
                    "dimensions": {
                        "factual_accuracy": 0.75,
                        "specificity": 0.65,
                        "temporal_awareness": 0.55,
                        "source_attribution": 0.6,
                        "confidence_calibration": 0.7
                    },
                    "question_count": 8,
                    "questions_answered": 8,
                    "degraded_sources": []
                }
            ],
            "scenarios_passed": 2,
            "scenarios_total": 2,
            "degraded_sources": []
        })),
        _ => Err(BridgeErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    GymBridge::new(Box::new(transport))
}

// ---------------------------------------------------------------------------
// Bridge integration tests
// ---------------------------------------------------------------------------

#[test]
fn bridge_list_scenarios_returns_typed_results() {
    let bridge = mock_bridge_with_scenarios();
    let scenarios = bridge.list_scenarios().unwrap();
    assert_eq!(scenarios.len(), 2);
    assert_eq!(scenarios[0].id, "L1");
    assert_eq!(scenarios[0].question_count, 5);
    assert_eq!(scenarios[1].id, "L2");
    assert_eq!(scenarios[1].article_count, 3);
}

#[test]
fn bridge_run_scenario_returns_scored_result() {
    let bridge = mock_bridge_with_scenarios();
    let result = bridge.run_scenario("L1").unwrap();
    assert!(result.success);
    assert_eq!(result.scenario_id, "L1");
    assert!((result.score - 0.85).abs() < 1e-9);
    assert!((result.dimensions.factual_accuracy - 0.9).abs() < 1e-9);
}

#[test]
fn bridge_run_suite_returns_aggregate_result() {
    let bridge = mock_bridge_with_scenarios();
    let result = bridge.run_suite("progressive").unwrap();
    assert!(result.success);
    assert_eq!(result.scenarios_passed, 2);
    assert_eq!(result.scenarios_total, 2);
    assert!((result.overall_score - 0.78).abs() < 1e-9);
    assert_eq!(result.scenario_results.len(), 2);
}

#[test]
fn bridge_error_propagates_as_simard_error() {
    let transport = InMemoryBridgeTransport::new("gym-eval", |_method, _params| {
        Err(BridgeErrorPayload {
            code: -32603,
            message: "eval backend crashed".to_string(),
        })
    });
    let bridge = GymBridge::new(Box::new(transport));
    let err = bridge.list_scenarios().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("eval backend crashed"));
}

// ---------------------------------------------------------------------------
// Scoring aggregation tests
// ---------------------------------------------------------------------------

#[test]
fn aggregate_suite_scores_averages_scenario_results() {
    let results = vec![
        scenario_result("L1", 0.9, true),
        scenario_result("L2", 0.7, true),
        scenario_result("L3", 0.5, true),
    ];
    let score = aggregate_suite_scores("progressive", &results);
    assert_eq!(score.scenario_count, 3);
    assert_eq!(score.scenarios_passed, 3);
    let expected_overall = (0.9 + 0.7 + 0.5) / 3.0;
    assert!((score.overall - expected_overall).abs() < 1e-9);
}

#[test]
fn aggregate_handles_mix_of_pass_and_fail() {
    let results = vec![
        scenario_result("L1", 0.9, true),
        scenario_result("L2", 0.0, false),
    ];
    let score = aggregate_suite_scores("progressive", &results);
    assert_eq!(score.scenarios_passed, 1);
    assert!((score.pass_rate - 0.5).abs() < 1e-9);
}

#[test]
fn suite_score_from_result_preserves_suite_level_values() {
    let result = GymSuiteResult {
        suite_id: "test".to_string(),
        success: true,
        overall_score: 0.77,
        dimensions: ScoreDimensions {
            factual_accuracy: 0.8,
            specificity: 0.75,
            temporal_awareness: 0.7,
            source_attribution: 0.65,
            confidence_calibration: 0.95,
        },
        scenario_results: vec![scenario_result("L1", 0.9, true)],
        scenarios_passed: 1,
        scenarios_total: 1,
        error_message: None,
        degraded_sources: vec![],
    };
    let score = suite_score_from_result(&result);
    // suite-level overall should win over naive average of scenario results.
    assert!((score.overall - 0.77).abs() < 1e-9);
    assert!((score.dimensions.confidence_calibration - 0.95).abs() < 1e-9);
}

// ---------------------------------------------------------------------------
// Regression detection tests
// ---------------------------------------------------------------------------

#[test]
fn detect_regression_catches_significant_drops() {
    let baseline = suite_score(0.85, 0.9);
    let current = suite_score(0.6, 0.5);
    let regressions = detect_regression(&current, &baseline);
    assert!(!regressions.is_empty());

    let accuracy = regressions
        .iter()
        .find(|r| r.dimension == "factual_accuracy")
        .expect("should detect factual_accuracy regression");
    assert!(accuracy.delta < 0.0);
    assert_eq!(accuracy.severity, RegressionSeverity::Severe);
}

#[test]
fn detect_regression_ignores_improvements() {
    let baseline = suite_score(0.5, 0.5);
    let current = suite_score(0.9, 0.9);
    let regressions = detect_regression(&current, &baseline);
    assert!(regressions.is_empty());
}

#[test]
fn detect_regression_ignores_small_fluctuations() {
    let baseline = suite_score(0.80, 0.80);
    let current = suite_score(0.795, 0.795);
    let regressions = detect_regression(&current, &baseline);
    assert!(
        regressions.is_empty(),
        "tiny drops below threshold should not trigger: {regressions:?}"
    );
}

// Improvement trend tests

#[test]
fn track_improvement_directions() {
    // Single entry: stable
    let t = track_improvement(&[suite_score(0.8, 0.8)]);
    assert_eq!(t.overall_direction, TrendDirection::Stable);
    assert!(t.dimension_trends.is_empty());
    // Stable when flat
    let t = track_improvement(&[suite_score(0.75, 0.75), suite_score(0.76, 0.76)]);
    assert_eq!(t.overall_direction, TrendDirection::Stable);
    // Improving
    let h = vec![
        suite_score(0.5, 0.5),
        suite_score(0.65, 0.65),
        suite_score(0.8, 0.8),
    ];
    let t = track_improvement(&h);
    assert_eq!(t.overall_direction, TrendDirection::Improving);
    assert_eq!(t.run_count, 3);
    for dt in &t.dimension_trends {
        assert_eq!(dt.history.len(), 3);
    }
    // Declining
    let h = vec![
        suite_score(0.9, 0.9),
        suite_score(0.7, 0.7),
        suite_score(0.5, 0.5),
    ];
    assert_eq!(
        track_improvement(&h).overall_direction,
        TrendDirection::Declining
    );
}

// End-to-end pipeline: bridge -> scoring -> regression -> trend

#[test]
fn full_pipeline_bridge_to_trend() {
    let bridge = mock_bridge_with_scenarios();
    let current_score = suite_score_from_result(&bridge.run_suite("progressive").unwrap());
    let baseline = suite_score(0.90, 0.95);
    let regressions = detect_regression(&current_score, &baseline);
    assert!(!regressions.is_empty(), "0.78 should regress vs 0.90");
    let trend = track_improvement(&[baseline, current_score]);
    assert_eq!(trend.overall_direction, TrendDirection::Declining);
}

// Degradation visibility and serialization (Pillar 11)

#[test]
fn degradation_and_serialization() {
    let r = GymScenarioResult {
        scenario_id: "L5".into(),
        success: false,
        score: 0.0,
        dimensions: ScoreDimensions::default(),
        question_count: 10,
        questions_answered: 0,
        error_message: Some("timeout".into()),
        degraded_sources: vec!["progressive_test_suite".into()],
    };
    assert_eq!(r.degraded_sources.len(), 1);
    assert!(r.error_message.is_some());
    // Roundtrip serialization
    let dims = ScoreDimensions {
        factual_accuracy: 0.91,
        specificity: 0.82,
        temporal_awareness: 0.73,
        source_attribution: 0.64,
        confidence_calibration: 0.55,
    };
    let back: ScoreDimensions =
        serde_json::from_str(&serde_json::to_string(&dims).unwrap()).unwrap();
    assert_eq!(dims, back);
}
