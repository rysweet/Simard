use super::*;

fn rec(scenario: &str, score: f64, ts: i64) -> ScoreRecord {
    ScoreRecord {
        suite_id: "progressive".into(),
        scenario_id: scenario.into(),
        score,
        timestamp: ts,
        commit_hash: None,
    }
}

fn mem_history() -> ScoreHistory {
    ScoreHistory::open(":memory:").unwrap()
}

#[test]
fn score_record_construction() {
    let r = ScoreRecord {
        suite_id: "s1".into(),
        scenario_id: "sc1".into(),
        score: 0.85,
        timestamp: 1_700_000_000,
        commit_hash: Some("abc123".into()),
    };
    assert_eq!(r.suite_id, "s1");
    assert_eq!(r.score, 0.85);
    assert_eq!(r.commit_hash.as_deref(), Some("abc123"));
}

#[test]
fn open_creates_schema() {
    let _h = mem_history();
}

#[test]
fn record_and_latest() {
    let h = mem_history();
    h.record(&rec("L1", 0.9, 100)).unwrap();
    let got = h.latest("progressive", "L1").unwrap();
    assert_eq!(got.score, 0.9);
    assert_eq!(got.timestamp, 100);
}

#[test]
fn latest_returns_newest() {
    let h = mem_history();
    h.record(&rec("L1", 0.5, 1)).unwrap();
    h.record(&rec("L1", 0.9, 2)).unwrap();
    assert_eq!(h.latest("progressive", "L1").unwrap().score, 0.9);
}

#[test]
fn latest_missing() {
    let h = mem_history();
    assert!(h.latest("progressive", "nonexistent").is_none());
}

#[test]
fn history_order_and_limit() {
    let h = mem_history();
    for i in 1..=5 {
        h.record(&rec("L1", i as f64 * 0.1, i)).unwrap();
    }
    let rows = h.history("progressive", "L1", 3).unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows[0].timestamp < rows[1].timestamp);
    assert!(rows[1].timestamp < rows[2].timestamp);
    assert_eq!(rows[2].score, 0.5);
}

#[test]
fn detect_regression_cases() {
    assert!(detect_regression(0.5, 0.8, 0.1));
    assert!(!detect_regression(0.8, 0.5, 0.1));
    assert!(!detect_regression(0.79, 0.8, 0.1));
    assert!(detect_regression(0.69, 0.8, 0.1));
    // Exact threshold boundary: drop of exactly 0.25, threshold 0.25 → not >
    assert!(!detect_regression(0.75, 1.0, 0.25));
}

#[test]
fn check_promotion_consecutive() {
    let recs: Vec<ScoreRecord> = (1..=4)
        .map(|i| rec("L1", 0.5 + i as f64 * 0.1, i))
        .collect();
    assert!(check_promotion(&recs, 3));
}

#[test]
fn check_promotion_broken_streak() {
    let recs = vec![
        rec("L1", 0.5, 1),
        rec("L1", 0.6, 2),
        rec("L1", 0.55, 3), // regression breaks streak
        rec("L1", 0.7, 4),
    ];
    assert!(!check_promotion(&recs, 3));
}

#[test]
fn check_promotion_insufficient_history() {
    let recs = vec![rec("L1", 0.5, 1), rec("L1", 0.6, 2)];
    assert!(!check_promotion(&recs, 3));
}

#[test]
fn check_promotion_zero_consecutive() {
    let recs = vec![rec("L1", 0.5, 1)];
    assert!(!check_promotion(&recs, 0));
}

#[test]
fn generate_signals_types() {
    let h = mem_history();
    // Scenario A: improving
    h.record(&rec("A", 0.5, 1)).unwrap();
    h.record(&rec("A", 0.7, 2)).unwrap();

    // Scenario B: regressing
    h.record(&rec("B", 0.8, 1)).unwrap();
    h.record(&rec("B", 0.5, 2)).unwrap();

    // Scenario C: stable
    h.record(&rec("C", 0.8, 1)).unwrap();
    h.record(&rec("C", 0.805, 2)).unwrap();

    let sigs = generate_signals(&h, "progressive").unwrap();
    assert_eq!(sigs.len(), 3);

    let find = |id: &str| sigs.iter().find(|s| s.scenario_id == id).unwrap();
    assert!(matches!(find("A").signal, GymSignal::Improvement { .. }));
    assert!(matches!(find("B").signal, GymSignal::Regression { .. }));
    assert!(matches!(find("C").signal, GymSignal::Stable));
}

