use super::prioritization::*;
use crate::gym_bridge::ScoreDimensions;
use crate::gym_scoring::GymSuiteScore;

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

// ---- find_weak_dimensions_detailed ----

#[test]
fn detailed_weak_dims_all_above_threshold() {
    let score = make_score(0.8);
    let weak = find_weak_dimensions_detailed(&score, 0.5, None);
    assert!(weak.is_empty());
}

#[test]
fn detailed_weak_dims_some_below() {
    let score = make_score(0.5);
    let weak = find_weak_dimensions_detailed(&score, 0.45, None);
    assert!(weak.iter().any(|w| w.name == "source_attribution"));
    assert!(weak[0].deficit >= weak.last().unwrap().deficit);
}

#[test]
fn detailed_weak_dims_with_target() {
    let score = make_score(0.5);
    let weak = find_weak_dimensions_detailed(&score, 0.6, Some("factual_accuracy"));
    assert_eq!(weak.len(), 1);
    assert_eq!(weak[0].name, "factual_accuracy");
    assert!((weak[0].deficit - 0.1).abs() < 1e-9);
}

#[test]
fn detailed_weak_dims_deficit_values_correct() {
    let score = make_score(0.5);
    let weak = find_weak_dimensions_detailed(&score, 0.6, None);
    for w in &weak {
        assert!(w.deficit > 0.0);
        assert!(w.deficit <= 0.6);
    }
    let sa = weak
        .iter()
        .find(|w| w.name == "source_attribution")
        .unwrap();
    assert!((sa.deficit - 0.25).abs() < 1e-9);
}

// ---- prioritize_dimensions ----

#[test]
fn prioritize_no_history_uses_deficit_only() {
    let current = make_score(0.5);
    let result = prioritize_dimensions_default(&current, 0.6, &[]);
    assert_eq!(result.len(), 5);
    assert_eq!(result[0].name, "source_attribution");
    assert!((result[0].current_deficit - 0.25).abs() < 1e-9);
    for dim in &result {
        assert!((dim.trend_slope - 0.0).abs() < 1e-9);
    }
}

#[test]
fn prioritize_with_history_boosts_chronically_weak() {
    let current = make_score(0.7);
    let past = vec![make_score(0.5), make_score(0.5), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    let sa = result
        .iter()
        .find(|d| d.name == "source_attribution")
        .unwrap();
    assert!((sa.historical_weakness_rate - 1.0).abs() < 1e-9);
    let fa = result
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();
    assert!((fa.historical_weakness_rate - 1.0).abs() < 1e-9);
    assert!((fa.current_deficit - 0.0).abs() < 1e-9);
}

#[test]
fn prioritize_worsening_trend_detected() {
    let current = make_score(0.5);
    let past = vec![make_score(0.7), make_score(0.6), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    for dim in &result {
        assert!(dim.worsening, "{} should be worsening", dim.name);
        assert!(
            dim.trend_slope < 0.0,
            "{} should have negative slope",
            dim.name
        );
    }
}

#[test]
fn prioritize_improving_trend_not_worsening() {
    let current = make_score(0.7);
    let past = vec![make_score(0.5), make_score(0.6), make_score(0.7)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    for dim in &result {
        assert!(!dim.worsening, "{} should not be worsening", dim.name);
    }
}

#[test]
fn prioritize_all_signals_combined() {
    let current = make_score(0.5);
    let past = vec![make_score(0.7), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    let sa = &result[0];
    assert_eq!(sa.name, "source_attribution");
    assert!(sa.current_deficit > 0.0);
    assert!(sa.worsening);
    assert!(sa.historical_weakness_rate > 0.0);
    assert!(sa.priority > 0.0);
}

#[test]
fn prioritize_custom_weights() {
    let current = make_score(0.5);
    let past = vec![make_score(0.5), make_score(0.5)];
    let weights = PriorityWeights {
        deficit: 0.0,
        chronic: 0.0,
        trend: 1.0,
    };
    let result = prioritize_dimensions(&current, 0.6, &past, &weights);
    for dim in &result {
        assert!((dim.priority - 0.0).abs() < 1e-9);
    }
}

#[test]
fn prioritize_single_past_cycle_no_trend() {
    let current = make_score(0.5);
    let past = vec![make_score(0.8)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    for dim in &result {
        assert!(!dim.worsening);
    }
}

#[test]
fn prioritize_all_strong_zero_priority() {
    let current = make_score(0.9);
    let past = vec![make_score(0.9), make_score(0.9)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    for dim in &result {
        assert!((dim.current_deficit - 0.0).abs() < 1e-9);
        assert!((dim.historical_weakness_rate - 0.0).abs() < 1e-9);
        assert!(!dim.worsening);
        assert!((dim.priority - 0.0).abs() < 1e-9);
    }
}

#[test]
fn prioritize_returns_all_five_dimensions() {
    let current = make_score(0.5);
    let result = prioritize_dimensions_default(&current, 0.6, &[]);
    assert_eq!(result.len(), 5);
    let names: Vec<&str> = result.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"factual_accuracy"));
    assert!(names.contains(&"specificity"));
    assert!(names.contains(&"temporal_awareness"));
    assert!(names.contains(&"source_attribution"));
    assert!(names.contains(&"confidence_calibration"));
}

#[test]
fn prioritize_trend_slope_continuous_value() {
    let current = make_score(0.5);
    let past = vec![
        make_score(0.8),
        make_score(0.7),
        make_score(0.6),
        make_score(0.5),
    ];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    let fa = result
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();
    assert!((fa.trend_slope - (-0.1)).abs() < 1e-9);
}

#[test]
fn prioritize_improving_trend_positive_slope() {
    let current = make_score(0.7);
    let past = vec![make_score(0.5), make_score(0.6), make_score(0.7)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    let fa = result
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();
    assert!(
        fa.trend_slope > 0.0,
        "improving dim should have positive slope"
    );
    assert!(!fa.worsening);
}

#[test]
fn prioritize_recency_weights_recent_weakness_more() {
    let current = make_score(0.5);
    let past_a = vec![make_score(0.9), make_score(0.9), make_score(0.3)];
    let result_a = prioritize_dimensions_default(&current, 0.6, &past_a);
    let past_b = vec![make_score(0.3), make_score(0.9), make_score(0.9)];
    let result_b = prioritize_dimensions_default(&current, 0.6, &past_b);
    let fa_a = result_a
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();
    let fa_b = result_b
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();
    assert!(
        fa_a.historical_weakness_rate > fa_b.historical_weakness_rate,
        "recent weakness should rate higher: {} vs {}",
        fa_a.historical_weakness_rate,
        fa_b.historical_weakness_rate,
    );
}

#[test]
fn prioritize_flat_trend_zero_slope() {
    let current = make_score(0.5);
    let past = vec![make_score(0.5), make_score(0.5), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    for dim in &result {
        assert!(
            dim.trend_slope.abs() < 1e-9,
            "{} should have zero slope on flat data",
            dim.name,
        );
        assert!(!dim.worsening);
    }
}
