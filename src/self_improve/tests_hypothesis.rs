//! Tests for [`super::hypothesis`] — verifies that benchmark/session failure
//! signals get turned into [`super::hypothesis::ImprovementHypothesis`] records
//! carrying the typed evidence required by `Specs/ProductArchitecture.md`
//! lines 679–681 and 696.

use super::hypothesis::{
    aggregate_hypotheses, form_hypotheses_from_benchmark_reports, form_hypotheses_from_review,
    form_hypotheses_from_session_failures, form_hypotheses_from_signals,
    form_hypotheses_from_weak_dimensions,
};
use super::types::WeakDimension;
use crate::gym::{
    BenchmarkArtifactPaths, BenchmarkCheckResult, BenchmarkClass, BenchmarkHandoffReport,
    BenchmarkRunReport, BenchmarkRuntimeReport, BenchmarkScenario, BenchmarkScorecard,
};
use crate::gym_history::{GymSignal, ScenarioSignal};
use crate::improvements::EvidenceRef;
use crate::review::{ImprovementProposal, ReviewArtifact, ReviewEvidenceSummary, ReviewTargetKind};
use crate::runtime::RuntimeTopology;

fn synthetic_scenario(id: &'static str) -> BenchmarkScenario {
    BenchmarkScenario {
        id,
        title: "title",
        description: "desc",
        class: BenchmarkClass::RepoExploration,
        identity: "simard-engineer",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "objective",
        expected_min_runtime_evidence: 1,
    }
}

fn synthetic_report(
    suite_id: &str,
    scenario_id: &'static str,
    passed: bool,
    failed_check_ids: &[&str],
) -> BenchmarkRunReport {
    let checks: Vec<BenchmarkCheckResult> = failed_check_ids
        .iter()
        .map(|id| BenchmarkCheckResult {
            id: id.to_string(),
            passed: false,
            detail: format!("{id} did not satisfy invariant"),
        })
        .collect();

    BenchmarkRunReport {
        suite_id: suite_id.to_string(),
        scenario: synthetic_scenario(scenario_id),
        session_id: format!("sess-{scenario_id}"),
        run_started_at_unix_ms: 1_700_000_000_000,
        passed,
        checks,
        scorecard: BenchmarkScorecard {
            task_completed: passed,
            evidence_quality: "ok".to_string(),
            correctness_checks_passed: 0,
            correctness_checks_total: failed_check_ids.len(),
            unnecessary_action_count: Some(2),
            retry_count: Some(1),
            human_review_notes: vec![],
            measurement_notes: vec![],
        },
        plan: "plan".to_string(),
        execution_summary: "exec".to_string(),
        reflection_summary: "refl".to_string(),
        benchmark_memory_key: "k".to_string(),
        benchmark_evidence_id: "e".to_string(),
        runtime: BenchmarkRuntimeReport {
            identity: "i".to_string(),
            selected_base_type: "b".to_string(),
            topology: "single-process".to_string(),
            adapter_implementation: "a".to_string(),
            topology_backend: "tb".to_string(),
            transport_backend: "tp".to_string(),
            supervisor_backend: "sv".to_string(),
            runtime_node: "rn".to_string(),
            mailbox_address: "ma".to_string(),
            snapshot_state_before_stop: "before".to_string(),
            snapshot_state_after_stop: "after".to_string(),
        },
        handoff: BenchmarkHandoffReport {
            exported_state: "ok".to_string(),
            exported_memory_records: 0,
            exported_evidence_records: 0,
            restored_runtime_state: "ok".to_string(),
            restored_session_phase: None,
            restored_session_objective: None,
        },
        artifacts: BenchmarkArtifactPaths {
            run_dir: "/tmp".to_string(),
            report_json: "/tmp/r.json".to_string(),
            report_txt: "/tmp/r.txt".to_string(),
            review_json: "/tmp/v.json".to_string(),
        },
    }
}

