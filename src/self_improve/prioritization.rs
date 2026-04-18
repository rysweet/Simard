//! Composite dimension prioritization combining current deficits with historical cycles.
//!
//! [`find_weak_dimensions`](super::cycle::find_weak_dimensions) returns dimension names
//! but not their deficits. This module adds deficit-aware analysis and composite
//! prioritization that weighs current deficit magnitude, historical weakness
//! frequency, and trend direction across past cycles.

use crate::gym_scoring::GymSuiteScore;
use serde::{Deserialize, Serialize};

use super::types::WeakDimension;

/// The five standard scoring dimensions.
const DIMENSION_NAMES: [&str; 5] = [
    "factual_accuracy",
    "specificity",
    "temporal_awareness",
    "source_attribution",
    "confidence_calibration",
];

/// A dimension with a composite priority score combining multiple signals.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrioritizedDimension {
    /// Dimension name (e.g. "specificity").
    pub name: String,
    /// Composite priority score (higher = more urgent). Range roughly 0.0–1.0.
    pub priority: f64,
    /// Current deficit below the weak threshold (0.0 if not currently weak).
    pub current_deficit: f64,
    /// Fraction of past cycles where this dimension was below threshold.
    pub historical_weakness_rate: f64,
    /// Whether the dimension is trending downward across past cycles.
    pub worsening: bool,
}

/// Weights for composite scoring. Should sum to 1.0 for a normalized result.
#[derive(Clone, Debug)]
pub struct PriorityWeights {
    /// Weight for current deficit magnitude.
    pub deficit: f64,
    /// Weight for historical weakness frequency.
    pub chronic: f64,
    /// Weight for negative trend direction.
    pub trend: f64,
}

impl Default for PriorityWeights {
    fn default() -> Self {
        Self {
            deficit: 0.5,
            chronic: 0.3,
            trend: 0.2,
        }
    }
}

