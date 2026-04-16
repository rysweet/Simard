use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};

use super::types::{
    BenchmarkComparisonReport, BenchmarkComparisonRunSummary, BenchmarkComparisonStatus,
    BenchmarkRunReport,
};

#[derive(Clone, Debug, Deserialize)]
pub(super) struct StoredBenchmarkScenario {
    pub(super) id: String,
    pub(super) title: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct StoredBenchmarkScorecard {
    pub(super) correctness_checks_passed: usize,
    pub(super) correctness_checks_total: usize,
    pub(super) evidence_quality: String,
    #[serde(default)]
    pub(super) unnecessary_action_count: Option<u32>,
    #[serde(default)]
    pub(super) retry_count: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct StoredBenchmarkHandoffReport {
    pub(super) exported_memory_records: usize,
    pub(super) exported_evidence_records: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct StoredBenchmarkRunReport {
    pub(super) suite_id: String,
    pub(super) scenario: StoredBenchmarkScenario,
    pub(super) session_id: String,
    pub(super) run_started_at_unix_ms: u128,
    pub(super) passed: bool,
    pub(super) scorecard: StoredBenchmarkScorecard,
    pub(super) handoff: StoredBenchmarkHandoffReport,
}

#[derive(Clone, Debug)]
pub(super) struct StoredBenchmarkRunArtifact {
    pub(super) report_path: PathBuf,
    pub(super) report: StoredBenchmarkRunReport,
}

pub(super) fn render_text_report(report: &BenchmarkRunReport) -> String {
    let mut lines = vec![
        format!("Suite: {}", report.suite_id),
        format!(
            "Scenario: {} ({})",
            report.scenario.id, report.scenario.title
        ),
        format!("Passed: {}", report.passed),
        format!("Identity: {}", report.runtime.identity),
        format!("Base type: {}", report.runtime.selected_base_type),
        format!("Topology: {}", report.runtime.topology),
        format!(
            "Checks passed: {}/{}",
            report.scorecard.correctness_checks_passed, report.scorecard.correctness_checks_total
        ),
        format!(
            "Unnecessary actions: {}",
            render_benchmark_count(report.scorecard.unnecessary_action_count)
        ),
        format!(
            "Retry count: {}",
            render_benchmark_count(report.scorecard.retry_count)
        ),
        format!("Plan: {}", report.plan),
        format!("Execution summary: {}", report.execution_summary),
        format!("Reflection summary: {}", report.reflection_summary),
        format!("Review artifact: {}", report.artifacts.review_json),
        "Checks:".to_string(),
    ];
    for check in &report.checks {
        lines.push(format!(
            "- {}: {} ({})",
            check.id,
            if check.passed { "passed" } else { "failed" },
            check.detail
        ));
    }
    if !report.scorecard.human_review_notes.is_empty() {
        lines.push("Human review notes:".to_string());
        for note in &report.scorecard.human_review_notes {
            lines.push(format!("- {note}"));
        }
    }
    lines.join("\n")
}

pub(super) fn render_text_comparison_report(report: &BenchmarkComparisonReport) -> String {
    [
        format!(
            "Scenario: {} ({})",
            report.scenario_id, report.scenario_title
        ),
        format!("Comparison status: {}", report.status),
        format!("Summary: {}", report.summary),
        format!("Current session: {}", report.current.session_id),
        format!("Current report: {}", report.current.report_json),
        format!(
            "Current unnecessary actions: {}",
            render_benchmark_count(report.current.unnecessary_action_count)
        ),
        format!(
            "Current retry count: {}",
            render_benchmark_count(report.current.retry_count)
        ),
        format!(
            "Current checks passed: {}/{}",
            report.current.correctness_checks_passed, report.current.correctness_checks_total
        ),
        format!("Previous session: {}", report.previous.session_id),
        format!("Previous report: {}", report.previous.report_json),
        format!(
            "Previous unnecessary actions: {}",
            render_benchmark_count(report.previous.unnecessary_action_count)
        ),
        format!(
            "Previous retry count: {}",
            render_benchmark_count(report.previous.retry_count)
        ),
        format!(
            "Previous checks passed: {}/{}",
            report.previous.correctness_checks_passed, report.previous.correctness_checks_total
        ),
        format!(
            "Delta correctness checks passed: {:+}",
            report.delta.correctness_checks_passed
        ),
        format!(
            "Delta unnecessary actions: {}",
            render_benchmark_delta(report.delta.unnecessary_action_count)
        ),
        format!(
            "Delta retry count: {}",
            render_benchmark_delta(report.delta.retry_count)
        ),
        format!(
            "Delta exported memory records: {:+}",
            report.delta.exported_memory_records
        ),
        format!(
            "Delta exported evidence records: {:+}",
            report.delta.exported_evidence_records
        ),
    ]
    .join("\n")
}

pub(crate) fn render_benchmark_count(value: Option<u32>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "unmeasured".to_string(),
    }
}

pub(crate) fn render_benchmark_delta(value: Option<i64>) -> String {
    match value {
        Some(value) => format!("{value:+}"),
        None => "unmeasured".to_string(),
    }
}

pub(super) fn render_comparison_summary(
    status: BenchmarkComparisonStatus,
    current: &BenchmarkComparisonRunSummary,
    previous: &BenchmarkComparisonRunSummary,
    delta: &super::types::BenchmarkComparisonDelta,
) -> String {
    let unnecessary_action_delta = render_benchmark_delta(delta.unnecessary_action_count);
    let retry_delta = render_benchmark_delta(delta.retry_count);
    match status {
        BenchmarkComparisonStatus::Improved => format!(
            "latest run improved from session '{}' to '{}' with check delta {:+}, unnecessary-action delta {}, retry delta {}, memory delta {:+}, and evidence delta {:+}",
            previous.session_id,
            current.session_id,
            delta.correctness_checks_passed,
            unnecessary_action_delta,
            retry_delta,
            delta.exported_memory_records,
            delta.exported_evidence_records
        ),
        BenchmarkComparisonStatus::Regressed => format!(
            "latest run regressed from session '{}' to '{}' with check delta {:+}, unnecessary-action delta {}, retry delta {}, memory delta {:+}, and evidence delta {:+}",
            previous.session_id,
            current.session_id,
            delta.correctness_checks_passed,
            unnecessary_action_delta,
            retry_delta,
            delta.exported_memory_records,
            delta.exported_evidence_records
        ),
        BenchmarkComparisonStatus::Unchanged => format!(
            "latest run matched session '{}' on pass/fail status and checks, with unnecessary-action delta {}, retry delta {}, memory delta {:+}, and evidence delta {:+}",
            previous.session_id,
            unnecessary_action_delta,
            retry_delta,
            delta.exported_memory_records,
            delta.exported_evidence_records
        ),
    }
}

pub(super) fn compare_runs(
    current: &BenchmarkComparisonRunSummary,
    previous: &BenchmarkComparisonRunSummary,
) -> BenchmarkComparisonStatus {
    if current.passed != previous.passed {
        return if current.passed {
            BenchmarkComparisonStatus::Improved
        } else {
            BenchmarkComparisonStatus::Regressed
        };
    }
    if current.correctness_checks_passed != previous.correctness_checks_passed {
        return if current.correctness_checks_passed > previous.correctness_checks_passed {
            BenchmarkComparisonStatus::Improved
        } else {
            BenchmarkComparisonStatus::Regressed
        };
    }
    if current.exported_evidence_records != previous.exported_evidence_records {
        return if current.exported_evidence_records > previous.exported_evidence_records {
            BenchmarkComparisonStatus::Improved
        } else {
            BenchmarkComparisonStatus::Regressed
        };
    }
    if current.exported_memory_records != previous.exported_memory_records {
        return if current.exported_memory_records > previous.exported_memory_records {
            BenchmarkComparisonStatus::Improved
        } else {
            BenchmarkComparisonStatus::Regressed
        };
    }
    if let Some(status) = compare_lower_is_better(
        current.unnecessary_action_count,
        previous.unnecessary_action_count,
    ) {
        return status;
    }
    if let Some(status) = compare_lower_is_better(current.retry_count, previous.retry_count) {
        return status;
    }
    match evidence_quality_rank(&current.evidence_quality)
        .cmp(&evidence_quality_rank(&previous.evidence_quality))
    {
        std::cmp::Ordering::Greater => BenchmarkComparisonStatus::Improved,
        std::cmp::Ordering::Less => BenchmarkComparisonStatus::Regressed,
        std::cmp::Ordering::Equal => BenchmarkComparisonStatus::Unchanged,
    }
}

pub(super) fn evidence_quality_rank(value: &str) -> u8 {
    match value {
        "sufficient" => 2,
        "thin" => 1,
        _ => 0,
    }
}

pub(super) fn compare_lower_is_better(
    current: Option<u32>,
    previous: Option<u32>,
) -> Option<BenchmarkComparisonStatus> {
    match (current, previous) {
        (Some(current), Some(previous)) if current != previous => Some(if current < previous {
            BenchmarkComparisonStatus::Improved
        } else {
            BenchmarkComparisonStatus::Regressed
        }),
        _ => None,
    }
}

pub(super) fn benchmark_count_delta(current: Option<u32>, previous: Option<u32>) -> Option<i64> {
    match (current, previous) {
        (Some(current), Some(previous)) => Some(current as i64 - previous as i64),
        _ => None,
    }
}

pub(super) fn summarize_stored_run(
    run: &StoredBenchmarkRunArtifact,
) -> BenchmarkComparisonRunSummary {
    BenchmarkComparisonRunSummary {
        suite_id: run.report.suite_id.clone(),
        session_id: run.report.session_id.clone(),
        run_started_at_unix_ms: run.report.run_started_at_unix_ms,
        passed: run.report.passed,
        correctness_checks_passed: run.report.scorecard.correctness_checks_passed,
        correctness_checks_total: run.report.scorecard.correctness_checks_total,
        evidence_quality: run.report.scorecard.evidence_quality.clone(),
        unnecessary_action_count: run.report.scorecard.unnecessary_action_count,
        retry_count: run.report.scorecard.retry_count,
        exported_memory_records: run.report.handoff.exported_memory_records,
        exported_evidence_records: run.report.handoff.exported_evidence_records,
        report_json: display_path(&run.report_path),
    }
}

pub(super) fn create_dir_all(path: &Path) -> SimardResult<()> {
    fs::create_dir_all(path).map_err(|error| SimardError::ArtifactIo {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

pub(super) fn write_json<T>(path: &Path, value: &T) -> SimardResult<()>
where
    T: Serialize,
{
    let json = serde_json::to_string_pretty(value).map_err(|error| SimardError::ArtifactIo {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    write_text(path, format!("{json}\n"))
}

pub(super) fn write_text(path: &Path, contents: String) -> SimardResult<()> {
    fs::write(path, contents).map_err(|error| SimardError::ArtifactIo {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

pub(super) fn load_scenario_run_reports(
    scenario_id: &str,
    output_root: &Path,
) -> SimardResult<Vec<StoredBenchmarkRunArtifact>> {
    let scenario_dir = output_root.join(scenario_id);
    let entries = match fs::read_dir(&scenario_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(SimardError::ArtifactIo {
                path: scenario_dir,
                reason: error.to_string(),
            });
        }
    };
    let mut reports = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| SimardError::ArtifactIo {
            path: scenario_dir.clone(),
            reason: error.to_string(),
        })?;
        let report_path = entry.path().join("report.json");
        if !report_path.is_file() {
            continue;
        }
        let report = load_stored_run_report(&report_path)?;
        if report.scenario.id == scenario_id {
            reports.push(StoredBenchmarkRunArtifact {
                report_path,
                report,
            });
        }
    }
    Ok(reports)
}

fn load_stored_run_report(path: &Path) -> SimardResult<StoredBenchmarkRunReport> {
    let raw = fs::read_to_string(path).map_err(|error| SimardError::ArtifactIo {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    serde_json::from_str(&raw).map_err(|error| SimardError::ArtifactIo {
        path: path.to_path_buf(),
        reason: format!("invalid benchmark report JSON: {error}"),
    })
}

pub(super) fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(super) fn now_unix_ms() -> SimardResult<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| SimardError::ClockBeforeUnixEpoch {
            reason: error.to_string(),
        })?
        .as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let summary = render_comparison_summary(
            BenchmarkComparisonStatus::Unchanged,
            &a,
            &b,
            &delta,
        );
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
}
