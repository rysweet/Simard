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
