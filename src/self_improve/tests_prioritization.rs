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

// ---- dimension_value ----

#[test]
fn dimension_value_known_dimensions() {
    let score = make_score(0.7);
    assert!((dimension_value(&score, "factual_accuracy") - 0.7).abs() < 1e-9);
    assert!((dimension_value(&score, "specificity") - 0.63).abs() < 1e-9);
    assert!((dimension_value(&score, "temporal_awareness") - 0.56).abs() < 1e-9);
    assert!((dimension_value(&score, "source_attribution") - 0.49).abs() < 1e-9);
    assert!((dimension_value(&score, "confidence_calibration") - 0.595).abs() < 1e-9);
}

#[test]
fn dimension_value_unknown_returns_zero() {
    let score = make_score(0.8);
    assert!((dimension_value(&score, "nonexistent") - 0.0).abs() < 1e-9);
    assert!((dimension_value(&score, "") - 0.0).abs() < 1e-9);
}

// ---- trend_velocity computation ----

#[test]
fn prioritize_trend_velocity_computed_correctly() {
    let current = make_score(0.5);
    // 3 cycles: 0.7 -> 0.6 -> 0.5, velocity = (0.5 - 0.7) / 2 = -0.1 per cycle
    let past = vec![make_score(0.7), make_score(0.6), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    let fa = result
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();
    // factual_accuracy: first=0.7, last=0.5, velocity = (0.5 - 0.7) / 2 = -0.1
    assert!(
        (fa.trend_velocity - (-0.1)).abs() < 1e-9,
        "expected velocity -0.1, got {}",
        fa.trend_velocity
    );
    assert!(fa.worsening);
}

#[test]
fn prioritize_trend_velocity_positive_when_improving() {
    let current = make_score(0.8);
    // 3 cycles: 0.5 -> 0.6 -> 0.7, velocity = (0.7 - 0.5) / 2 = +0.1 per cycle
    let past = vec![make_score(0.5), make_score(0.6), make_score(0.7)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    let fa = result
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();
    assert!(
        (fa.trend_velocity - 0.1).abs() < 1e-9,
        "expected velocity +0.1, got {}",
        fa.trend_velocity
    );
    assert!(!fa.worsening);
}

#[test]
fn prioritize_trend_velocity_zero_with_no_history() {
    let current = make_score(0.5);
    let result = prioritize_dimensions_default(&current, 0.6, &[]);
    for dim in &result {
        assert!(
            (dim.trend_velocity - 0.0).abs() < 1e-9,
            "{} should have zero velocity with no history, got {}",
            dim.name,
            dim.trend_velocity
        );
    }
}

#[test]
fn prioritize_fast_decline_higher_priority_than_slow() {
    // Fast decline: 0.9 -> 0.3 over 2 steps = velocity -0.3/step
    let fast_past = vec![make_score(0.9), make_score(0.6), make_score(0.3)];
    // Slow decline: 0.7 -> 0.5 over 2 steps = velocity -0.1/step
    let slow_past = vec![make_score(0.7), make_score(0.6), make_score(0.5)];

    let current = make_score(0.5);
    let weights = PriorityWeights {
        deficit: 0.0,
        chronic: 0.0,
        trend: 1.0, // only trend matters
    };

    let fast_result = prioritize_dimensions(&current, 0.6, &fast_past, &weights);
    let slow_result = prioritize_dimensions(&current, 0.6, &slow_past, &weights);

    let fast_fa = fast_result
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();
    let slow_fa = slow_result
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();

    assert!(
        fast_fa.priority > slow_fa.priority,
        "fast decline ({}) should have higher trend priority than slow decline ({})",
        fast_fa.priority,
        slow_fa.priority
    );
}

#[test]
fn prioritize_trend_velocity_per_dimension_varies() {
    // Build scores where different dimensions decline at different rates
    let past_1 = GymSuiteScore {
        suite_id: "test".into(),
        overall: 0.8,
        dimensions: ScoreDimensions {
            factual_accuracy: 0.8,
            specificity: 0.8,
            temporal_awareness: 0.8,
            source_attribution: 0.8,
            confidence_calibration: 0.8,
        },
        scenario_count: 4,
        scenarios_passed: 4,
        pass_rate: 1.0,
        recorded_at_unix_ms: None,
    };
    let past_2 = GymSuiteScore {
        suite_id: "test".into(),
        overall: 0.5,
        dimensions: ScoreDimensions {
            factual_accuracy: 0.7,        // dropped 0.1
            specificity: 0.4,             // dropped 0.4
            temporal_awareness: 0.8,      // no change
            source_attribution: 0.6,      // dropped 0.2
            confidence_calibration: 0.75, // dropped 0.05
        },
        scenario_count: 4,
        scenarios_passed: 4,
        pass_rate: 1.0,
        recorded_at_unix_ms: None,
    };
    let current = make_score(0.5);
    let result = prioritize_dimensions_default(&current, 0.6, &[past_1, past_2]);

    let spec = result.iter().find(|d| d.name == "specificity").unwrap();
    let fa = result
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();
    let ta = result
        .iter()
        .find(|d| d.name == "temporal_awareness")
        .unwrap();

    // specificity dropped 0.4 in 1 step = velocity -0.4
    assert!((spec.trend_velocity - (-0.4)).abs() < 1e-9);
    assert!(spec.worsening);

    // factual_accuracy dropped 0.1 in 1 step = velocity -0.1
    assert!((fa.trend_velocity - (-0.1)).abs() < 1e-9);
    assert!(fa.worsening);

    // temporal_awareness didn't change = velocity 0.0, not worsening
    assert!((ta.trend_velocity - 0.0).abs() < 1e-9);
    assert!(!ta.worsening);
}

// ---- PrioritizedDimension serde ----

#[test]
fn prioritized_dimension_serde_round_trip() {
    let dim = PrioritizedDimension {
        name: "specificity".into(),
        priority: 0.85,
        current_deficit: 0.15,
        historical_weakness_rate: 0.6,
        worsening: true,
        trend_velocity: -0.05,
    };
    let json = serde_json::to_string(&dim).unwrap();
    let parsed: PrioritizedDimension = serde_json::from_str(&json).unwrap();
    assert_eq!(dim, parsed);
}

// ---- PriorityWeights ----

#[test]
fn priority_weights_default_sums_to_one() {
    let w = PriorityWeights::default();
    let sum = w.deficit + w.chronic + w.trend;
    assert!(
        (sum - 1.0).abs() < 1e-9,
        "weights should sum to 1.0, got {sum}"
    );
}

#[test]
fn prioritize_all_zero_weights_yields_zero_priority() {
    let current = make_score(0.3);
    let past = vec![make_score(0.8), make_score(0.3)];
    let weights = PriorityWeights {
        deficit: 0.0,
        chronic: 0.0,
        trend: 0.0,
    };
    let result = prioritize_dimensions(&current, 0.6, &past, &weights);
    for dim in &result {
        assert!(
            (dim.priority - 0.0).abs() < 1e-9,
            "{} should have zero priority with zero weights, got {}",
            dim.name,
            dim.priority
        );
    }
}

#[test]
fn detailed_weak_dims_at_exact_threshold_not_weak() {
    let score = GymSuiteScore {
        suite_id: "test".into(),
        overall: 0.6,
        dimensions: ScoreDimensions {
            factual_accuracy: 0.6,
            specificity: 0.6,
            temporal_awareness: 0.6,
            source_attribution: 0.6,
            confidence_calibration: 0.6,
        },
        scenario_count: 4,
        scenarios_passed: 4,
        pass_rate: 1.0,
        recorded_at_unix_ms: None,
    };
    let weak = find_weak_dimensions_detailed(&score, 0.6, None);
    assert!(
        weak.is_empty(),
        "dimensions at exact threshold should not be weak"
    );
}

#[test]
fn prioritize_equal_deficit_deterministic_order() {
    // When all dimensions have identical scores, the output order should be
    // deterministic (stable — preserving the DIMENSION_NAMES declaration order
    // since all priorities are equal).
    let score = GymSuiteScore {
        suite_id: "test".into(),
        overall: 0.5,
        dimensions: ScoreDimensions {
            factual_accuracy: 0.5,
            specificity: 0.5,
            temporal_awareness: 0.5,
            source_attribution: 0.5,
            confidence_calibration: 0.5,
        },
        scenario_count: 4,
        scenarios_passed: 4,
        pass_rate: 1.0,
        recorded_at_unix_ms: None,
    };
    let result1 = prioritize_dimensions_default(&score, 0.6, &[]);
    let result2 = prioritize_dimensions_default(&score, 0.6, &[]);
    let names1: Vec<&str> = result1.iter().map(|d| d.name.as_str()).collect();
    let names2: Vec<&str> = result2.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names1, names2,
        "equal-priority dimensions should have deterministic order"
    );
    // All priorities should be equal
    for dim in &result1 {
        assert!(
            (dim.priority - result1[0].priority).abs() < 1e-9,
            "all dimensions should have equal priority"
        );
    }
}

#[test]
fn prioritize_zero_overall_score() {
    let score = GymSuiteScore {
        suite_id: "test".into(),
        overall: 0.0,
        dimensions: ScoreDimensions {
            factual_accuracy: 0.0,
            specificity: 0.0,
            temporal_awareness: 0.0,
            source_attribution: 0.0,
            confidence_calibration: 0.0,
        },
        scenario_count: 0,
        scenarios_passed: 0,
        pass_rate: 0.0,
        recorded_at_unix_ms: None,
    };
    let result = prioritize_dimensions_default(&score, 0.6, &[]);
    assert_eq!(result.len(), 5);
    // All should have maximum deficit (0.6) and equal normalized priority
    for dim in &result {
        assert!((dim.current_deficit - 0.6).abs() < 1e-9);
    }
}
