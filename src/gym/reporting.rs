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
struct StoredBenchmarkScorecard {
    correctness_checks_passed: usize,
    correctness_checks_total: usize,
    evidence_quality: String,
    #[serde(default)]
    unnecessary_action_count: Option<u32>,
    #[serde(default)]
    retry_count: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
struct StoredBenchmarkHandoffReport {
    exported_memory_records: usize,
    exported_evidence_records: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct StoredBenchmarkRunReport {
    pub(super) suite_id: String,
    pub(super) scenario: StoredBenchmarkScenario,
    pub(super) session_id: String,
    pub(super) run_started_at_unix_ms: u128,
    pub(super) passed: bool,
    scorecard: StoredBenchmarkScorecard,
    handoff: StoredBenchmarkHandoffReport,
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

fn evidence_quality_rank(value: &str) -> u8 {
    match value {
        "sufficient" => 2,
        "thin" => 1,
        _ => 0,
    }
}

fn compare_lower_is_better(
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
    use std::path::PathBuf;

    use super::*;
    use crate::gym::types::{
        BenchmarkArtifactPaths, BenchmarkCheckResult, BenchmarkClass,
        BenchmarkComparisonArtifactPaths, BenchmarkComparisonDelta, BenchmarkHandoffReport,
        BenchmarkRuntimeReport, BenchmarkScenario, BenchmarkScorecard,
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

    #[test]
    fn render_comparison_summary_unchanged() {
        let current = make_summary(true, 5, 5, 5, Some(0), Some(0), "sufficient");
        let previous = make_summary(true, 5, 5, 5, Some(0), Some(0), "sufficient");
        let delta = BenchmarkComparisonDelta {
            correctness_checks_passed: 0,
            unnecessary_action_count: Some(0),
            retry_count: Some(0),
            exported_memory_records: 0,
            exported_evidence_records: 0,
        };
        let summary = render_comparison_summary(
            BenchmarkComparisonStatus::Unchanged,
            &current,
            &previous,
            &delta,
        );
        assert!(summary.contains("matched"));
        assert!(summary.contains(&previous.session_id));
    }

    // --- summarize_stored_run ---

    #[test]
    fn summarize_stored_run_maps_fields_correctly() {
        let artifact = StoredBenchmarkRunArtifact {
            report_path: PathBuf::from("/output/scenario-1/session-abc/report.json"),
            report: StoredBenchmarkRunReport {
                suite_id: "starter".to_string(),
                scenario: StoredBenchmarkScenario {
                    id: "scenario-1".to_string(),
                    title: "Scenario One".to_string(),
                },
                session_id: "session-abc".to_string(),
                run_started_at_unix_ms: 5000,
                passed: true,
                scorecard: StoredBenchmarkScorecard {
                    correctness_checks_passed: 7,
                    correctness_checks_total: 10,
                    evidence_quality: "sufficient".to_string(),
                    unnecessary_action_count: Some(2),
                    retry_count: None,
                },
                handoff: StoredBenchmarkHandoffReport {
                    exported_memory_records: 12,
                    exported_evidence_records: 8,
                },
            },
        };
        let summary = summarize_stored_run(&artifact);
        assert_eq!(summary.suite_id, "starter");
        assert_eq!(summary.session_id, "session-abc");
        assert_eq!(summary.run_started_at_unix_ms, 5000);
        assert!(summary.passed);
        assert_eq!(summary.correctness_checks_passed, 7);
        assert_eq!(summary.correctness_checks_total, 10);
        assert_eq!(summary.evidence_quality, "sufficient");
        assert_eq!(summary.unnecessary_action_count, Some(2));
        assert_eq!(summary.retry_count, None);
        assert_eq!(summary.exported_memory_records, 12);
        assert_eq!(summary.exported_evidence_records, 8);
        assert!(summary.report_json.contains("report.json"));
    }

    // --- render_text_report ---

    fn make_test_run_report() -> BenchmarkRunReport {
        BenchmarkRunReport {
            suite_id: "starter".to_string(),
            scenario: BenchmarkScenario {
                id: "test-scenario",
                title: "Test Scenario",
                description: "A test scenario",
                class: BenchmarkClass::RepoExploration,
                identity: "test-identity",
                base_type: "local-harness",
                topology: RuntimeTopology::SingleProcess,
                objective: "Test objective",
                expected_min_runtime_evidence: 3,
            },
            session_id: "session-001".to_string(),
            run_started_at_unix_ms: 1000,
            passed: true,
            checks: vec![
                BenchmarkCheckResult {
                    id: "check-alpha".to_string(),
                    passed: true,
                    detail: "looks good".to_string(),
                },
                BenchmarkCheckResult {
                    id: "check-beta".to_string(),
                    passed: false,
                    detail: "missing evidence".to_string(),
                },
            ],
            scorecard: BenchmarkScorecard {
                task_completed: true,
                evidence_quality: "sufficient".to_string(),
                correctness_checks_passed: 1,
                correctness_checks_total: 2,
                unnecessary_action_count: Some(3),
                retry_count: None,
                human_review_notes: vec!["review note one".to_string()],
                measurement_notes: vec![],
            },
            plan: "Test plan text".to_string(),
            execution_summary: "Test execution text".to_string(),
            reflection_summary: "Test reflection text".to_string(),
            benchmark_memory_key: "mem-key".to_string(),
            benchmark_evidence_id: "evi-id".to_string(),
            runtime: BenchmarkRuntimeReport {
                identity: "test-identity".to_string(),
                selected_base_type: "local-harness".to_string(),
                topology: "single-process".to_string(),
                adapter_implementation: "test-adapter".to_string(),
                topology_backend: "loopback-topo".to_string(),
                transport_backend: "loopback-transport".to_string(),
                supervisor_backend: "coordinated".to_string(),
                runtime_node: "node-1".to_string(),
                mailbox_address: "addr-1".to_string(),
                snapshot_state_before_stop: "ready".to_string(),
                snapshot_state_after_stop: "stopped".to_string(),
            },
            handoff: BenchmarkHandoffReport {
                exported_state: "stopped".to_string(),
                exported_memory_records: 5,
                exported_evidence_records: 4,
                restored_runtime_state: "ready".to_string(),
                restored_session_phase: Some("complete".to_string()),
                restored_session_objective: Some("test objective".to_string()),
            },
            artifacts: BenchmarkArtifactPaths {
                run_dir: "/output/run".to_string(),
                report_json: "/output/run/report.json".to_string(),
                report_txt: "/output/run/report.txt".to_string(),
                review_json: "/output/run/review.json".to_string(),
            },
        }
    }

    #[test]
    fn render_text_report_contains_key_fields() {
        let report = make_test_run_report();
        let text = render_text_report(&report);
        assert!(text.contains("Suite: starter"));
        assert!(text.contains("Scenario: test-scenario (Test Scenario)"));
        assert!(text.contains("Passed: true"));
        assert!(text.contains("Identity: test-identity"));
        assert!(text.contains("Base type: local-harness"));
        assert!(text.contains("Topology: single-process"));
        assert!(text.contains("Checks passed: 1/2"));
        assert!(text.contains("Unnecessary actions: 3"));
        assert!(text.contains("Retry count: unmeasured"));
        assert!(text.contains("Plan: Test plan text"));
        assert!(text.contains("Execution summary: Test execution text"));
        assert!(text.contains("Reflection summary: Test reflection text"));
    }

    #[test]
    fn render_text_report_contains_check_details() {
        let report = make_test_run_report();
        let text = render_text_report(&report);
        assert!(text.contains("- check-alpha: passed (looks good)"));
        assert!(text.contains("- check-beta: failed (missing evidence)"));
    }

    #[test]
    fn render_text_report_contains_human_review_notes() {
        let report = make_test_run_report();
        let text = render_text_report(&report);
        assert!(text.contains("Human review notes:"));
        assert!(text.contains("- review note one"));
    }

    #[test]
    fn render_text_report_omits_human_notes_when_empty() {
        let mut report = make_test_run_report();
        report.scorecard.human_review_notes.clear();
        let text = render_text_report(&report);
        assert!(!text.contains("Human review notes:"));
    }

    // --- render_text_comparison_report ---

    fn make_test_comparison_report() -> BenchmarkComparisonReport {
        BenchmarkComparisonReport {
            scenario_id: "test-scenario".to_string(),
            scenario_title: "Test Scenario".to_string(),
            status: BenchmarkComparisonStatus::Improved,
            summary: "improved run".to_string(),
            current: BenchmarkComparisonRunSummary {
                suite_id: "starter".to_string(),
                session_id: "session-new".to_string(),
                run_started_at_unix_ms: 2000,
                passed: true,
                correctness_checks_passed: 8,
                correctness_checks_total: 10,
                evidence_quality: "sufficient".to_string(),
                unnecessary_action_count: Some(1),
                retry_count: Some(0),
                exported_memory_records: 10,
                exported_evidence_records: 8,
                report_json: "/output/new/report.json".to_string(),
            },
            previous: BenchmarkComparisonRunSummary {
                suite_id: "starter".to_string(),
                session_id: "session-old".to_string(),
                run_started_at_unix_ms: 1000,
                passed: true,
                correctness_checks_passed: 5,
                correctness_checks_total: 10,
                evidence_quality: "thin".to_string(),
                unnecessary_action_count: Some(3),
                retry_count: Some(2),
                exported_memory_records: 6,
                exported_evidence_records: 4,
                report_json: "/output/old/report.json".to_string(),
            },
            delta: BenchmarkComparisonDelta {
                correctness_checks_passed: 3,
                unnecessary_action_count: Some(-2),
                retry_count: Some(-2),
                exported_memory_records: 4,
                exported_evidence_records: 4,
            },
            artifact_paths: BenchmarkComparisonArtifactPaths {
                report_json: "/cmp/report.json".to_string(),
                report_txt: "/cmp/report.txt".to_string(),
            },
        }
    }

    #[test]
    fn render_text_comparison_report_contains_key_sections() {
        let report = make_test_comparison_report();
        let text = render_text_comparison_report(&report);
        assert!(text.contains("Scenario: test-scenario (Test Scenario)"));
        assert!(text.contains("Comparison status: improved"));
        assert!(text.contains("Summary: improved run"));
        assert!(text.contains("Current session: session-new"));
        assert!(text.contains("Previous session: session-old"));
        assert!(text.contains("Delta correctness checks passed: +3"));
        assert!(text.contains("Delta unnecessary actions: -2"));
        assert!(text.contains("Delta retry count: -2"));
        assert!(text.contains("Delta exported memory records: +4"));
        assert!(text.contains("Delta exported evidence records: +4"));
    }

    #[test]
    fn render_text_comparison_report_shows_per_run_details() {
        let report = make_test_comparison_report();
        let text = render_text_comparison_report(&report);
        assert!(text.contains("Current checks passed: 8/10"));
        assert!(text.contains("Previous checks passed: 5/10"));
        assert!(text.contains("Current unnecessary actions: 1"));
        assert!(text.contains("Previous unnecessary actions: 3"));
    }

    // --- File I/O ---

    #[test]
    fn write_text_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        write_text(&file, "hello world".to_string()).unwrap();
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn write_json_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.json");
        let value = serde_json::json!({"key": "value", "num": 42});
        write_json(&file, &value).unwrap();
        let content = std::fs::read_to_string(&file).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["key"], "value");
        assert_eq!(parsed["num"], 42);
    }

    #[test]
    fn create_dir_all_creates_nested_directories() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("c");
        create_dir_all(&nested).unwrap();
        assert!(nested.is_dir());
    }

