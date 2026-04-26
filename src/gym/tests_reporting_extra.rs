use std::path::PathBuf;

use super::reporting::*;
use crate::gym::types::{
    BenchmarkArtifactPaths, BenchmarkCheckResult, BenchmarkClass, BenchmarkComparisonArtifactPaths,
    BenchmarkComparisonDelta, BenchmarkComparisonReport, BenchmarkComparisonRunSummary,
    BenchmarkComparisonStatus, BenchmarkHandoffReport, BenchmarkRunReport, BenchmarkRuntimeReport,
    BenchmarkScenario, BenchmarkScorecard,
};
use crate::runtime::RuntimeTopology;

#[test]
fn render_benchmark_count_some() {
    assert_eq!(render_benchmark_count(Some(42)), "42");
}

#[test]
fn render_benchmark_count_none() {
    assert_eq!(render_benchmark_count(None), "unmeasured");
}

#[test]
fn render_benchmark_delta_positive() {
    assert_eq!(render_benchmark_delta(Some(3)), "+3");
}

#[test]
fn render_benchmark_delta_negative() {
    assert_eq!(render_benchmark_delta(Some(-5)), "-5");
}

#[test]
fn render_benchmark_delta_none() {
    assert_eq!(render_benchmark_delta(None), "unmeasured");
}

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
    assert_eq!(evidence_quality_rank("garbage"), 0);
    assert_eq!(evidence_quality_rank(""), 0);
}

#[test]
fn compare_lower_is_better_improved() {
    assert_eq!(
        compare_lower_is_better(Some(2), Some(5)),
        Some(BenchmarkComparisonStatus::Improved)
    );
}

#[test]
fn compare_lower_is_better_regressed() {
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
fn compare_lower_is_better_none_returns_none() {
    assert_eq!(compare_lower_is_better(None, Some(3)), None);
    assert_eq!(compare_lower_is_better(Some(3), None), None);
    assert_eq!(compare_lower_is_better(None, None), None);
}

#[test]
fn benchmark_count_delta_both_some() {
    assert_eq!(benchmark_count_delta(Some(10), Some(7)), Some(3));
    assert_eq!(benchmark_count_delta(Some(3), Some(8)), Some(-5));
}

#[test]
fn benchmark_count_delta_any_none() {
    assert_eq!(benchmark_count_delta(None, Some(5)), None);
    assert_eq!(benchmark_count_delta(Some(5), None), None);
    assert_eq!(benchmark_count_delta(None, None), None);
}

#[test]
fn display_path_renders_lossy() {
    let path = PathBuf::from("/foo/bar/baz.json");
    assert_eq!(display_path(&path), "/foo/bar/baz.json");
}

fn make_run_summary(
    passed: bool,
    checks_passed: usize,
    evidence: usize,
    memory: usize,
) -> BenchmarkComparisonRunSummary {
    BenchmarkComparisonRunSummary {
        suite_id: "s".into(),
        session_id: "sess".into(),
        run_started_at_unix_ms: 0,
        passed,
        correctness_checks_passed: checks_passed,
        correctness_checks_total: 10,
        evidence_quality: "sufficient".into(),
        unnecessary_action_count: None,
        retry_count: None,
        exported_memory_records: memory,
        exported_evidence_records: evidence,
        report_json: "r.json".into(),
    }
}

#[test]
fn compare_runs_pass_vs_fail() {
    let current = make_run_summary(true, 8, 4, 3);
    let previous = make_run_summary(false, 8, 4, 3);
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Improved
    );

    let current = make_run_summary(false, 8, 4, 3);
    let previous = make_run_summary(true, 8, 4, 3);
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Regressed
    );
}

#[test]
fn compare_runs_checks_differ() {
    let current = make_run_summary(true, 9, 4, 3);
    let previous = make_run_summary(true, 7, 4, 3);
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Improved
    );
}

#[test]
fn compare_runs_unchanged() {
    let a = make_run_summary(true, 8, 4, 3);
    let b = make_run_summary(true, 8, 4, 3);
    assert_eq!(compare_runs(&a, &b), BenchmarkComparisonStatus::Unchanged);
}

#[test]
fn now_unix_ms_returns_nonzero() {
    let ms = now_unix_ms().unwrap();
    assert!(ms > 0);
}

// --- compare_runs: evidence quality tiebreaker ---

#[test]
fn compare_runs_evidence_quality_improved() {
    let mut current = make_run_summary(true, 8, 4, 3);
    current.evidence_quality = "sufficient".into();
    let mut previous = make_run_summary(true, 8, 4, 3);
    previous.evidence_quality = "thin".into();
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Improved
    );
}

