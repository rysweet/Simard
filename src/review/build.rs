use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{SimardError, SimardResult};
use crate::handoff::RuntimeHandoffSnapshot;
use crate::memory::MemoryScope;

use super::types::{
    ImprovementProposal, ReviewArtifact, ReviewEvidenceSummary, ReviewRequest, ReviewSignal,
    ReviewTargetKind,
};

pub fn build_review_artifact(
    request: ReviewRequest,
    handoff: &RuntimeHandoffSnapshot,
) -> SimardResult<ReviewArtifact> {
    let session = handoff
        .session
        .as_ref()
        .ok_or_else(|| SimardError::InvalidHandoffSnapshot {
            field: "session".to_string(),
            reason: "review requires an exported session boundary".to_string(),
        })?;
    let failed_signals = request
        .signals
        .iter()
        .filter(|signal| !signal.passed)
        .map(|signal| format!("{}: {}", signal.id, signal.detail))
        .collect::<Vec<_>>();
    let evidence_summary = ReviewEvidenceSummary {
        memory_records: handoff.memory_records.len(),
        evidence_records: handoff.evidence_records.len(),
        decision_records: handoff
            .memory_records
            .iter()
            .filter(|record| record.scope == MemoryScope::Decision)
            .count(),
        benchmark_records: handoff
            .memory_records
            .iter()
            .filter(|record| record.scope == MemoryScope::Benchmark)
            .count(),
        exported_state: handoff.exported_state.to_string(),
        session_phase: handoff
            .session
            .as_ref()
            .map(|record| record.phase.to_string()),
        failed_signals,
    };
    let proposals = improvement_proposals(&request, handoff, &evidence_summary);
    let summary = review_summary(&request, &evidence_summary, &proposals);

    Ok(ReviewArtifact {
        review_id: format!("{}-review", session.id),
        reviewed_at_unix_ms: now_unix_ms()?,
        target_kind: request.target_kind,
        target_label: request.target_label,
        identity_name: handoff.identity_name.clone(),
        session_id: session.id.to_string(),
        selected_base_type: handoff.selected_base_type.to_string(),
        topology: handoff.topology.to_string(),
        objective_metadata: session.objective.clone(),
        execution_summary: request.execution_summary,
        reflection_summary: request.reflection_summary,
        summary,
        measurement_notes: request.measurement_notes,
        evidence_summary,
        proposals,
    })
}

fn improvement_proposals(
    request: &ReviewRequest,
    handoff: &RuntimeHandoffSnapshot,
    summary: &ReviewEvidenceSummary,
) -> Vec<ImprovementProposal> {
    let mut proposals = Vec::new();

    if summary.evidence_records <= 4 || failed_signal(&request.signals, "evidence") {
        proposals.push(ImprovementProposal {
            category: "evidence-capture".to_string(),
            title: "Capture denser execution evidence".to_string(),
            rationale: format!(
                "This review only had {} persisted evidence records, which leaves the operator with a thin trail when checking what really happened.",
                summary.evidence_records
            ),
            suggested_change: "Record one concise evidence item per major phase boundary or adapter turn so reviews can cite planning, execution, and persistence evidence separately.".to_string(),
            evidence: handoff
                .evidence_records
                .iter()
                .take(3)
                .map(|record| record.detail.clone())
                .collect(),
        });
    }

    if request.target_kind == ReviewTargetKind::Session && summary.benchmark_records == 0 {
        proposals.push(ImprovementProposal {
            category: "benchmark-coverage".to_string(),
            title: "Promote this pattern into a repeatable benchmark".to_string(),
            rationale: "This review covered a one-off session with no benchmark-scoped memory, so future regressions would be hard to compare objectively.".to_string(),
            suggested_change: "Turn the reviewed operator flow into a named benchmark scenario so the same task can be replayed before accepting follow-on prompt or policy changes.".to_string(),
            evidence: vec![format!(
                "target={} exported benchmark records={}",
                request.target_label, summary.benchmark_records
            )],
        });
    }

    if request
        .measurement_notes
        .iter()
        .any(|note| note.contains("unnecessary_action_count"))
    {
        proposals.push(ImprovementProposal {
            category: "operator-metrics".to_string(),
            title: "Measure unnecessary action count explicitly".to_string(),
            rationale: "The benchmark report still admits that unnecessary actions are unmeasured, so review decisions cannot separate wasteful behavior from necessary work.".to_string(),
            suggested_change: "Add a bounded action counter in the gym runner and emit it as structured review evidence instead of leaving it as a free-form note.".to_string(),
            evidence: request
                .measurement_notes
                .iter()
                .filter(|note| note.contains("unnecessary_action_count"))
                .cloned()
                .collect(),
        });
    }

    if request
        .measurement_notes
        .iter()
        .any(|note| note.contains("retry_count"))
    {
        proposals.push(ImprovementProposal {
            category: "retry-policy".to_string(),
            title: "Track bounded retries in benchmark runs".to_string(),
            rationale: "The current benchmark foundation cannot show whether a scenario only succeeded because it would have needed a retry loop, so promotion decisions are missing an important quality signal.".to_string(),
            suggested_change: "Add an explicit retry-and-replan path for benchmark scenarios and persist the observed retry count as review evidence.".to_string(),
            evidence: request
                .measurement_notes
                .iter()
                .filter(|note| note.contains("retry_count"))
                .cloned()
                .collect(),
        });
    }

    if failed_signal(&request.signals, "reflection") || request.reflection_summary.trim().len() < 80
    {
        proposals.push(ImprovementProposal {
            category: "reflection-calibration".to_string(),
            title: "Tighten reflection summaries around operator-visible facts".to_string(),
            rationale: "The current reflection summary is too thin to explain why the session should count as successful or where it struggled.".to_string(),
            suggested_change: "Require reflection summaries to mention the concrete outcome, evidence density, and the next operator check instead of only restating the runtime wiring.".to_string(),
            evidence: vec![request.reflection_summary.clone()],
        });
    }

    if proposals.is_empty() {
        proposals.push(ImprovementProposal {
            category: "review-promotion".to_string(),
            title: "Carry accepted findings into the next run deliberately".to_string(),
            rationale: "This review did not surface an obvious failure, but it still produced evidence that should gate the next change instead of being discarded.".to_string(),
            suggested_change: "Use the persisted review artifact as an explicit approval checkpoint before promoting prompt, policy, or orchestration changes into the next benchmark cycle.".to_string(),
            evidence: vec![format!(
                "reviewed target={} with {} evidence records and {} failed signals",
                request.target_label,
                summary.evidence_records,
                summary.failed_signals.len()
            )],
        });
    }

    proposals.truncate(3);
    proposals
}

fn failed_signal(signals: &[ReviewSignal], needle: &str) -> bool {
    signals
        .iter()
        .any(|signal| !signal.passed && signal.id.contains(needle))
}

fn review_summary(
    request: &ReviewRequest,
    summary: &ReviewEvidenceSummary,
    proposals: &[ImprovementProposal],
) -> String {
    format!(
        "{} review for '{}' inspected {} evidence records, {} memory records, and {} failed signals, then emitted {} concrete proposal(s).",
        request.target_kind.as_str(),
        request.target_label,
        summary.evidence_records,
        summary.memory_records,
        summary.failed_signals.len(),
        proposals.len()
    )
}

fn now_unix_ms() -> SimardResult<u128> {
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
}
