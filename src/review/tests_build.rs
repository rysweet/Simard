use super::build::*;
use super::types::*;
use crate::handoff::RuntimeHandoffSnapshot;
use crate::memory::MemoryScope;

// ---- failed_signal ----

#[test]
fn failed_signal_matches_failing_signal() {
    let signals = vec![
        ReviewSignal {
            id: "evidence-density".into(),
            passed: false,
            detail: "too few".into(),
        },
        ReviewSignal {
            id: "reflection-quality".into(),
            passed: true,
            detail: "ok".into(),
        },
    ];
    assert!(failed_signal(&signals, "evidence"));
    assert!(!failed_signal(&signals, "reflection")); // passed=true
    assert!(!failed_signal(&signals, "nonexistent"));
}

#[test]
fn failed_signal_empty_signals() {
    assert!(!failed_signal(&[], "anything"));
}

// ---- now_unix_ms ----

#[test]
fn now_unix_ms_returns_reasonable_timestamp() {
    let ms = now_unix_ms().unwrap();
    // Should be after 2024-01-01 (1704067200000 ms)
    assert!(ms > 1_704_067_200_000);
}

// ---- review_summary ----

#[test]
fn review_summary_formats_correctly() {
    let request = ReviewRequest {
        target_kind: ReviewTargetKind::Session,
        target_label: "test-session".into(),
        execution_summary: "ok".into(),
        reflection_summary: "good".into(),
        measurement_notes: vec![],
        signals: vec![],
    };
    let evidence = ReviewEvidenceSummary {
        memory_records: 10,
        evidence_records: 5,
        decision_records: 2,
        benchmark_records: 1,
        exported_state: "stopped".into(),
        session_phase: Some("complete".into()),
        failed_signals: vec!["sig1".into()],
    };
    let proposals = vec![ImprovementProposal {
        category: "test".into(),
        title: "test proposal".into(),
        rationale: "because".into(),
        suggested_change: "do this".into(),
        evidence: vec![],
    }];
    let summary = review_summary(&request, &evidence, &proposals);
    assert!(summary.contains("session"));
    assert!(summary.contains("test-session"));
    assert!(summary.contains("5 evidence records"));
    assert!(summary.contains("10 memory records"));
    assert!(summary.contains("1 failed signals"));
    assert!(summary.contains("1 concrete proposal(s)"));
}

// ---- ReviewTargetKind::as_str ----

#[test]
fn target_kind_as_str() {
    assert_eq!(ReviewTargetKind::Session.as_str(), "session");
    assert_eq!(ReviewTargetKind::Benchmark.as_str(), "benchmark");
}

// ---- improvement_proposals ----

fn minimal_handoff() -> RuntimeHandoffSnapshot {
    use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeState, RuntimeTopology};
    RuntimeHandoffSnapshot {
        exported_state: RuntimeState::Stopped,
        identity_name: "test".into(),
        selected_base_type: "test-base".into(),
        topology: RuntimeTopology::SingleProcess,
        source_runtime_node: RuntimeNodeId::new("node-1"),
        source_mailbox_address: RuntimeAddress::new("addr-1"),
        session: None,
        memory_records: vec![],
        evidence_records: vec![],
        copilot_submit_audit: None,
    }
}

#[test]
fn proposals_thin_evidence_triggers_evidence_capture() {
    let request = ReviewRequest {
        target_kind: ReviewTargetKind::Session,
        target_label: "test".into(),
        execution_summary: "ok".into(),
        reflection_summary: "a decent reflection that is long enough to pass the 80-char threshold for calibration purposes yes".into(),
        measurement_notes: vec![],
        signals: vec![],
    };
    let handoff = minimal_handoff();
    let evidence = ReviewEvidenceSummary {
        memory_records: 2,
        evidence_records: 3, // <= 4 triggers
        decision_records: 0,
        benchmark_records: 1,
        exported_state: "stopped".into(),
        session_phase: None,
        failed_signals: vec![],
    };
    let proposals = improvement_proposals(&request, &handoff, &evidence);
    assert!(proposals.iter().any(|p| p.category == "evidence-capture"));
}