#[test]
fn generate_signals_promoted() {
    let h = mem_history();
    for i in 1..=5 {
        h.record(&rec("P", 0.5 + i as f64 * 0.05, i)).unwrap();
    }
    let sigs = generate_signals(&h, "progressive").unwrap();
    let sig = sigs.iter().find(|s| s.scenario_id == "P").unwrap();
    assert_eq!(sig.signal, GymSignal::Promoted);
}

#[test]
fn scenario_ids_listing() {
    let h = mem_history();
    h.record(&rec("X", 0.1, 1)).unwrap();
    h.record(&rec("Y", 0.2, 1)).unwrap();
    h.record(&rec("X", 0.3, 2)).unwrap();
    let ids = h.scenario_ids("progressive").unwrap();
    assert_eq!(ids, vec!["X", "Y"]);
}

#[test]
fn gym_signal_display() {
    assert_eq!(format!("{}", GymSignal::Stable), "stable");
    assert_eq!(format!("{}", GymSignal::Promoted), "promoted");
    assert!(format!("{}", GymSignal::Improvement { delta: 0.1 }).starts_with("improvement"));
}

#[test]
fn generate_signals_skips_single_record() {
    let h = mem_history();
    h.record(&rec("solo", 0.5, 1)).unwrap();
    let sigs = generate_signals(&h, "progressive").unwrap();
    assert!(sigs.is_empty());
}

#[test]
fn score_record_serde_round_trip() {
    let r = ScoreRecord {
        suite_id: "progressive".into(),
        scenario_id: "L3".into(),
        score: 0.876,
        timestamp: 1_700_000_042,
        commit_hash: Some("deadbeef".into()),
    };
    let json = serde_json::to_string(&r).unwrap();
    let parsed: ScoreRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(r, parsed);
}

#[test]
fn score_record_serde_none_commit() {
    let r = ScoreRecord {
        suite_id: "s".into(),
        scenario_id: "sc".into(),
        score: 0.5,
        timestamp: 100,
        commit_hash: None,
    };
    let json = serde_json::to_string(&r).unwrap();
    let parsed: ScoreRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.commit_hash, None);
}

#[test]
fn gym_signal_serde_round_trip() {
    for signal in [
        GymSignal::Improvement { delta: 0.15 },
        GymSignal::Regression { delta: -0.08 },
        GymSignal::Stable,
        GymSignal::Promoted,
    ] {
        let json = serde_json::to_string(&signal).unwrap();
        let parsed: GymSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(signal, parsed);
    }
}

#[test]
fn scenario_signal_serde_round_trip() {
    let sig = ScenarioSignal {
        scenario_id: "L5".into(),
        signal: GymSignal::Improvement { delta: 0.1 },
    };
    let json = serde_json::to_string(&sig).unwrap();
    let parsed: ScenarioSignal = serde_json::from_str(&json).unwrap();
    assert_eq!(sig, parsed);
}

#[test]
fn history_returns_empty_for_unknown_scenario() {
    let h = mem_history();
    h.record(&rec("L1", 0.5, 1)).unwrap();
    let rows = h.history("progressive", "nonexistent", 10).unwrap();
    assert!(rows.is_empty());
}

#[test]
fn generate_signals_empty_suite() {
    let h = mem_history();
    let sigs = generate_signals(&h, "nonexistent").unwrap();
    assert!(sigs.is_empty());
}

// ── BenchmarkRunReport intake tests ─────────────────────────────────

use crate::gym::{
    BenchmarkArtifactPaths, BenchmarkClass, BenchmarkHandoffReport, BenchmarkRunReport,
    BenchmarkRuntimeReport, BenchmarkScenario, BenchmarkScorecard,
};
use crate::gym_scoring::EvidenceQualityAssessment;
use crate::runtime::RuntimeTopology;