    #[test]
    fn load_scenario_run_reports_returns_empty_for_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let reports = load_scenario_run_reports("nonexistent", dir.path()).unwrap();
        assert!(reports.is_empty());
    }

    #[test]
    fn load_scenario_run_reports_finds_valid_reports() {
        let dir = tempfile::tempdir().unwrap();
        let scenario_dir = dir.path().join("test-scenario").join("run-1");
        std::fs::create_dir_all(&scenario_dir).unwrap();

        let report_json = serde_json::json!({
            "suite_id": "starter",
            "scenario": {"id": "test-scenario", "title": "Test"},
            "session_id": "session-abc",
            "run_started_at_unix_ms": 1000u64,
            "passed": true,
            "scorecard": {
                "correctness_checks_passed": 5,
                "correctness_checks_total": 8,
                "evidence_quality": "sufficient"
            },
            "handoff": {
                "exported_memory_records": 3,
                "exported_evidence_records": 4
            }
        });
        std::fs::write(
            scenario_dir.join("report.json"),
            serde_json::to_string_pretty(&report_json).unwrap(),
        )
        .unwrap();

        let reports = load_scenario_run_reports("test-scenario", dir.path()).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].report.session_id, "session-abc");
        assert!(reports[0].report.passed);
    }

    #[test]
    fn load_scenario_run_reports_skips_dirs_without_report_json() {
        let dir = tempfile::tempdir().unwrap();
        let scenario_dir = dir.path().join("test-scenario").join("run-empty");
        std::fs::create_dir_all(&scenario_dir).unwrap();

        let reports = load_scenario_run_reports("test-scenario", dir.path()).unwrap();
        assert!(reports.is_empty());
    }

    #[test]
    fn load_scenario_run_reports_skips_mismatched_scenario_ids() {
        let dir = tempfile::tempdir().unwrap();
        let scenario_dir = dir.path().join("scenario-a").join("run-1");
        std::fs::create_dir_all(&scenario_dir).unwrap();

        let report_json = serde_json::json!({
            "suite_id": "starter",
            "scenario": {"id": "scenario-b", "title": "Wrong"},
            "session_id": "session-xyz",
            "run_started_at_unix_ms": 2000u64,
            "passed": false,
            "scorecard": {
                "correctness_checks_passed": 0,
                "correctness_checks_total": 5,
                "evidence_quality": "thin"
            },
            "handoff": {
                "exported_memory_records": 0,
                "exported_evidence_records": 0
            }
        });
        std::fs::write(
            scenario_dir.join("report.json"),
            serde_json::to_string_pretty(&report_json).unwrap(),
        )
        .unwrap();

        let reports = load_scenario_run_reports("scenario-a", dir.path()).unwrap();
        assert!(reports.is_empty());
    }
}
