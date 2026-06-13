//! Score aggregation, regression detection, and improvement tracking for gym results.

use serde::{Deserialize, Serialize};

use crate::gym::BenchmarkRunReport;
use crate::gym_bridge::{GymScenarioResult, GymSuiteResult, ScoreDimensions};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GymSuiteScore {
    pub suite_id: String,
    pub overall: f64,
    pub dimensions: ScoreDimensions,
    pub scenario_count: usize,
    pub scenarios_passed: usize,
    pub pass_rate: f64,
    pub recorded_at_unix_ms: Option<u128>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Regression {
    pub dimension: String,
    pub baseline_score: f64,
    pub current_score: f64,
    pub delta: f64,
    pub severity: RegressionSeverity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegressionSeverity {
    Minor,
    Moderate,
    Severe,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrendDirection {
    Improving,
    Stable,
    Declining,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DimensionTrend {
    pub dimension: String,
    pub direction: TrendDirection,
    pub total_delta: f64,
    pub average: f64,
    pub history: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImprovementTrend {
    pub run_count: usize,
    pub overall_direction: TrendDirection,
    pub overall_delta: f64,
    pub dimension_trends: Vec<DimensionTrend>,
}

/// Aggregate scenario results into a suite-level score. Empty input yields zeroed score.
pub fn aggregate_suite_scores(suite_id: &str, results: &[GymScenarioResult]) -> GymSuiteScore {
    if results.is_empty() {
        return GymSuiteScore {
            suite_id: suite_id.to_string(),
            overall: 0.0,
            dimensions: ScoreDimensions::default(),
            scenario_count: 0,
            scenarios_passed: 0,
            pass_rate: 0.0,
            recorded_at_unix_ms: None,
        };
    }

    let n = results.len() as f64;
    let passed = results.iter().filter(|r| r.success).count();
    let avg = |f: fn(&ScoreDimensions) -> f64| -> f64 {
        results.iter().map(|r| f(&r.dimensions)).sum::<f64>() / n
    };
    let dims = ScoreDimensions {
        factual_accuracy: avg(|d| d.factual_accuracy),
        specificity: avg(|d| d.specificity),
        temporal_awareness: avg(|d| d.temporal_awareness),
        source_attribution: avg(|d| d.source_attribution),
        confidence_calibration: avg(|d| d.confidence_calibration),
    };
    let overall = results.iter().map(|r| r.score).sum::<f64>() / n;

    GymSuiteScore {
        suite_id: suite_id.to_string(),
        overall,
        dimensions: dims,
        scenario_count: results.len(),
        scenarios_passed: passed,
        pass_rate: passed as f64 / n,
        recorded_at_unix_ms: None,
    }
}

/// Build a [`GymSuiteScore`] from a [`GymSuiteResult`], preferring suite-level values.
pub fn suite_score_from_result(result: &GymSuiteResult) -> GymSuiteScore {
    let mut score = aggregate_suite_scores(&result.suite_id, &result.scenario_results);
    // Prefer the suite-level values when present since they may differ from
    // a naive average of scenario results (e.g. weighted scoring).
    score.overall = result.overall_score;
    score.dimensions = result.dimensions.clone();
    score
}

/// Return regressions where a dimension dropped by more than 0.01 vs baseline.
pub fn detect_regression(current: &GymSuiteScore, baseline: &GymSuiteScore) -> Vec<Regression> {
    const THRESHOLD: f64 = 0.01;
    let c = &current.dimensions;
    let b = &baseline.dimensions;
    let pairs: [(&str, f64, f64); 6] = [
        ("factual_accuracy", c.factual_accuracy, b.factual_accuracy),
        ("specificity", c.specificity, b.specificity),
        (
            "temporal_awareness",
            c.temporal_awareness,
            b.temporal_awareness,
        ),
        (
            "source_attribution",
            c.source_attribution,
            b.source_attribution,
        ),
        (
            "confidence_calibration",
            c.confidence_calibration,
            b.confidence_calibration,
        ),
        ("overall", current.overall, baseline.overall),
    ];

    pairs
        .into_iter()
        .filter_map(|(name, curr, base)| {
            let delta = curr - base;
            if delta < -THRESHOLD {
                let severity = if delta.abs() > 0.15 {
                    RegressionSeverity::Severe
                } else if delta.abs() > 0.05 {
                    RegressionSeverity::Moderate
                } else {
                    RegressionSeverity::Minor
                };
                Some(Regression {
                    dimension: name.to_string(),
                    baseline_score: base,
                    current_score: curr,
                    delta,
                    severity,
                })
            } else {
                None
            }
        })
        .collect()
}

/// Analyze a chronological series of suite scores. Requires >= 2 entries for a trend.
pub fn track_improvement(history: &[GymSuiteScore]) -> ImprovementTrend {
    if history.len() < 2 {
        return ImprovementTrend {
            run_count: history.len(),
            overall_direction: TrendDirection::Stable,
            overall_delta: 0.0,
            dimension_trends: Vec::new(),
        };
    }

    let extract_dim = |name: &str, getter: fn(&ScoreDimensions) -> f64| -> DimensionTrend {
        let scores: Vec<f64> = history.iter().map(|s| getter(&s.dimensions)).collect();
        let total_delta = scores.last().unwrap_or(&0.0) - scores.first().unwrap_or(&0.0);
        let average = scores.iter().sum::<f64>() / scores.len() as f64;
        let direction = classify_trend(total_delta);
        DimensionTrend {
            dimension: name.to_string(),
            direction,
            total_delta,
            average,
            history: scores,
        }
    };

    let dimension_trends = vec![
        extract_dim("factual_accuracy", |d| d.factual_accuracy),
        extract_dim("specificity", |d| d.specificity),
        extract_dim("temporal_awareness", |d| d.temporal_awareness),
        extract_dim("source_attribution", |d| d.source_attribution),
        extract_dim("confidence_calibration", |d| d.confidence_calibration),
    ];

    let overall_scores: Vec<f64> = history.iter().map(|s| s.overall).collect();
    let overall_delta =
        overall_scores.last().unwrap_or(&0.0) - overall_scores.first().unwrap_or(&0.0);

    ImprovementTrend {
        run_count: history.len(),
        overall_direction: classify_trend(overall_delta),
        overall_delta,
        dimension_trends,
    }
}

fn classify_trend(delta: f64) -> TrendDirection {
    const STABILITY_BAND: f64 = 0.02;
    if delta > STABILITY_BAND {
        TrendDirection::Improving
    } else if delta < -STABILITY_BAND {
        TrendDirection::Declining
    } else {
        TrendDirection::Stable
    }
}

// ── BenchmarkRunReport intake ────────────────────────────────────────

/// Build a [`GymSuiteScore`] from a single [`BenchmarkRunReport`].
///
/// Maps the benchmark scorecard into scoring dimensions so that
/// [`detect_regression`] and [`track_improvement`] can consume executor output
/// directly. The overall score is the correctness-check pass rate. Dimension
/// values are derived from scorecard signals:
///
/// - `factual_accuracy`: correctness-check pass rate
/// - `specificity`: evidence quality mapped to 0.0 / 0.5 / 1.0
/// - `temporal_awareness`: correctness-check pass rate (benchmarks test temporal aspects)
/// - `source_attribution`: evidence quality indicator
/// - `confidence_calibration`: 1.0 minus penalty for unnecessary actions and retries
pub fn suite_score_from_benchmark_report(report: &BenchmarkRunReport) -> GymSuiteScore {
    let checks_total = report.scorecard.correctness_checks_total;
    let checks_passed = report.scorecard.correctness_checks_passed;
    let pass_rate = if checks_total > 0 {
        checks_passed as f64 / checks_total as f64
    } else {
        0.0
    };

    let evidence_score = match report.scorecard.evidence_quality.as_str() {
        "sufficient" => 1.0,
        "thin" => 0.5,
        _ => 0.0,
    };

    let action_penalty = report
        .scorecard
        .unnecessary_action_count
        .map(|c| (c as f64 * 0.1).min(0.5))
        .unwrap_or(0.0);
    let retry_penalty = report
        .scorecard
        .retry_count
        .map(|c| (c as f64 * 0.1).min(0.5))
        .unwrap_or(0.0);
    let calibration = (1.0 - action_penalty - retry_penalty).max(0.0);

    GymSuiteScore {
        suite_id: report.suite_id.clone(),
        overall: pass_rate,
        dimensions: ScoreDimensions {
            factual_accuracy: pass_rate,
            specificity: evidence_score,
            temporal_awareness: pass_rate,
            source_attribution: evidence_score,
            confidence_calibration: calibration,
        },
        scenario_count: 1,
        scenarios_passed: if report.passed { 1 } else { 0 },
        pass_rate: if report.passed { 1.0 } else { 0.0 },
        recorded_at_unix_ms: Some(report.run_started_at_unix_ms),
    }
}

/// Build a [`GymSuiteScore`] by aggregating multiple [`BenchmarkRunReport`]s.
///
/// Each report is converted via [`suite_score_from_benchmark_report`] and
/// the resulting dimension values are averaged across all reports. This is the
/// primary entry point for suite-level regression detection after a full
/// benchmark suite run.
pub fn suite_score_from_benchmark_reports(
    suite_id: &str,
    reports: &[BenchmarkRunReport],
) -> GymSuiteScore {
    if reports.is_empty() {
        return GymSuiteScore {
            suite_id: suite_id.to_string(),
            overall: 0.0,
            dimensions: ScoreDimensions::default(),
            scenario_count: 0,
            scenarios_passed: 0,
            pass_rate: 0.0,
            recorded_at_unix_ms: None,
        };
    }

    let scores: Vec<GymSuiteScore> = reports
        .iter()
        .map(suite_score_from_benchmark_report)
        .collect();
    let n = scores.len() as f64;
    let overall = scores.iter().map(|s| s.overall).sum::<f64>() / n;
    let passed = scores.iter().filter(|s| s.scenarios_passed > 0).count();
    let dims = ScoreDimensions {
        factual_accuracy: scores
            .iter()
            .map(|s| s.dimensions.factual_accuracy)
            .sum::<f64>()
            / n,
        specificity: scores.iter().map(|s| s.dimensions.specificity).sum::<f64>() / n,
        temporal_awareness: scores
            .iter()
            .map(|s| s.dimensions.temporal_awareness)
            .sum::<f64>()
            / n,
        source_attribution: scores
            .iter()
            .map(|s| s.dimensions.source_attribution)
            .sum::<f64>()
            / n,
        confidence_calibration: scores
            .iter()
            .map(|s| s.dimensions.confidence_calibration)
            .sum::<f64>()
            / n,
    };
    let latest_ts = reports.iter().map(|r| r.run_started_at_unix_ms).max();

    GymSuiteScore {
        suite_id: suite_id.to_string(),
        overall,
        dimensions: dims,
        scenario_count: reports.len(),
        scenarios_passed: passed,
        pass_rate: passed as f64 / n,
        recorded_at_unix_ms: latest_ts,
    }
}

#[cfg(test)]
mod tests;