fn make_report(
    suite: &str,
    scenario_id: &'static str,
    passed: bool,
    checks_passed: usize,
    checks_total: usize,
    evidence: &str,
    ts_ms: u128,
) -> BenchmarkRunReport {
    BenchmarkRunReport {
        suite_id: suite.to_string(),
        scenario: BenchmarkScenario {
            id: scenario_id,
            title: "Test",
            description: "desc",
            class: BenchmarkClass::RepoExploration,
            identity: "test",
            base_type: "local-harness",
            topology: RuntimeTopology::SingleProcess,
            objective: "obj",
            expected_min_runtime_evidence: 1,
        },
        session_id: format!("session-{ts_ms}"),
        run_started_at_unix_ms: ts_ms,
        passed,
        checks: vec![],
        scorecard: BenchmarkScorecard {
            task_completed: passed,
            evidence_quality: evidence.to_string(),
            evidence_quality_assessment: EvidenceQualityAssessment::default(),
            correctness_checks_passed: checks_passed,
            correctness_checks_total: checks_total,
            unnecessary_action_count: None,
            retry_count: None,
            human_review_notes: vec![],
            measurement_notes: vec![],
        },
        plan: String::new(),
        execution_summary: String::new(),
        reflection_summary: String::new(),
        benchmark_memory_key: String::new(),
        benchmark_evidence_id: String::new(),
        runtime: BenchmarkRuntimeReport {
            identity: String::new(),
            selected_base_type: String::new(),
            topology: String::new(),
            adapter_implementation: String::new(),
            topology_backend: String::new(),
            transport_backend: String::new(),
            supervisor_backend: String::new(),
            runtime_node: String::new(),
            mailbox_address: String::new(),
            snapshot_state_before_stop: String::new(),
            snapshot_state_after_stop: String::new(),
        },
        handoff: BenchmarkHandoffReport {
            exported_state: String::new(),
            exported_memory_records: 0,
            exported_evidence_records: 0,
            restored_runtime_state: String::new(),
            restored_session_phase: None,
            restored_session_objective: None,
        },
        artifacts: BenchmarkArtifactPaths {
            run_dir: String::new(),
            report_json: String::new(),
            report_txt: String::new(),
            review_json: String::new(),
        },
    }
}

#[test]
fn score_from_benchmark_report_derives_pass_rate() {
    let report = make_report("s1", "sc1", true, 7, 10, "sufficient", 1_700_000_000_000);
    let record = score_from_benchmark_report(&report, Some("abc123".into()));
    assert_eq!(record.suite_id, "s1");
    assert_eq!(record.scenario_id, "sc1");
    assert!((record.score - 0.7).abs() < 1e-9);
    assert_eq!(record.timestamp, 1_700_000_000);
    assert_eq!(record.commit_hash.as_deref(), Some("abc123"));
}

#[test]
fn score_from_benchmark_report_zero_checks() {
    let report = make_report("s1", "sc1", false, 0, 0, "thin", 1000);
    let record = score_from_benchmark_report(&report, None);
    assert_eq!(record.score, 0.0);
}

#[test]
fn score_from_benchmark_report_perfect_score() {
    let report = make_report("s1", "sc1", true, 8, 8, "sufficient", 2000);
    let record = score_from_benchmark_report(&report, None);
    assert!((record.score - 1.0).abs() < 1e-9);
}

#[test]
fn record_benchmark_run_persists_and_returns() {
    let h = mem_history();
    let report = make_report("suite-a", "L1", true, 6, 8, "sufficient", 5_000_000);
    let record = record_benchmark_run(&h, &report, None).unwrap();
    assert!((record.score - 0.75).abs() < 1e-9);
    let latest = h.latest("suite-a", "L1").unwrap();
    assert!((latest.score - 0.75).abs() < 1e-9);
}

#[test]
fn record_benchmark_run_enables_signal_generation() {
    let h = mem_history();
    let r1 = make_report("suite-b", "L2", true, 5, 10, "thin", 1_000_000);
    let r2 = make_report("suite-b", "L2", true, 9, 10, "sufficient", 2_000_000);
    record_benchmark_run(&h, &r1, None).unwrap();
    record_benchmark_run(&h, &r2, None).unwrap();
    let sigs = generate_signals(&h, "suite-b").unwrap();
    assert_eq!(sigs.len(), 1);
    assert!(matches!(sigs[0].signal, GymSignal::Improvement { .. }));
}

#[test]
fn record_benchmark_run_detects_regression_signal() {
    let h = mem_history();
    let r1 = make_report("suite-c", "L3", true, 9, 10, "sufficient", 1_000_000);
    let r2 = make_report("suite-c", "L3", false, 3, 10, "thin", 2_000_000);
    record_benchmark_run(&h, &r1, None).unwrap();
    record_benchmark_run(&h, &r2, None).unwrap();
    let sigs = generate_signals(&h, "suite-c").unwrap();
    assert_eq!(sigs.len(), 1);
    assert!(matches!(sigs[0].signal, GymSignal::Regression { .. }));
}