#[test]
fn benchmark_report_with_failed_scenarios_produces_hypotheses_with_evidence() {
    // Three reports, two of which failed (with different check counts).
    let reports = vec![
        synthetic_report("suite-a", "scenario-1", false, &["chk-1", "chk-2"]),
        synthetic_report("suite-a", "scenario-2", true, &[]),
        synthetic_report("suite-a", "scenario-3", false, &["chk-9"]),
    ];

    let hypotheses = form_hypotheses_from_benchmark_reports(&reports);

    // Issue #2099 acceptance: N failed scenarios → ≥1 hypothesis with evidence
    // refs back to the originating BenchmarkRunReport / scenario / checks.
    assert_eq!(
        hypotheses.len(),
        2,
        "expected one hypothesis per failed scenario"
    );
    assert_eq!(hypotheses[0].category, "benchmark-failure");
    assert!(
        hypotheses.iter().all(|h| h
            .source_evidence
            .iter()
            .any(|ev| matches!(ev, EvidenceRef::BenchmarkRunReport { .. }))),
        "every hypothesis must reference its BenchmarkRunReport"
    );
    assert!(
        hypotheses[0]
            .source_evidence
            .iter()
            .filter(|ev| matches!(ev, EvidenceRef::BenchmarkCheckFailure { .. }))
            .count()
            == 2,
        "scenario-1 had two failed checks; expected two check-failure refs"
    );
    assert!(
        hypotheses[1]
            .source_evidence
            .iter()
            .filter(|ev| matches!(ev, EvidenceRef::BenchmarkCheckFailure { .. }))
            .count()
            == 1,
        "scenario-3 had one failed check"
    );

    // Hypothesis ids must be deterministic and disambiguate across scenarios.
    assert_ne!(hypotheses[0].id, hypotheses[1].id);
    assert!(hypotheses[0].id.contains("scenario-1"));
    assert!(hypotheses[1].id.contains("scenario-3"));
}

#[test]
fn passed_reports_emit_no_hypotheses() {
    let reports = vec![synthetic_report("suite-a", "scenario-1", true, &[])];
    assert!(form_hypotheses_from_benchmark_reports(&reports).is_empty());
}

#[test]
fn regression_signals_produce_hypotheses() {
    let signals = vec![
        ScenarioSignal {
            scenario_id: "scenario-1".to_string(),
            signal: GymSignal::Regression { delta: -0.25 },
        },
        ScenarioSignal {
            scenario_id: "scenario-2".to_string(),
            signal: GymSignal::Stable,
        },
    ];
    let hypotheses = form_hypotheses_from_signals("suite-a", &signals);
    assert_eq!(hypotheses.len(), 1);
    assert_eq!(hypotheses[0].category, "benchmark-regression");
    assert!(hypotheses[0].id.contains("scenario-1"));
    assert!(matches!(
        hypotheses[0].source_evidence[0],
        EvidenceRef::BenchmarkScenario { .. }
    ));
}

#[test]
fn weak_dimensions_produce_hypotheses_with_dimension_evidence() {
    let weak = vec![WeakDimension {
        name: "specificity".to_string(),
        deficit: 0.18,
    }];
    let hypotheses = form_hypotheses_from_weak_dimensions(&weak);
    assert_eq!(hypotheses.len(), 1);
    assert_eq!(hypotheses[0].category, "weak-dimension");
    let evidence = &hypotheses[0].source_evidence;
    assert_eq!(evidence.len(), 1);
    match &evidence[0] {
        EvidenceRef::WeakDimension { dimension, deficit } => {
            assert_eq!(dimension, "specificity");
            assert!((*deficit - 0.18).abs() < 1e-9);
        }
        other => panic!("expected WeakDimension evidence, got {other:?}"),
    }
}