#[test]
fn proposals_no_benchmark_triggers_benchmark_coverage() {
    let request = ReviewRequest {
        target_kind: ReviewTargetKind::Session,
        target_label: "test".into(),
        execution_summary: "ok".into(),
        reflection_summary: "a decent reflection that is long enough to pass the 80-char threshold for calibration purposes yes".into(),
        measurement_notes: vec![],
        signals: vec![],
    };
    let handoff = minimal_handoff();
    let evidence = ReviewEvidenceSummary {
        memory_records: 10,
        evidence_records: 10,
        decision_records: 0,
        benchmark_records: 0, // triggers
        exported_state: "stopped".into(),
        session_phase: None,
        failed_signals: vec![],
    };
    let proposals = improvement_proposals(&request, &handoff, &evidence);
    assert!(proposals.iter().any(|p| p.category == "benchmark-coverage"));
}

#[test]
fn proposals_short_reflection_triggers_calibration() {
    let request = ReviewRequest {
        target_kind: ReviewTargetKind::Benchmark,
        target_label: "test".into(),
        execution_summary: "ok".into(),
        reflection_summary: "short".into(), // < 80 chars
        measurement_notes: vec![],
        signals: vec![],
    };
    let handoff = minimal_handoff();
    let evidence = ReviewEvidenceSummary {
        memory_records: 10,
        evidence_records: 10,
        decision_records: 0,
        benchmark_records: 1,
        exported_state: "stopped".into(),
        session_phase: None,
        failed_signals: vec![],
    };
    let proposals = improvement_proposals(&request, &handoff, &evidence);
    assert!(
        proposals
            .iter()
            .any(|p| p.category == "reflection-calibration")
    );
}

#[test]
fn proposals_measurement_notes_trigger_metrics() {
    let request = ReviewRequest {
        target_kind: ReviewTargetKind::Benchmark,
        target_label: "test".into(),
        execution_summary: "ok".into(),
        reflection_summary: "a decent reflection that is long enough to pass the 80-char threshold for calibration purposes yes".into(),
        measurement_notes: vec!["unnecessary_action_count=3".into(), "retry_count=1".into()],
        signals: vec![],
    };
    let handoff = minimal_handoff();
    let evidence = ReviewEvidenceSummary {
        memory_records: 10,
        evidence_records: 10,
        decision_records: 0,
        benchmark_records: 1,
        exported_state: "stopped".into(),
        session_phase: None,
        failed_signals: vec![],
    };
    let proposals = improvement_proposals(&request, &handoff, &evidence);
    assert!(proposals.iter().any(|p| p.category == "operator-metrics"));
    assert!(proposals.iter().any(|p| p.category == "retry-policy"));
}

#[test]
fn proposals_truncated_to_three() {
    // Trigger multiple proposals: thin evidence, no benchmark, short reflection, metrics
    let request = ReviewRequest {
        target_kind: ReviewTargetKind::Session,
        target_label: "test".into(),
        execution_summary: "ok".into(),
        reflection_summary: "short".into(),
        measurement_notes: vec!["unnecessary_action_count=3".into(), "retry_count=1".into()],
        signals: vec![ReviewSignal {
            id: "evidence-check".into(),
            passed: false,
            detail: "bad".into(),
        }],
    };
    let handoff = minimal_handoff();
    let evidence = ReviewEvidenceSummary {
        memory_records: 1,
        evidence_records: 1,
        decision_records: 0,
        benchmark_records: 0,
        exported_state: "stopped".into(),
        session_phase: None,
        failed_signals: vec!["evidence-check".into()],
    };
    let proposals = improvement_proposals(&request, &handoff, &evidence);
    assert!(proposals.len() <= 3, "proposals truncated to max 3");
}

#[test]
fn proposals_clean_review_emits_promotion() {
    let request = ReviewRequest {
        target_kind: ReviewTargetKind::Benchmark,
        target_label: "clean-run".into(),
        execution_summary: "ok".into(),
        reflection_summary: "a decent reflection that is long enough to pass the 80-char threshold for calibration purposes yes".into(),
        measurement_notes: vec![],
        signals: vec![],
    };
    let handoff = minimal_handoff();
    let evidence = ReviewEvidenceSummary {
        memory_records: 10,
        evidence_records: 10,
        decision_records: 2,
        benchmark_records: 1,
        exported_state: "stopped".into(),
        session_phase: None,
        failed_signals: vec![],
    };
    let proposals = improvement_proposals(&request, &handoff, &evidence);
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].category, "review-promotion");
}
