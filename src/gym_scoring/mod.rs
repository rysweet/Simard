//! Score aggregation, regression detection, and improvement tracking for gym results.

use serde::{Deserialize, Serialize};

use crate::evidence::{EvidenceRecord, EvidenceSource};
use crate::gym::BenchmarkRunReport;
use crate::gym_client::{GymScenarioResult, GymSuiteResult, ScoreDimensions};
use crate::memory::MemoryRecord;

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

/// Structured evidence-quality assessment carried by a benchmark scorecard.
///
/// Replaces the coarse binary `sufficient`/`thin` categorical with the five
/// canonical scoring dimensions (see [`ScoreDimensions`]), an `overall` mean,
/// and a human-readable `category`. This makes evidence quality a meaningful
/// key scoring field that can distinguish quality differences between runs,
/// as required by the scoring strategy in `Specs/ProductArchitecture.md`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EvidenceQualityAssessment {
    pub dimensions: ScoreDimensions,
    pub overall: f64,
    pub category: String,
}

impl EvidenceQualityAssessment {
    /// Build an assessment from dimensions, deriving `overall` (their mean) and
    /// the human-readable `category`.
    pub fn from_dimensions(dimensions: ScoreDimensions) -> Self {
        let overall = dimensions.mean();
        let category = evidence_quality_category(overall).to_string();
        Self {
            dimensions,
            overall,
            category,
        }
    }
}

/// Map an overall evidence-quality score in `[0.0, 1.0]` to a coarse tier.
pub fn evidence_quality_category(overall: f64) -> &'static str {
    if overall >= 0.75 {
        "strong"
    } else if overall >= 0.5 {
        "moderate"
    } else {
        "weak"
    }
}

/// Raw signals captured during a benchmark run, used to derive a structured
/// [`EvidenceQualityAssessment`].
pub struct EvidenceQualityInputs<'a> {
    pub correctness_checks_passed: usize,
    pub correctness_checks_total: usize,
    pub evidence_records: &'a [EvidenceRecord],
    pub memory_records: &'a [MemoryRecord],
    pub unnecessary_action_count: Option<u32>,
    pub retry_count: Option<u32>,
}

/// Evidence-record count at which the quantity signal saturates to `1.0`.
const EVIDENCE_COUNT_TARGET: f64 = 4.0;
/// Evidence `detail` length (chars) at which the richness signal saturates.
const EVIDENCE_DETAIL_TARGET: f64 = 80.0;
/// Penalty per unnecessary action / retry when deriving calibration.
const CALIBRATION_PENALTY_PER_EVENT: f64 = 0.1;
/// Maximum calibration penalty contributed by either actions or retries.
const CALIBRATION_PENALTY_CAP: f64 = 0.5;

/// Derive a structured [`EvidenceQualityAssessment`] from raw run signals.
///
/// Each dimension is bounded to `[0.0, 1.0]` and responds to a distinct,
/// observable signal so two runs can be compared dimension-by-dimension:
///
/// - `factual_accuracy`: correctness-check pass rate.
/// - `specificity`: total evidence "units" — each record contributes up to one
///   unit scaled by its detail richness (saturating at [`EVIDENCE_DETAIL_TARGET`]
///   characters), with the summed units saturating at [`EVIDENCE_COUNT_TARGET`].
///   Empty-detail records contribute nothing.
/// - `temporal_awareness`: fraction of memory records carrying a creation
///   timestamp (temporal grounding).
/// - `source_attribution`: how strongly evidence is attributed to a concrete
///   source — base-type sources count fully, generic runtime sources partially.
/// - `confidence_calibration`: `1.0` minus capped penalties for unnecessary
///   actions and retries; unmeasured metrics incur no penalty.
pub fn assess_evidence_quality(inputs: &EvidenceQualityInputs) -> EvidenceQualityAssessment {
    let factual_accuracy = if inputs.correctness_checks_total > 0 {
        inputs.correctness_checks_passed as f64 / inputs.correctness_checks_total as f64
    } else {
        0.0
    };

    let evidence_count = inputs.evidence_records.len();
    // Each evidence record contributes up to one "evidence unit" scaled by its
    // detail richness (saturating at EVIDENCE_DETAIL_TARGET chars); the summed
    // units saturate at EVIDENCE_COUNT_TARGET. Empty-detail records contribute
    // nothing, so neither sparse evidence nor many empty records score well.
    let specificity = (inputs
        .evidence_records
        .iter()
        .map(|record| {
            (record.detail.trim().chars().count() as f64 / EVIDENCE_DETAIL_TARGET).min(1.0)
        })
        .sum::<f64>()
        / EVIDENCE_COUNT_TARGET)
        .min(1.0);

    let temporal_awareness = if inputs.memory_records.is_empty() {
        0.0
    } else {
        let grounded = inputs
            .memory_records
            .iter()
            .filter(|record| record.created_at.is_some())
            .count();
        grounded as f64 / inputs.memory_records.len() as f64
    };

    let source_attribution = if evidence_count == 0 {
        0.0
    } else {
        inputs
            .evidence_records
            .iter()
            .map(|record| match record.source {
                EvidenceSource::BaseType(_) => 1.0,
                EvidenceSource::Runtime => 0.5,
            })
            .sum::<f64>()
            / evidence_count as f64
    };

    let confidence_calibration =
        (1.0 - penalty(inputs.unnecessary_action_count) - penalty(inputs.retry_count)).max(0.0);

    EvidenceQualityAssessment::from_dimensions(ScoreDimensions {
        factual_accuracy,
        specificity,
        temporal_awareness,
        source_attribution,
        confidence_calibration,
    })
}

/// Capped calibration penalty for a measured event count. `None` (unmeasured)
/// yields no penalty.
fn penalty(count: Option<u32>) -> f64 {
    count
        .map(|c| (c as f64 * CALIBRATION_PENALTY_PER_EVENT).min(CALIBRATION_PENALTY_CAP))
        .unwrap_or(0.0)
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
/// directly. The overall score is the correctness-check pass rate. The five
/// dimensions are taken from the scorecard's structured
/// [`EvidenceQualityAssessment`] (see [`assess_evidence_quality`]), so the
/// regression/improvement pipeline operates on the same meaningful assessment
/// that the executor records.
///
/// Note: `GymSuiteScore.overall` remains the correctness-check pass rate (task
/// correctness), which is intentionally distinct from
/// `evidence_quality_assessment.overall`. Evidence-quality-only changes surface
/// through the per-dimension values consumed by [`detect_regression`], not
/// through `overall`.
pub fn suite_score_from_benchmark_report(report: &BenchmarkRunReport) -> GymSuiteScore {
    let checks_total = report.scorecard.correctness_checks_total;
    let checks_passed = report.scorecard.correctness_checks_passed;
    let pass_rate = if checks_total > 0 {
        checks_passed as f64 / checks_total as f64
    } else {
        0.0
    };

    GymSuiteScore {
        suite_id: report.suite_id.clone(),
        overall: pass_rate,
        dimensions: report
            .scorecard
            .evidence_quality_assessment
            .dimensions
            .clone(),
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
