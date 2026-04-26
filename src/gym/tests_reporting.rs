use std::path::PathBuf;

use super::reporting::*;
use crate::gym::types::{
    BenchmarkArtifactPaths, BenchmarkCheckResult, BenchmarkClass, BenchmarkComparisonArtifactPaths,
    BenchmarkComparisonDelta, BenchmarkComparisonReport, BenchmarkComparisonRunSummary,
    BenchmarkComparisonStatus, BenchmarkHandoffReport, BenchmarkRunReport, BenchmarkRuntimeReport,
    BenchmarkScenario, BenchmarkScorecard,
};
use crate::runtime::RuntimeTopology;
// --- render_benchmark_count ---

#[test]
fn render_benchmark_count_some_value() {
    assert_eq!(render_benchmark_count(Some(42)), "42");
}

#[test]
fn render_benchmark_count_zero() {
    assert_eq!(render_benchmark_count(Some(0)), "0");
}

#[test]
fn render_benchmark_count_none() {
    assert_eq!(render_benchmark_count(None), "unmeasured");
}

// --- render_benchmark_delta ---

#[test]
fn render_benchmark_delta_positive() {
    assert_eq!(render_benchmark_delta(Some(5)), "+5");
}

#[test]
fn render_benchmark_delta_negative() {
    assert_eq!(render_benchmark_delta(Some(-3)), "-3");
}

#[test]
fn render_benchmark_delta_zero() {
    assert_eq!(render_benchmark_delta(Some(0)), "+0");
}

#[test]
fn render_benchmark_delta_none() {
    assert_eq!(render_benchmark_delta(None), "unmeasured");
}

// --- benchmark_count_delta ---

#[test]
fn benchmark_count_delta_both_some() {
    assert_eq!(benchmark_count_delta(Some(10), Some(7)), Some(3));
}

#[test]
fn benchmark_count_delta_both_some_negative() {
    assert_eq!(benchmark_count_delta(Some(3), Some(10)), Some(-7));
}

#[test]
fn benchmark_count_delta_current_none() {
    assert_eq!(benchmark_count_delta(None, Some(5)), None);
}

#[test]
fn benchmark_count_delta_previous_none() {
    assert_eq!(benchmark_count_delta(Some(5), None), None);
}

#[test]
fn benchmark_count_delta_both_none() {
    assert_eq!(benchmark_count_delta(None, None), None);
}

// --- display_path ---

#[test]
fn display_path_converts_path_to_string() {
    let path = PathBuf::from("/some/path/to/file.json");
    assert_eq!(display_path(&path), "/some/path/to/file.json");
}

#[test]
fn display_path_relative() {
    let path = PathBuf::from("target/gym/report.json");
    assert_eq!(display_path(&path), "target/gym/report.json");
}

// --- now_unix_ms ---

#[test]
fn now_unix_ms_returns_reasonable_timestamp() {
    let ms = now_unix_ms().unwrap();
    assert!(ms > 1_704_067_200_000);
    assert!(ms < 4_000_000_000_000);
}

// --- evidence_quality_rank ---

#[test]
fn evidence_quality_rank_sufficient() {
    assert_eq!(evidence_quality_rank("sufficient"), 2);
}

#[test]
fn evidence_quality_rank_thin() {
    assert_eq!(evidence_quality_rank("thin"), 1);
}

#[test]
fn evidence_quality_rank_unknown() {
    assert_eq!(evidence_quality_rank("unknown"), 0);
    assert_eq!(evidence_quality_rank(""), 0);
}

// --- compare_lower_is_better ---

#[test]
fn compare_lower_is_better_current_lower_is_improved() {
    assert_eq!(
        compare_lower_is_better(Some(2), Some(5)),
        Some(BenchmarkComparisonStatus::Improved)
    );
}

#[test]
fn compare_lower_is_better_current_higher_is_regressed() {
    assert_eq!(
        compare_lower_is_better(Some(5), Some(2)),
        Some(BenchmarkComparisonStatus::Regressed)
    );
}

#[test]
fn compare_lower_is_better_equal_returns_none() {
    assert_eq!(compare_lower_is_better(Some(3), Some(3)), None);
}

#[test]
fn compare_lower_is_better_current_none_returns_none() {
    assert_eq!(compare_lower_is_better(None, Some(3)), None);
}

#[test]
fn compare_lower_is_better_both_none_returns_none() {
    assert_eq!(compare_lower_is_better(None, None), None);
}

// --- compare_runs ---

fn make_summary(
    passed: bool,
    checks_passed: usize,
    evidence_records: usize,
    memory_records: usize,
    unnecessary_actions: Option<u32>,
    retry_count: Option<u32>,
    evidence_quality: &str,
) -> BenchmarkComparisonRunSummary {
    BenchmarkComparisonRunSummary {
        suite_id: "starter".to_string(),
        session_id: "session-test".to_string(),
        run_started_at_unix_ms: 1000,
        passed,
        correctness_checks_passed: checks_passed,
        correctness_checks_total: 10,
        evidence_quality: evidence_quality.to_string(),
        unnecessary_action_count: unnecessary_actions,
        retry_count,
        exported_memory_records: memory_records,
        exported_evidence_records: evidence_records,
        report_json: "/path/report.json".to_string(),
    }
}

#[test]
fn compare_runs_current_passed_previous_failed_is_improved() {
    let current = make_summary(true, 5, 5, 5, None, None, "sufficient");
    let previous = make_summary(false, 5, 5, 5, None, None, "sufficient");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Improved
    );
}