/// Identify dimensions scoring below the threshold, returning deficit details.
///
/// This is a standalone implementation that mirrors the deficit-aware behavior
/// now also present in [`find_weak_dimensions`](super::cycle::find_weak_dimensions),
/// with results sorted by deficit (largest first).
pub fn find_weak_dimensions_detailed(
    score: &GymSuiteScore,
    weak_threshold: f64,
    target: Option<&str>,
) -> Vec<WeakDimension> {
    let dims = &score.dimensions;
    let checks: [(&str, f64); 5] = [
        ("factual_accuracy", dims.factual_accuracy),
        ("specificity", dims.specificity),
        ("temporal_awareness", dims.temporal_awareness),
        ("source_attribution", dims.source_attribution),
        ("confidence_calibration", dims.confidence_calibration),
    ];
    let mut weak = Vec::new();
    for (name, value) in checks {
        if let Some(t) = target
            && name != t
        {
            continue;
        }
        if value < weak_threshold {
            weak.push(WeakDimension {
                name: name.to_string(),
                deficit: weak_threshold - value,
            });
        }
    }
    weak.sort_by(|a, b| {
        b.deficit
            .partial_cmp(&a.deficit)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    weak
}

/// Build a prioritized ranking of dimensions by combining current-cycle
/// deficits with historical cycle data.
///
/// `current_score` and `weak_threshold` determine the current deficit.
/// `past_baselines` provides historical scores for trend and chronic weakness
/// analysis. The result is sorted by composite priority (highest first).
pub fn prioritize_dimensions(
    current_score: &GymSuiteScore,
    weak_threshold: f64,
    past_baselines: &[GymSuiteScore],
    weights: &PriorityWeights,
) -> Vec<PrioritizedDimension> {
    let weak_dims = find_weak_dimensions_detailed(current_score, weak_threshold, None);

    let max_deficit = weak_dims
        .iter()
        .map(|w| w.deficit)
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);

    let mut results: Vec<PrioritizedDimension> = DIMENSION_NAMES
        .iter()
        .map(|&name| {
            let current_deficit = weak_dims
                .iter()
                .find(|w| w.name == name)
                .map(|w| w.deficit)
                .unwrap_or(0.0);

            let normalized_deficit = current_deficit / max_deficit;

            // Historical weakness rate: fraction of past cycles where this dim was weak.
            let weakness_rate = if past_baselines.is_empty() {
                0.0
            } else {
                let weak_count = past_baselines
                    .iter()
                    .filter(|s| dimension_value(s, name) < weak_threshold)
                    .count();
                weak_count as f64 / past_baselines.len() as f64
            };

            // Trend: compare first and last past baselines.
            let worsening = if past_baselines.len() >= 2 {
                let first = dimension_value(&past_baselines[0], name);
                let last = dimension_value(
                    past_baselines.last().expect("len >= 2 guarantees last()"),
                    name,
                );
                last < first
            } else {
                false
            };
            let trend_signal = if worsening { 1.0 } else { 0.0 };

            let priority = weights.deficit * normalized_deficit
                + weights.chronic * weakness_rate
                + weights.trend * trend_signal;

            PrioritizedDimension {
                name: name.to_string(),
                priority,
                current_deficit,
                historical_weakness_rate: weakness_rate,
                worsening,
            }
        })
        .collect();

    results.sort_by(|a, b| {
        b.priority
            .partial_cmp(&a.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results
}

/// Convenience wrapper using default weights.
pub fn prioritize_dimensions_default(
    current_score: &GymSuiteScore,
    weak_threshold: f64,
    past_baselines: &[GymSuiteScore],
) -> Vec<PrioritizedDimension> {
    prioritize_dimensions(
        current_score,
        weak_threshold,
        past_baselines,
        &PriorityWeights::default(),
    )
}

/// Look up a single dimension's value by name.
pub fn dimension_value(score: &GymSuiteScore, name: &str) -> f64 {
    match name {
        "factual_accuracy" => score.dimensions.factual_accuracy,
        "specificity" => score.dimensions.specificity,
        "temporal_awareness" => score.dimensions.temporal_awareness,
        "source_attribution" => score.dimensions.source_attribution,
        "confidence_calibration" => score.dimensions.confidence_calibration,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gym_bridge::ScoreDimensions;

    fn make_score(v: f64) -> GymSuiteScore {
        GymSuiteScore {
            suite_id: "test".into(),
            overall: v,
            dimensions: ScoreDimensions {
                factual_accuracy: v,
                specificity: v,
                temporal_awareness: v,
                source_attribution: v,
                confidence_calibration: v,
            },
            scenario_count: 4,
            scenarios_passed: 4,
            pass_rate: 1.0,
            recorded_at_unix_ms: None,
        }
    }

    #[test]
    fn priority_weights_default_sum_to_one() {
        let w = PriorityWeights::default();
        let sum = w.deficit + w.chronic + w.trend;
        assert!(
            (sum - 1.0).abs() < 1e-10,
            "weights must sum to 1.0, got {sum}"
        );
    }

    #[test]
    fn find_weak_dimensions_detailed_empty_when_all_above_threshold() {
        let score = make_score(0.9);
        let result = find_weak_dimensions_detailed(&score, 0.5, None);
        assert!(
            result.is_empty(),
            "no dimensions should be weak when all scores exceed threshold"
        );
    }

    #[test]
    fn find_weak_dimensions_detailed_detects_below_threshold() {
        let score = make_score(0.3);
        let result = find_weak_dimensions_detailed(&score, 0.5, None);
        assert_eq!(
            result.len(),
            5,
            "all 5 dimensions should be weak when score is 0.3"
        );
        for dim in &result {
            assert!(
                (dim.deficit - 0.2).abs() < 1e-10,
                "deficit should be 0.5 - 0.3 = 0.2"
            );
        }
    }

    #[test]
    fn find_weak_dimensions_detailed_sorted_by_deficit_descending() {
        let mut score = make_score(0.9);
        score.dimensions.specificity = 0.2; // large deficit
        score.dimensions.factual_accuracy = 0.4; // smaller deficit
        let result = find_weak_dimensions_detailed(&score, 0.5, None);
        assert_eq!(
            result[0].name, "specificity",
            "largest deficit should sort first"
        );
    }

    #[test]
    fn find_weak_dimensions_detailed_target_filter_restricts_to_one() {
        let score = make_score(0.3);
        let result = find_weak_dimensions_detailed(&score, 0.5, Some("specificity"));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "specificity");
    }

    #[test]
    fn prioritize_dimensions_returns_all_five() {
        let score = make_score(0.7);
        let result = prioritize_dimensions(&score, 0.5, &[], &PriorityWeights::default());
        assert_eq!(result.len(), 5, "must return one entry per dimension");
    }

    #[test]
    fn dimension_value_unknown_name_returns_zero() {
        let score = make_score(0.8);
        assert_eq!(dimension_value(&score, "unknown_dimension"), 0.0);
    }

    #[test]
    fn prioritize_dimensions_default_matches_explicit_default_weights() {
        let score = make_score(0.4);
        let past = vec![make_score(0.5), make_score(0.3)];
        let via_default = prioritize_dimensions_default(&score, 0.6, &past);
        let via_explicit = prioritize_dimensions(&score, 0.6, &past, &PriorityWeights::default());
        assert_eq!(via_default, via_explicit);
    }
}
