use super::trend::{analyze_trends, rank_dimensions_by_priority};
use super::types::*;
use crate::gym_bridge::ScoreDimensions;
use crate::gym_scoring::GymSuiteScore;

fn make_score(v: f64) -> GymSuiteScore {
    GymSuiteScore {
        suite_id: "test".into(),
        overall: v,
        dimensions: ScoreDimensions {
            factual_accuracy: v,
            specificity: v * 0.9,
            temporal_awareness: v * 0.8,
            source_attribution: v * 0.7,
            confidence_calibration: v * 0.85,
        },
        scenario_count: 4,
        scenarios_passed: 4,
        pass_rate: 1.0,
        recorded_at_unix_ms: None,
    }
}

fn make_cycle(overall: f64) -> ImprovementCycle {
    ImprovementCycle {
        baseline: make_score(overall),
        proposed_changes: Vec::new(),
        post_score: None,
        regressions: Vec::new(),
        decision: None,
        final_phase: ImprovementPhase::Eval,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
    }
}

#[test]
fn empty_cycles_produces_empty_trend() {
    let trend = analyze_trends(&[], 10, 0.6);
    assert_eq!(trend.cycle_count, 0);
    assert!(trend.overall_scores.is_empty());
    assert!(trend.dimensions.is_empty());
    assert!((trend.overall_delta - 0.0).abs() < 1e-9);
}

#[test]
fn single_cycle_zero_delta() {
    let cycles = vec![make_cycle(0.7)];
    let trend = analyze_trends(&cycles, 10, 0.6);
    assert_eq!(trend.cycle_count, 1);
    assert_eq!(trend.overall_scores.len(), 1);
    assert!((trend.overall_delta - 0.0).abs() < 1e-9);
}

#[test]
fn improving_trend_positive_delta() {
    let cycles = vec![make_cycle(0.5), make_cycle(0.6), make_cycle(0.7)];
    let trend = analyze_trends(&cycles, 10, 0.6);
    assert_eq!(trend.cycle_count, 3);
    assert!((trend.overall_delta - 0.2).abs() < 1e-9);
}

#[test]
fn declining_trend_negative_delta() {
    let cycles = vec![make_cycle(0.8), make_cycle(0.7), make_cycle(0.6)];
    let trend = analyze_trends(&cycles, 10, 0.6);
    assert!((trend.overall_delta - (-0.2)).abs() < 1e-9);
}

#[test]
fn max_cycles_window_applied() {
    let cycles = vec![
        make_cycle(0.3),
        make_cycle(0.5),
        make_cycle(0.6),
        make_cycle(0.7),
    ];
    let trend = analyze_trends(&cycles, 2, 0.6);
    assert_eq!(trend.cycle_count, 2);
    // Window is last 2: [0.6, 0.7]
    assert!((trend.overall_delta - 0.1).abs() < 1e-9);
}

#[test]
fn chronically_weak_dimension_detected() {
    // source_attribution = overall * 0.7
    // With overall at 0.5, source_attribution = 0.35 — well below 0.6
    let cycles = vec![make_cycle(0.5), make_cycle(0.5), make_cycle(0.5)];
    let trend = analyze_trends(&cycles, 10, 0.6);
    let sa = trend
        .dimensions
        .iter()
        .find(|d| d.dimension == "source_attribution")
        .unwrap();
    assert!(sa.chronically_weak);
}

#[test]
fn strong_dimension_not_chronically_weak() {
    // factual_accuracy = overall, at 0.8 that's above 0.6
    let cycles = vec![make_cycle(0.8), make_cycle(0.8), make_cycle(0.8)];
    let trend = analyze_trends(&cycles, 10, 0.6);
    let fa = trend
        .dimensions
        .iter()
        .find(|d| d.dimension == "factual_accuracy")
        .unwrap();
    assert!(!fa.chronically_weak);
}