#[test]
fn compare_runs_evidence_quality_regressed() {
    let mut current = make_run_summary(true, 8, 4, 3);
    current.evidence_quality = "thin".into();
    let mut previous = make_run_summary(true, 8, 4, 3);
    previous.evidence_quality = "sufficient".into();
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Regressed
    );
}

// --- compare_runs: unnecessary_action_count tiebreaker ---

#[test]
fn compare_runs_fewer_unnecessary_actions_is_improved() {
    let mut current = make_run_summary(true, 8, 4, 3);
    current.unnecessary_action_count = Some(1);
    let mut previous = make_run_summary(true, 8, 4, 3);
    previous.unnecessary_action_count = Some(5);
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Improved
    );
}

#[test]
fn compare_runs_more_unnecessary_actions_is_regressed() {
    let mut current = make_run_summary(true, 8, 4, 3);
    current.unnecessary_action_count = Some(5);
    let mut previous = make_run_summary(true, 8, 4, 3);
    previous.unnecessary_action_count = Some(1);
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Regressed
    );
}

// --- compare_runs: retry_count tiebreaker ---

#[test]
fn compare_runs_fewer_retries_is_improved() {
    let mut current = make_run_summary(true, 8, 4, 3);
    current.retry_count = Some(0);
    let mut previous = make_run_summary(true, 8, 4, 3);
    previous.retry_count = Some(3);
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Improved
    );
}

// --- compare_runs: evidence records differ ---

#[test]
fn compare_runs_more_evidence_is_improved() {
    let current = make_run_summary(true, 8, 6, 3);
    let previous = make_run_summary(true, 8, 2, 3);
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Improved
    );
}

#[test]
fn compare_runs_fewer_evidence_is_regressed() {
    let current = make_run_summary(true, 8, 1, 3);
    let previous = make_run_summary(true, 8, 5, 3);
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Regressed
    );
}

// --- compare_runs: memory records differ ---

#[test]
fn compare_runs_more_memory_is_improved() {
    let current = make_run_summary(true, 8, 4, 10);
    let previous = make_run_summary(true, 8, 4, 2);
    assert_eq!(
        compare_runs(&current, &previous),
        BenchmarkComparisonStatus::Improved
    );
}

// --- render_comparison_summary ---

#[test]
fn render_comparison_summary_improved() {
    let current = make_run_summary(true, 9, 5, 4);
    let previous = make_run_summary(false, 7, 3, 2);
    let delta = BenchmarkComparisonDelta {
        correctness_checks_passed: 2,
        unnecessary_action_count: None,
        retry_count: None,
        exported_memory_records: 2,
        exported_evidence_records: 2,
    };
    let summary = render_comparison_summary(
        BenchmarkComparisonStatus::Improved,
        &current,
        &previous,
        &delta,
    );
    assert!(summary.contains("improved"));
    assert!(summary.contains(&current.session_id));
    assert!(summary.contains(&previous.session_id));
}

#[test]
fn render_comparison_summary_regressed() {
    let current = make_run_summary(false, 5, 2, 1);
    let previous = make_run_summary(true, 8, 4, 3);
    let delta = BenchmarkComparisonDelta {
        correctness_checks_passed: -3,
        unnecessary_action_count: None,
        retry_count: None,
        exported_memory_records: -2,
        exported_evidence_records: -2,
    };
    let summary = render_comparison_summary(
        BenchmarkComparisonStatus::Regressed,
        &current,
        &previous,
        &delta,
    );
    assert!(summary.contains("regressed"));
}

#[test]
fn render_comparison_summary_unchanged() {
    let a = make_run_summary(true, 8, 4, 3);
    let b = make_run_summary(true, 8, 4, 3);
    let delta = BenchmarkComparisonDelta {
        correctness_checks_passed: 0,
        unnecessary_action_count: None,
        retry_count: None,
        exported_memory_records: 0,
        exported_evidence_records: 0,
    };
    let summary = render_comparison_summary(BenchmarkComparisonStatus::Unchanged, &a, &b, &delta);
    assert!(summary.contains("matched"));
}

// --- evidence_quality_rank: edge cases ---

#[test]
fn evidence_quality_rank_empty_string() {
    assert_eq!(evidence_quality_rank(""), 0);
}

#[test]
fn evidence_quality_rank_case_sensitive() {
    assert_eq!(evidence_quality_rank("Sufficient"), 0);
    assert_eq!(evidence_quality_rank("THIN"), 0);
}

// --- render_benchmark_delta: large values ---

#[test]
fn render_benchmark_delta_large_positive() {
    assert_eq!(render_benchmark_delta(Some(999999)), "+999999");
}

#[test]
fn render_benchmark_delta_large_negative() {
    assert_eq!(render_benchmark_delta(Some(-123456)), "-123456");
}