#[test]
fn review_proposals_become_hypotheses_with_review_and_proposal_evidence() {
    let review = ReviewArtifact {
        review_id: "rev-1".to_string(),
        reviewed_at_unix_ms: 0,
        target_kind: ReviewTargetKind::Benchmark,
        target_label: "suite-a:scenario-x".to_string(),
        identity_name: "id".to_string(),
        session_id: "sess".to_string(),
        selected_base_type: "bt".to_string(),
        topology: "single-process".to_string(),
        objective_metadata: "meta".to_string(),
        execution_summary: "exec".to_string(),
        reflection_summary: "refl".to_string(),
        summary: "sum".to_string(),
        measurement_notes: vec![],
        evidence_summary: ReviewEvidenceSummary {
            memory_records: 0,
            evidence_records: 0,
            decision_records: 0,
            benchmark_records: 0,
            exported_state: "stopped".to_string(),
            session_phase: None,
            failed_signals: vec![],
        },
        proposals: vec![ImprovementProposal {
            category: "evidence-capture".to_string(),
            title: "Capture more execution evidence".to_string(),
            rationale: "thin trail".to_string(),
            suggested_change: "record more phases".to_string(),
            evidence: vec!["benchmark-scenario:suite-a/scenario-x".to_string()],
        }],
    };
    let hypotheses = form_hypotheses_from_review(&review);
    assert_eq!(hypotheses.len(), 1);
    assert!(hypotheses[0].category.starts_with("review-"));
    let has_review = hypotheses[0]
        .source_evidence
        .iter()
        .any(|ev| matches!(ev, EvidenceRef::Review { review_id, .. } if review_id == "rev-1"));
    let has_scenario = hypotheses[0]
        .source_evidence
        .iter()
        .any(|ev| matches!(ev, EvidenceRef::BenchmarkScenario { .. }));
    assert!(
        has_review,
        "review evidence missing: {:?}",
        hypotheses[0].source_evidence
    );
    assert!(
        has_scenario,
        "proposal-supplied benchmark evidence missing: {:?}",
        hypotheses[0].source_evidence
    );
}

#[test]
fn session_failures_become_hypotheses() {
    let hypotheses = form_hypotheses_from_session_failures(
        "sess-1",
        &["benchmark-timeout".to_string(), "retry-storm".to_string()],
    );
    assert_eq!(hypotheses.len(), 2);
    for h in &hypotheses {
        assert_eq!(h.category, "session-failure");
        assert!(matches!(
            h.source_evidence[0],
            EvidenceRef::SessionFailure { .. }
        ));
    }
}

#[test]
fn aggregate_combines_all_sources_in_order() {
    let reports = vec![synthetic_report("suite-a", "scenario-1", false, &["chk-1"])];
    let signals = vec![ScenarioSignal {
        scenario_id: "scenario-2".to_string(),
        signal: GymSignal::Regression { delta: -0.1 },
    }];
    let weak = vec![WeakDimension {
        name: "explainability".to_string(),
        deficit: 0.2,
    }];
    let reviews: Vec<ReviewArtifact> = vec![];
    let session_failures = vec![("sess-1".to_string(), vec!["timeout".to_string()])];

    let all = aggregate_hypotheses(
        "suite-a",
        &reports,
        &signals,
        &weak,
        &reviews,
        &session_failures,
    );
    assert_eq!(all.len(), 4);
    assert_eq!(all[0].category, "benchmark-failure");
    assert_eq!(all[1].category, "benchmark-regression");
    assert_eq!(all[2].category, "weak-dimension");
    assert_eq!(all[3].category, "session-failure");
}

#[test]
fn hypothesis_into_proposed_change_carries_evidence_in_expected_impact() {
    let reports = vec![synthetic_report("suite-a", "scenario-1", false, &["chk-1"])];
    let mut hypotheses = form_hypotheses_from_benchmark_reports(&reports);
    let hypothesis = hypotheses.remove(0);
    let change = hypothesis.into_proposed_change("src/policies/scenario_1.toml");
    assert_eq!(change.file_path, "src/policies/scenario_1.toml");
    assert!(change.expected_impact.contains("benchmark-failure"));
    assert!(
        change
            .expected_impact
            .contains("benchmark:suite-a/scenario-1")
    );
}
