//! Composite dimension prioritization combining current deficits with historical cycles.
//!
//! [`find_weak_dimensions_detailed`] is the single source of truth for
//! identifying dimensions below the weak threshold. [`cycle::find_weak_dimensions`](super::cycle::find_weak_dimensions)
//! delegates here. This module also provides composite prioritization that
//! weighs current deficit magnitude, historical weakness frequency, and trend
//! direction across past cycles.

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
    /// Composite priority score (higher = more urgent). Range roughly 0.0-1.0.
    pub priority: f64,
    /// Current deficit below the weak threshold (0.0 if not currently weak).
    pub current_deficit: f64,
    /// Recency-weighted fraction of past cycles where this dimension was below
    /// threshold. Recent cycles carry exponentially more weight (decay factor 0.7).
    pub historical_weakness_rate: f64,
    /// Whether the dimension is trending downward across past cycles.
    pub worsening: bool,
    /// Linear regression slope across past baselines (negative = declining).
    /// Zero when fewer than 2 past baselines exist.
    #[serde(default)]
    pub trend_slope: f64,
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
/// This is the canonical implementation used by both this module and
/// [`cycle::find_weak_dimensions`](super::cycle::find_weak_dimensions).
/// Results are sorted by deficit (largest first).
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
        if let Some(t) = target {
            if name != t {
                continue;
            }
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

            // Historical weakness rate with recency weighting.
            // Recent cycles carry exponentially more weight (decay = 0.7 per step).
            let weakness_rate = if past_baselines.is_empty() {
                0.0
            } else {
                recency_weighted_weakness_rate(past_baselines, name, weak_threshold)
            };

            // Trend: linear regression slope across all past baselines.
            let slope = trend_slope(past_baselines, name);
            let worsening = slope < 0.0;
            // Continuous trend signal: magnitude of decline (clamped to [0, 1]).
            let trend_signal = (-slope).clamp(0.0, 1.0);

            let priority = weights.deficit * normalized_deficit
                + weights.chronic * weakness_rate
                + weights.trend * trend_signal;

            PrioritizedDimension {
                name: name.to_string(),
                priority,
                current_deficit,
                historical_weakness_rate: weakness_rate,
                worsening,
                trend_slope: slope,
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

/// Compute recency-weighted historical weakness rate.
///
/// Each past cycle gets a weight that decays exponentially from most recent
/// to oldest: weight_i = decay^(n - 1 - i) where i=0 is the oldest cycle.
/// This makes recent weakness count more than ancient weakness.
fn recency_weighted_weakness_rate(
    past_baselines: &[GymSuiteScore],
    name: &str,
    weak_threshold: f64,
) -> f64 {
    const DECAY: f64 = 0.7;
    let n = past_baselines.len();
    let mut weighted_sum = 0.0;
    let mut weight_total = 0.0;
    for (i, score) in past_baselines.iter().enumerate() {
        // i=0 is oldest, i=n-1 is most recent
        let weight = DECAY.powi((n - 1 - i) as i32);
        weight_total += weight;
        if dimension_value(score, name) < weak_threshold {
            weighted_sum += weight;
        }
    }
    if weight_total > 0.0 {
        weighted_sum / weight_total
    } else {
        0.0
    }
}

/// Compute the linear regression slope of a dimension across past baselines.
///
/// Uses ordinary least squares with x = 0, 1, ..., n-1 (cycle index).
/// Returns 0.0 when fewer than 2 data points exist. A negative slope means
/// the dimension is declining over time.
fn trend_slope(past_baselines: &[GymSuiteScore], name: &str) -> f64 {
    let n = past_baselines.len();
    if n < 2 {
        return 0.0;
    }
    let n_f = n as f64;
    let x_mean = (n_f - 1.0) / 2.0;
    let y_mean: f64 = past_baselines
        .iter()
        .map(|s| dimension_value(s, name))
        .sum::<f64>()
        / n_f;

    let mut numerator = 0.0;
    let mut denominator = 0.0;
    for (i, score) in past_baselines.iter().enumerate() {
        let x_diff = i as f64 - x_mean;
        let y_diff = dimension_value(score, name) - y_mean;
        numerator += x_diff * y_diff;
        denominator += x_diff * x_diff;
    }
    if denominator.abs() < f64::EPSILON {
        0.0
    } else {
        numerator / denominator
    }
}
