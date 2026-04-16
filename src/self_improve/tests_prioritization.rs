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
    let past = vec![make_score(0.7), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    let fa = result
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();
    assert!((fa.trend_velocity - (-0.2)).abs() < 1e-9);
    assert!(fa.worsening);

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
        plateau: 0.0,
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

#[test]
fn proportional_trend_velocity_affects_priority() {
    let current = make_score(0.5);
    // Sharp decline: 0.8 → 0.5 over 2 intervals, velocity = -0.15
    let steep = vec![make_score(0.8), make_score(0.65), make_score(0.5)];
    // Mild decline: 0.6 → 0.5 over 2 intervals, velocity = -0.05
    let mild = vec![make_score(0.6), make_score(0.55), make_score(0.5)];

    let weights = PriorityWeights {
        deficit: 0.0,
        chronic: 0.0,
        trend: 1.0,
        plateau: 0.0,
    };

    let steep_result = prioritize_dimensions(&current, 0.6, &steep, &weights);
    let mild_result = prioritize_dimensions(&current, 0.6, &mild, &weights);

    // Every dimension should have strictly higher trend priority under steep decline
    for (s, m) in steep_result.iter().zip(mild_result.iter()) {
        assert_eq!(s.name, m.name);
        assert!(
            s.priority > m.priority,
            "{}: steep priority {:.4} should exceed mild {:.4}",
            s.name,
            s.priority,
            m.priority,
        );
    }
}

#[test]
fn trend_velocity_populated_on_worsening_dimension() {
    let current = make_score(0.5);
    let past = vec![make_score(0.7), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    // factual_accuracy: 0.7 → 0.5, velocity = -0.2
    let fa = result
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();
    assert!(fa.worsening);
    assert!((fa.trend_velocity - (-0.2)).abs() < 1e-9);
}

#[test]
fn trend_velocity_zero_when_no_history() {
    let current = make_score(0.5);
    let result = prioritize_dimensions_default(&current, 0.6, &[]);
    for dim in &result {
        assert!((dim.trend_velocity - 0.0).abs() < 1e-9);
    }
}

#[test]
fn plateau_detected_when_chronically_weak_with_zero_velocity() {
    let current = make_score(0.5);
    let past = vec![
        make_score(0.5),
        make_score(0.5),
        make_score(0.5),
        make_score(0.5),
    ];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    let sa = result
        .iter()
        .find(|d| d.name == "source_attribution")
        .unwrap();
    assert!(
        sa.plateau_detected,
        "source_attribution should be plateaued"
    );
    assert!(sa.current_deficit > 0.0);
    let ta = result
        .iter()
        .find(|d| d.name == "temporal_awareness")
        .unwrap();
    assert!(
        ta.plateau_detected,
        "temporal_awareness should be plateaued"
    );
}

#[test]
fn plateau_not_detected_with_insufficient_history() {
    let current = make_score(0.5);
    let past = vec![make_score(0.5), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    for dim in &result {
        assert!(
            !dim.plateau_detected,
            "{} should not be plateaued with only 2 cycles",
            dim.name
        );
    }
}

#[test]
fn plateau_not_detected_when_velocity_is_significant() {
    let current = make_score(0.5);
    let past = vec![make_score(0.3), make_score(0.4), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    let fa = result
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();
    assert!(
        !fa.plateau_detected,
        "improving dimension should not be plateaued"
    );
}

#[test]
fn plateau_boosts_priority() {
    let current = make_score(0.5);
    let past_plateau = vec![
        make_score(0.5),
        make_score(0.5),
        make_score(0.5),
        make_score(0.5),
    ];
    let result_plateau = prioritize_dimensions_default(&current, 0.6, &past_plateau);
    let result_no_history = prioritize_dimensions_default(&current, 0.6, &[]);

    let sa_plateau = result_plateau
        .iter()
        .find(|d| d.name == "source_attribution")
        .unwrap();
    let sa_no_hist = result_no_history
        .iter()
        .find(|d| d.name == "source_attribution")
        .unwrap();
    assert!(
        sa_plateau.priority > sa_no_hist.priority,
        "plateau priority ({}) should exceed no-history priority ({})",
        sa_plateau.priority,
        sa_no_hist.priority
    );
}

#[test]
fn decay_weights_recent_cycles_higher() {
    let past_recent_weak = vec![
        make_score(0.9),
        make_score(0.9),
        make_score(0.4),
        make_score(0.4),
        make_score(0.4),
    ];
    let past_old_weak = vec![
        make_score(0.4),
        make_score(0.4),
        make_score(0.4),
        make_score(0.9),
        make_score(0.9),
    ];
    let current = make_score(0.5);
    let result_recent = prioritize_dimensions_default(&current, 0.6, &past_recent_weak);
    let result_old = prioritize_dimensions_default(&current, 0.6, &past_old_weak);

    let sa_recent = result_recent
        .iter()
        .find(|d| d.name == "source_attribution")
        .unwrap();
    let sa_old = result_old
        .iter()
        .find(|d| d.name == "source_attribution")
        .unwrap();
    assert!(
        sa_recent.historical_weakness_rate > sa_old.historical_weakness_rate,
        "recent weakness rate ({}) should exceed old weakness rate ({})",
        sa_recent.historical_weakness_rate,
        sa_old.historical_weakness_rate
    );
}

#[test]
fn plateau_not_detected_when_currently_strong() {
    let current = make_score(0.9);
    let past = vec![make_score(0.5), make_score(0.5), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    for dim in &result {
        assert!(
            !dim.plateau_detected,
            "{} should not be plateaued when currently strong",
            dim.name
        );
    }
}

#[test]
fn detect_plateau_dimensions_returns_names() {
    let current = make_score(0.5);
    let past = vec![
        make_score(0.5),
        make_score(0.5),
        make_score(0.5),
        make_score(0.5),
    ];
    let plateaus = detect_plateau_dimensions(&current, 0.6, &past);
    assert!(
        plateaus.contains(&"source_attribution".to_string()),
        "source_attribution should be plateaued"
    );
    assert!(
        plateaus.contains(&"temporal_awareness".to_string()),
        "temporal_awareness should be plateaued"
    );
}

#[test]
fn detect_plateau_dimensions_empty_when_strong() {
    let current = make_score(0.9);
    let past = vec![make_score(0.9), make_score(0.9), make_score(0.9)];
    let plateaus = detect_plateau_dimensions(&current, 0.6, &past);
    assert!(plateaus.is_empty());
}

#[test]
fn enrich_with_history_populates_plateau_dimensions() {
    use super::types::ImprovementCycle;
    use super::types::ImprovementPhase;

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
    let past = vec![
        make_score(0.5),
        make_score(0.5),
        make_score(0.5),
        make_score(0.5),
    ];
    cycle.enrich_with_history(0.6, &past);
    assert!(
        !cycle.plateau_dimensions.is_empty(),
        "enrich_with_history should populate plateau_dimensions"
    );
    assert!(
        cycle
            .plateau_dimensions
            .contains(&"source_attribution".to_string())
    );
}

// ---- suggest_next_target ----

#[test]
fn suggest_next_target_returns_weakest_dimension() {
    let current = make_score(0.5);
    let suggestion = suggest_next_target(&current, 0.6, &[]);
    assert!(suggestion.is_some());
    let dim = suggestion.unwrap();
    // source_attribution has the largest deficit (0.5 * 0.7 = 0.35, deficit = 0.25)
    assert_eq!(dim.name, "source_attribution");
    assert!(dim.current_deficit > 0.0);
}

#[test]
fn suggest_next_target_none_when_all_strong() {
    let current = make_score(0.9);
    let suggestion = suggest_next_target(&current, 0.6, &[]);
    assert!(suggestion.is_none());
}

#[test]
fn suggest_next_target_considers_history() {
    let current = make_score(0.5);
    let past = vec![make_score(0.7), make_score(0.5)];
    let suggestion = suggest_next_target(&current, 0.6, &past);
    assert!(suggestion.is_some());
    let dim = suggestion.unwrap();
    assert_eq!(dim.name, "source_attribution");
    assert!(dim.worsening);
}

// ---- PriorityWeights::validate ----

#[test]
fn priority_weights_default_validates() {
    assert!(PriorityWeights::default().validate().is_ok());
}

#[test]
fn priority_weights_bad_sum_rejected() {
    let w = PriorityWeights {
        deficit: 0.5,
        chronic: 0.5,
        trend: 0.5,
        plateau: 0.1,
    };
    let err = w.validate().unwrap_err();
    assert!(err.contains("sum to ~1.0"));
}

#[test]
fn priority_weights_negative_rejected() {
    let w = PriorityWeights {
        deficit: -0.1,
        chronic: 0.6,
        trend: 0.5,
        plateau: 0.1,
    };
    let err = w.validate().unwrap_err();
    assert!(err.contains("non-negative"));
}

#[test]
fn priority_weights_custom_valid() {
    let w = PriorityWeights {
        deficit: 0.3,
        chronic: 0.3,
        trend: 0.4,
        plateau: 0.2,
    };
    assert!(w.validate().is_ok());
}

// ---- plateau detection edge cases ----

#[test]
fn plateau_detected_with_exactly_three_cycles() {
    let current = make_score(0.5);
    let past = vec![make_score(0.5), make_score(0.5), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    let sa = result
        .iter()
        .find(|d| d.name == "source_attribution")
        .unwrap();
    assert!(
        sa.plateau_detected,
        "plateau should be detected with exactly 3 cycles"
    );
}

#[test]
fn plateau_boundary_velocity_at_threshold() {
    // Velocity of exactly 0.05 should NOT trigger plateau (requires < 0.05)
    let current = make_score(0.5);
    // factual_accuracy: 0.4 → 0.5 over 2 intervals = velocity 0.05
    let past = vec![make_score(0.4), make_score(0.45), make_score(0.5)];
    let result = prioritize_dimensions_default(&current, 0.6, &past);
    let fa = result
        .iter()
        .find(|d| d.name == "factual_accuracy")
        .unwrap();
    assert!(
        !fa.plateau_detected,
        "velocity exactly at 0.05 boundary should not trigger plateau"
    );
}

// ---- dimension_value unknown dimension ----

#[test]
fn dimension_value_unknown_returns_zero() {
    let score = make_score(0.8);
    assert!((dimension_value(&score, "nonexistent") - 0.0).abs() < 1e-9);
}

#[test]
fn dimension_value_all_known_dimensions() {
    let score = make_score(0.8);
    for name in DIMENSION_NAMES {
        let val = dimension_value(&score, name);
        assert!(val > 0.0, "{name} should have a positive value");
    }
}
