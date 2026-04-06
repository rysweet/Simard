use super::promotion::*;
use super::types::ImprovementPromotionPlan;
use crate::error::SimardError;
use crate::goals::GoalStatus;
use crate::review::{ImprovementProposal, ReviewArtifact, ReviewEvidenceSummary, ReviewTargetKind};

#[test]
fn parses_review_context_and_operator_decisions() {
    let raw = "\
review-id: session-1-review\n\
review-target: operator-review\n\
proposal: Capture denser execution evidence | category=evidence-capture | rationale=thin trail | suggested_change=record more phases | evidence=phase-1 ;; phase-2\n\
proposal: Promote this pattern into a repeatable benchmark | category=benchmark-coverage | rationale=one-off session | suggested_change=make a scenario | evidence=target=operator-review\n\
approve: Capture denser execution evidence | priority=1 | status=active | rationale=make this visible now\n\
defer: Promote this pattern into a repeatable benchmark | rationale=wait for the next planning pass";

    let plan = ImprovementPromotionPlan::parse(raw).expect("plan should parse");

    assert_eq!(plan.review_id, "session-1-review");
    assert_eq!(plan.proposals.len(), 2);
    assert_eq!(plan.approvals.len(), 1);
    assert_eq!(plan.deferrals.len(), 1);
    assert_eq!(plan.approvals[0].status, GoalStatus::Active);
}

#[test]
fn rejects_decisions_for_unknown_proposals() {
    let raw = "\
review-id: session-1-review\n\
proposal: Capture denser execution evidence | category=evidence-capture | rationale=thin trail | suggested_change=record more phases | evidence=phase-1\n\
approve: Missing proposal | priority=1 | status=active | rationale=bad";

    let error = ImprovementPromotionPlan::parse(raw).unwrap_err();
    assert_eq!(
        error,
        SimardError::InvalidImprovementRecord {
            field: "decision".to_string(),
            reason: "decision references unknown proposal 'Missing proposal'".to_string(),
        }
    );
}

#[test]
fn renders_review_context_directives_for_operator_curator_sessions() {
    let review = ReviewArtifact {
        review_id: "session-1-review".to_string(),
        reviewed_at_unix_ms: 1,
        target_kind: ReviewTargetKind::Session,
        target_label: "operator-review".to_string(),
        identity_name: "simard-engineer".to_string(),
        session_id: "session-1".to_string(),
        selected_base_type: "local-harness".to_string(),
        topology: "single-process".to_string(),
        objective_metadata: "objective-metadata(chars=10, words=2, lines=1)".to_string(),
        execution_summary: "done".to_string(),
        reflection_summary: "reflect".to_string(),
        summary: "summary".to_string(),
        measurement_notes: Vec::new(),
        evidence_summary: ReviewEvidenceSummary {
            memory_records: 1,
            evidence_records: 1,
            decision_records: 1,
            benchmark_records: 0,
            exported_state: "ready".to_string(),
            session_phase: Some("complete".to_string()),
            failed_signals: Vec::new(),
        },
        proposals: vec![ImprovementProposal {
            category: "evidence-capture".to_string(),
            title: "Capture denser execution evidence".to_string(),
            rationale: "thin trail".to_string(),
            suggested_change: "record more phases".to_string(),
            evidence: vec!["phase-1".to_string(), "phase-2".to_string()],
        }],
    };

    let directives = render_review_context_directives(&review);
    assert!(directives.contains("review-id: session-1-review"));
    assert!(directives.contains("proposal: Capture denser execution evidence"));
    assert!(directives.contains("evidence=phase-1 ;; phase-2"));
}
