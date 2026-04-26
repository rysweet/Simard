use super::reporting::*;
use crate::gym::types::{
    BenchmarkArtifactPaths, BenchmarkCheckResult, BenchmarkClass, BenchmarkComparisonArtifactPaths,
    BenchmarkComparisonDelta, BenchmarkComparisonReport, BenchmarkComparisonRunSummary,
    BenchmarkComparisonStatus, BenchmarkHandoffReport, BenchmarkRunReport, BenchmarkRuntimeReport,
    BenchmarkScenario, BenchmarkScorecard,
};
use crate::runtime::RuntimeTopology;
use std::path::PathBuf;
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