#[test]
fn compare_runs_current_failed_previous_passed_is_regressed() {
    let current = make_summary(false, 5, 5, 5, None, None, "sufficient");
    let previous = make_summary(true, 5, 5, 5, None, None, "sufficient");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Regressed
    );
}

#[test]
fn compare_runs_more_checks_passed_is_improved() {
    let current = make_summary(true, 8, 5, 5, None, None, "sufficient");
    let previous = make_summary(true, 5, 5, 5, None, None, "sufficient");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Improved
    );
}

#[test]
fn compare_runs_fewer_checks_passed_is_regressed() {
    let current = make_summary(true, 3, 5, 5, None, None, "sufficient");
    let previous = make_summary(true, 5, 5, 5, None, None, "sufficient");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Regressed
    );
}

#[test]
fn compare_runs_more_evidence_records_is_improved() {
    let current = make_summary(true, 5, 8, 5, None, None, "sufficient");
    let previous = make_summary(true, 5, 5, 5, None, None, "sufficient");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Improved
    );
}

#[test]
fn compare_runs_fewer_evidence_records_is_regressed() {
    let current = make_summary(true, 5, 3, 5, None, None, "sufficient");
    let previous = make_summary(true, 5, 5, 5, None, None, "sufficient");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Regressed
    );
}

#[test]
fn compare_runs_more_memory_records_is_improved() {
    let current = make_summary(true, 5, 5, 8, None, None, "sufficient");
    let previous = make_summary(true, 5, 5, 5, None, None, "sufficient");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Improved
    );
}

#[test]
fn compare_runs_fewer_memory_records_is_regressed() {
    let current = make_summary(true, 5, 5, 3, None, None, "sufficient");
    let previous = make_summary(true, 5, 5, 5, None, None, "sufficient");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Regressed
    );
}

#[test]
fn compare_runs_fewer_unnecessary_actions_is_improved() {
    let current = make_summary(true, 5, 5, 5, Some(1), None, "sufficient");
    let previous = make_summary(true, 5, 5, 5, Some(3), None, "sufficient");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Improved
    );
}

#[test]
fn compare_runs_more_unnecessary_actions_is_regressed() {
    let current = make_summary(true, 5, 5, 5, Some(5), None, "sufficient");
    let previous = make_summary(true, 5, 5, 5, Some(2), None, "sufficient");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Regressed
    );
}

#[test]
fn compare_runs_fewer_retries_is_improved() {
    let current = make_summary(true, 5, 5, 5, Some(0), Some(0), "sufficient");
    let previous = make_summary(true, 5, 5, 5, Some(0), Some(3), "sufficient");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Improved
    );
}

#[test]
fn compare_runs_more_retries_is_regressed() {
    let current = make_summary(true, 5, 5, 5, Some(0), Some(5), "sufficient");
    let previous = make_summary(true, 5, 5, 5, Some(0), Some(1), "sufficient");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Regressed
    );
}

#[test]
fn compare_runs_better_evidence_quality_is_improved() {
    let current = make_summary(true, 5, 5, 5, Some(0), Some(0), "sufficient");
    let previous = make_summary(true, 5, 5, 5, Some(0), Some(0), "thin");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Improved
    );
}

#[test]
fn compare_runs_worse_evidence_quality_is_regressed() {
    let current = make_summary(true, 5, 5, 5, Some(0), Some(0), "thin");
    let previous = make_summary(true, 5, 5, 5, Some(0), Some(0), "sufficient");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Regressed
    );
}

#[test]
fn compare_runs_identical_is_unchanged() {
    let current = make_summary(true, 5, 5, 5, Some(0), Some(0), "sufficient");
    let previous = make_summary(true, 5, 5, 5, Some(0), Some(0), "sufficient");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Unchanged
    );
}

#[test]
fn compare_runs_unmeasured_actions_skips_to_evidence_quality() {
    let current = make_summary(true, 5, 5, 5, None, None, "sufficient");
    let previous = make_summary(true, 5, 5, 5, None, None, "thin");
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Improved
    );
}

// --- render_comparison_summary ---

#[test]
fn render_comparison_summary_improved() {
    let current = make_summary(true, 8, 5, 5, Some(1), Some(0), "sufficient");
    let previous = make_summary(true, 5, 5, 5, Some(3), Some(1), "sufficient");
    let delta = BenchmarkComparisonDelta {
        correctness_checks_passed: 3,
        unnecessary_action_count: Some(-2),
        retry_count: Some(-1),
        exported_memory_records: 0,
        exported_evidence_records: 0,
    };
    let summary = render_comparison_summary(
        BenchmarkComparisonStatus::Improved,
        &current,
        &previous,
        &delta,
    );
    assert!(summary.contains("improved"));
    assert!(summary.contains(&previous.session_id));
    assert!(summary.contains(&current.session_id));
}

#[test]
fn render_comparison_summary_regressed() {
    let current = make_summary(true, 3, 5, 5, None, None, "sufficient");
    let previous = make_summary(true, 5, 5, 5, None, None, "sufficient");
    let delta = BenchmarkComparisonDelta {
        correctness_checks_passed: -2,
        unnecessary_action_count: None,
        retry_count: None,
        exported_memory_records: 0,
        exported_evidence_records: 0,
    };
    let summary = render_comparison_summary(
        BenchmarkComparisonStatus::Regressed,
        &current,
        &previous,
        &delta,
    );
    assert!(summary.contains("regressed"));
}
