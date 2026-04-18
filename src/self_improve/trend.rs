//! Trend analysis across improvement cycles.
//!
//! Computes per-dimension score deltas over the last N cycles to surface
//! persistent weaknesses vs transient dips. This helps the improvement
//! cycle prioritize chronically-weak dimensions.

use serde::{Deserialize, Serialize};

use super::types::ImprovementCycle;

/// Per-dimension trend summary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DimensionTrend {
    pub dimension: String,
    /// Score values across cycles (oldest first).
    pub scores: Vec<f64>,
    /// Net change from first to last cycle.
    pub net_delta: f64,
    /// Whether this dimension is chronically weak (below threshold in >50% of cycles).
    pub chronically_weak: bool,
}

/// Aggregate trend across all dimensions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CycleTrend {
    /// Overall score values across cycles (oldest first).
    pub overall_scores: Vec<f64>,
    /// Net change in overall score from first to last cycle.
    pub overall_delta: f64,
    /// Per-dimension breakdowns.
    pub dimensions: Vec<DimensionTrend>,
    /// Number of cycles analyzed.
    pub cycle_count: usize,
}

/// Analyze trends across the last `max_cycles` improvement cycles.
///
/// `weak_threshold` is used to determine if a dimension is chronically weak
/// (below threshold in more than half of the analyzed cycles).
pub fn analyze_trends(
    cycles: &[ImprovementCycle],
    max_cycles: usize,
    weak_threshold: f64,
) -> CycleTrend {
    let window: &[ImprovementCycle] = if cycles.len() > max_cycles {
        &cycles[cycles.len() - max_cycles..]
    } else {
        cycles
    };

    if window.is_empty() {
        return CycleTrend {
            overall_scores: Vec::new(),
            overall_delta: 0.0,
            dimensions: Vec::new(),
            cycle_count: 0,
        };
    }

    let overall_scores: Vec<f64> = window.iter().map(|c| c.baseline.overall).collect();
    let overall_delta =
        overall_scores.last().unwrap_or(&0.0) - overall_scores.first().unwrap_or(&0.0);

    let dim_names = [
        "factual_accuracy",
        "specificity",
        "temporal_awareness",
        "source_attribution",
        "confidence_calibration",
    ];

    let dimensions = dim_names
        .iter()
        .map(|&name| {
            let scores: Vec<f64> = window
                .iter()
                .map(|c| dimension_value(&c.baseline, name))
                .collect();
            let net_delta = scores.last().unwrap_or(&0.0) - scores.first().unwrap_or(&0.0);
            let weak_count = scores.iter().filter(|&&v| v < weak_threshold).count();
            let chronically_weak = weak_count > window.len() / 2;
            DimensionTrend {
                dimension: name.to_string(),
                scores,
                net_delta,
                chronically_weak,
            }
        })
        .collect();

    CycleTrend {
        overall_scores,
        overall_delta,
        dimensions,
        cycle_count: window.len(),
    }
}

/// Rank dimensions by priority for improvement.
///
/// Chronically weak dimensions come first, then by lowest average score.
pub fn rank_dimensions_by_priority(trend: &CycleTrend) -> Vec<String> {
    let mut dims: Vec<&DimensionTrend> = trend.dimensions.iter().collect();
    dims.sort_by(|a, b| {
        // Chronically weak first
        b.chronically_weak.cmp(&a.chronically_weak).then_with(|| {
            // Then by average score (lower = higher priority)
            let avg_a = if a.scores.is_empty() {
                0.0
            } else {
                a.scores.iter().sum::<f64>() / a.scores.len() as f64
            };
            let avg_b = if b.scores.is_empty() {
                0.0
            } else {
                b.scores.iter().sum::<f64>() / b.scores.len() as f64
            };
            avg_a
                .partial_cmp(&avg_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    dims.iter().map(|d| d.dimension.clone()).collect()
}

fn dimension_value(score: &crate::gym_scoring::GymSuiteScore, name: &str) -> f64 {
    match name {
        "factual_accuracy" => score.dimensions.factual_accuracy,
        "specificity" => score.dimensions.specificity,
        "temporal_awareness" => score.dimensions.temporal_awareness,
        "source_attribution" => score.dimensions.source_attribution,
        "confidence_calibration" => score.dimensions.confidence_calibration,
        _ => 0.0,
    }
}