#[test]
fn dimension_scores_tracked_correctly() {
    let cycles = vec![make_cycle(0.5), make_cycle(0.6), make_cycle(0.7)];
    let trend = analyze_trends(&cycles, 10, 0.6);
    let fa = trend
        .dimensions
        .iter()
        .find(|d| d.dimension == "factual_accuracy")
        .unwrap();
    assert_eq!(fa.scores.len(), 3);
    assert!((fa.scores[0] - 0.5).abs() < 1e-9);
    assert!((fa.scores[2] - 0.7).abs() < 1e-9);
    assert!((fa.net_delta - 0.2).abs() < 1e-9);
}

#[test]
fn rank_dimensions_chronically_weak_first() {
    // At 0.5 overall: source_attribution=0.35, temporal_awareness=0.40 are weak
    // factual_accuracy=0.50 is also weak, but less chronically
    let cycles = vec![make_cycle(0.5), make_cycle(0.5), make_cycle(0.5)];
    let trend = analyze_trends(&cycles, 10, 0.6);
    let ranked = rank_dimensions_by_priority(&trend);
    // All are chronically weak at these scores, but source_attribution has lowest average
    assert_eq!(ranked[0], "source_attribution");
}

#[test]
fn rank_dimensions_with_mixed_weakness() {
    // Two cycles at 0.5, one at 0.9 — some dimensions will be weak in >50% of cycles
    let cycles = vec![make_cycle(0.5), make_cycle(0.5), make_cycle(0.9)];
    let trend = analyze_trends(&cycles, 10, 0.6);
    let ranked = rank_dimensions_by_priority(&trend);
    // Should have 5 dimensions
    assert_eq!(ranked.len(), 5);
    // source_attribution at [0.35, 0.35, 0.63] — weak in 2/3 = chronically weak
    let sa_idx = ranked
        .iter()
        .position(|d| d == "source_attribution")
        .unwrap();
    // factual_accuracy at [0.5, 0.5, 0.9] — weak in 2/3 = chronically weak
    let fa_idx = ranked.iter().position(|d| d == "factual_accuracy").unwrap();
    // sa should be before fa because its average is lower
    assert!(sa_idx < fa_idx);
}

#[test]
fn five_dimensions_always_present() {
    let cycles = vec![make_cycle(0.7)];
    let trend = analyze_trends(&cycles, 10, 0.6);
    assert_eq!(trend.dimensions.len(), 5);
    let names: Vec<&str> = trend
        .dimensions
        .iter()
        .map(|d| d.dimension.as_str())
        .collect();
    assert!(names.contains(&"factual_accuracy"));
    assert!(names.contains(&"specificity"));
    assert!(names.contains(&"temporal_awareness"));
    assert!(names.contains(&"source_attribution"));
    assert!(names.contains(&"confidence_calibration"));
}

#[test]
fn cycle_trend_serde_round_trip() {
    let cycles = vec![make_cycle(0.5), make_cycle(0.6), make_cycle(0.7)];
    let trend = analyze_trends(&cycles, 10, 0.6);
    let json = serde_json::to_string(&trend).expect("serialize CycleTrend");
    let deser: super::trend::CycleTrend =
        serde_json::from_str(&json).expect("deserialize CycleTrend");
    assert_eq!(deser.cycle_count, trend.cycle_count);
    assert_eq!(deser.dimensions.len(), trend.dimensions.len());
    assert!((deser.overall_delta - trend.overall_delta).abs() < 1e-9);
}

#[test]
fn dimension_trend_serde_round_trip() {
    let cycles = vec![make_cycle(0.5)];
    let trend = analyze_trends(&cycles, 10, 0.6);
    let dim = &trend.dimensions[0];
    let json = serde_json::to_string(dim).expect("serialize DimensionTrend");
    let deser: super::trend::DimensionTrend =
        serde_json::from_str(&json).expect("deserialize DimensionTrend");
    assert_eq!(deser.dimension, dim.dimension);
    assert_eq!(deser.chronically_weak, dim.chronically_weak);
}
