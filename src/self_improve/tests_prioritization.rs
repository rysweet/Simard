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
    // source_attribution = 0.35, temporal_awareness = 0.40, both below 0.45
    let weak = find_weak_dimensions_detailed(&score, 0.45, None);
    assert!(weak.iter().any(|w| w.name == "source_attribution"));
    // sorted by deficit descending
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
    // source_attribution = 0.35, deficit = 0.25
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
    // source_attribution has highest deficit (0.25) so should be first
    assert_eq!(result[0].name, "source_attribution");
    assert!((result[0].current_deficit - 0.25).abs() < 1e-9);
}

#[test]
fn prioritize_with_history_boosts_chronically_weak() {
    let current = make_score(0.7); // all dims above 0.6 except source_attribution=0.49
    let past = vec![make_score(0.5), make_score(0.5), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    // source_attribution was weak in all 3 past cycles (0.35 < 0.6) AND currently weak
    let sa = result
        .iter()
        .find(|d| d.name == "source_attribution")
        .unwrap();
    assert!((sa.historical_weakness_rate - 1.0).abs() < 1e-9);
    // factual_accuracy was weak in all 3 past cycles (0.5 < 0.6) but NOT currently weak (0.7)
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
    // Trend: first=0.7 -> last=0.5 = worsening for all dimensions
    let past = vec![make_score(0.7), make_score(0.6), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    for dim in &result {
        assert!(dim.worsening, "{} should be worsening", dim.name);
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
fn prioritize_trend_velocity_proportional() {
    let current = make_score(0.5);
    // 2 past cycles: 0.7 → 0.5, velocity = (0.5 - 0.7) / 1 = -0.2 per cycle
    let past = vec![make_score(0.7), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    let fa = result
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();
    assert!((fa.trend_velocity - (-0.2)).abs() < 1e-9);
    assert!(fa.worsening);

    // Improving trend: velocity should be positive
    let past_up = vec![make_score(0.5), make_score(0.7)];
    let result_up = prioritize_dimensions_default(&current, 0.6, &past_up);
    let fa_up = result_up
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();
    assert!((fa_up.trend_velocity - 0.2).abs() < 1e-9);
    assert!(!fa_up.worsening);
}

#[test]
fn prioritize_trend_velocity_zero_with_single_history() {
    let current = make_score(0.5);
    let past = vec![make_score(0.7)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    for dim in &result {
        assert!((dim.trend_velocity - 0.0).abs() < 1e-9);
    }
}

#[test]
fn trend_velocity_nan_guard() {
    let nan_score = GymSuiteScore {
        suite_id: "test".into(),
        overall: f64::NAN,
        dimensions: ScoreDimensions {
            factual_accuracy: f64::NAN,
            specificity: f64::NAN,
            temporal_awareness: f64::NAN,
            source_attribution: f64::NAN,
            confidence_calibration: f64::NAN,
        },
        scenario_count: 0,
        scenarios_passed: 0,
        pass_rate: 0.0,
        recorded_at_unix_ms: None,
    };
    let current = make_score(0.5);
    let past = vec![nan_score.clone(), nan_score];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    for dim in &result {
        assert!(
            dim.trend_velocity.is_finite(),
            "trend_velocity should be finite for {}",
            dim.name
        );
        assert!(
            dim.priority.is_finite(),
            "priority should be finite for {}",
            dim.name
        );
    }
}

#[test]
fn prioritized_dimension_deserialize_without_trend_velocity() {
    let json = r#"{
        "name": "specificity",
        "priority": 0.5,
        "current_deficit": 0.1,
        "historical_weakness_rate": 0.3,
        "worsening": true
    }"#;
    let dim: PrioritizedDimension =
        serde_json::from_str(json).expect("should deserialize without trend_velocity");
    assert_eq!(dim.name, "specificity");
    assert!((dim.trend_velocity - 0.0).abs() < 1e-9);
}

#[test]
fn prioritize_all_signals_combined() {
    // current: source_attribution = 0.35 (deficit 0.25 from threshold 0.6)
    let current = make_score(0.5);
    // past: worsening from 0.7 to 0.5, all dims were weak in past cycles at 0.5
    let past = vec![make_score(0.7), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);

    // source_attribution has highest deficit, was weak in 50% of history, and is worsening
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
    // No worsening (all past cycles same), so all trend signals are 0
    for dim in &result {
        assert!((dim.priority - 0.0).abs() < 1e-9);
    }
}

#[test]
fn prioritize_single_past_cycle_no_trend() {
    let current = make_score(0.5);
    let past = vec![make_score(0.8)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    // With only 1 past cycle, worsening can't be determined
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
