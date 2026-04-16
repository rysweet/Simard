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
    /// Rate of change per cycle (negative = declining). Zero when fewer than 2
    /// data points exist.
    #[serde(default)]
    pub trend_velocity: f64,
    /// Whether the dimension appears stuck: currently weak, sufficient history,
    /// and near-zero velocity.
    #[serde(default)]
    pub plateau_detected: bool,
}

/// Weights for composite scoring. Should sum to 1.0 for a normalized result
/// (excluding the plateau bonus which is additive).
#[derive(Clone, Debug)]
pub struct PriorityWeights {
    /// Weight for current deficit magnitude.
    pub deficit: f64,
    /// Weight for historical weakness frequency.
    pub chronic: f64,
    /// Weight for negative trend direction.
    pub trend: f64,
    /// Additive bonus when a plateau is detected (default 0.1).
    pub plateau: f64,
}

impl Default for PriorityWeights {
    fn default() -> Self {
        Self {
            deficit: 0.5,
            chronic: 0.2,
            trend: 0.3,
            plateau: 0.1,
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
                .map_or(0.0, |w| w.deficit);

            let normalized_deficit = current_deficit / max_deficit;

            // Historical weakness rate: decay-weighted fraction of past cycles
            // where this dimension was below threshold. More recent cycles (later
            // indices) receive higher weight.
            let weakness_rate = if past_baselines.is_empty() {
                0.0
            } else {
                let n = past_baselines.len();
                let mut weighted_weak = 0.0;
                let mut total_weight = 0.0;
                for (i, s) in past_baselines.iter().enumerate() {
                    // Linear decay: weight = (i + 1) / n, so latest cycle has weight 1.0
                    let w = (i + 1) as f64 / n as f64;
                    if dimension_value(s, name) < weak_threshold {
                        weighted_weak += w;
                    }
                    total_weight += w;
                }
                if total_weight > 0.0 {
                    weighted_weak / total_weight
                } else {
                    0.0
                }
            };

            // Trend: compare first and last past baselines.
            let (worsening, trend_velocity) = if past_baselines.len() >= 2 {
                let first = dimension_value(&past_baselines[0], name);
                let last = dimension_value(
                    past_baselines.last().expect("len >= 2 guarantees last()"),
                    name,
                );
                let intervals = (past_baselines.len() - 1) as f64;
                let vel = (last - first) / intervals;
                let vel = if vel.is_finite() { vel } else { 0.0 };
                (vel < 0.0, vel)
            } else {
                (false, 0.0)
            };
            let trend_signal = (-trend_velocity).clamp(0.0, 1.0);

            // Plateau: currently weak, ≥3 past cycles, and near-zero velocity.
            let plateau_detected =
                current_deficit > 0.0 && past_baselines.len() >= 3 && trend_velocity.abs() < 0.05;

            let plateau_boost = if plateau_detected {
                weights.plateau
            } else {
                0.0
            };
            let priority = weights.deficit * normalized_deficit
                + weights.chronic * weakness_rate
                + weights.trend * trend_signal
                + plateau_boost;
            let priority = if priority.is_finite() { priority } else { 0.0 };

            PrioritizedDimension {
                name: name.to_string(),
                priority,
                current_deficit,
                historical_weakness_rate: weakness_rate,
                worsening,
                trend_velocity,
                plateau_detected,
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
