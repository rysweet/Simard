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

#[cfg(test)]
mod promotion_inline {
    use super::super::promotion::*;
    use super::super::types::ImprovementPromotionPlan;
    use crate::review::ReviewArtifact;

    fn valid_plan_text() -> String {
        [
            "review-id: rev-001",
            "review-target: benchmark-run",
            "proposal: Fix flaky test | category=testing | rationale=Reduces CI noise | suggested_change=Add retry logic | evidence=ci-log-42",
            "approve: Fix flaky test | priority=2 | status=active | rationale=High impact fix",
        ]
        .join("\n")
    }

    #[test]
    fn parse_valid_plan() {
        let plan = ImprovementPromotionPlan::parse(&valid_plan_text()).unwrap();
        assert_eq!(plan.review_id, "rev-001");
        assert_eq!(plan.review_target, "benchmark-run");
        assert_eq!(plan.proposals.len(), 1);
        assert_eq!(plan.approvals.len(), 1);
        assert!(plan.deferrals.is_empty());
    }

    #[test]
    fn parse_missing_review_id_errors() {
        let raw = [
            "proposal: X | category=c | rationale=r | suggested_change=s | evidence=e",
            "approve: X",
        ]
        .join("\n");
        assert!(ImprovementPromotionPlan::parse(&raw).is_err());
    }

    #[test]
    fn parse_no_proposals_errors() {
        let raw = "review-id: rev-001\napprove: X";
        assert!(ImprovementPromotionPlan::parse(raw).is_err());
    }

    #[test]
    fn parse_no_decisions_errors() {
        let raw = [
            "review-id: rev-001",
            "proposal: X | category=c | rationale=r | suggested_change=s | evidence=e",
        ]
        .join("\n");
        assert!(ImprovementPromotionPlan::parse(&raw).is_err());
    }

    #[test]
    fn parse_unknown_decision_title_errors() {
        let raw = [
            "review-id: rev-001",
            "proposal: Real title | category=c | rationale=r | suggested_change=s | evidence=e",
            "approve: Wrong title",
        ]
        .join("\n");
        assert!(ImprovementPromotionPlan::parse(&raw).is_err());
    }

    #[test]
    fn parse_with_deferral() {
        let raw = [
            "review-id: rev-002",
            "proposal: Later thing | category=perf | rationale=low impact | suggested_change=optimize | evidence=bench-data",
            "defer: Later thing | rationale=Not a priority now",
        ]
        .join("\n");
        let plan = ImprovementPromotionPlan::parse(&raw).unwrap();
        assert_eq!(plan.deferrals.len(), 1);
        assert_eq!(plan.deferrals[0].title, "Later thing");
    }

    #[test]
    fn approval_summaries_format() {
        let plan = ImprovementPromotionPlan::parse(&valid_plan_text()).unwrap();
        let summaries = plan.approval_summaries();
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].starts_with("p2"));
        assert!(summaries[0].contains("[active]"));
    }

    #[test]
    fn deferral_summaries_format() {
        let raw = [
            "review-id: rev-003",
            "proposal: X | category=c | rationale=r | suggested_change=s | evidence=e",
            "defer: X | rationale=Not yet",
        ]
        .join("\n");
        let plan = ImprovementPromotionPlan::parse(&raw).unwrap();
        let summaries = plan.deferral_summaries();
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].contains("Not yet"));
    }

    #[test]
    fn render_review_context_directives_includes_review_id() {
        let review = ReviewArtifact {
            review_id: "rev-100".into(),
            reviewed_at_unix_ms: 0,
            target_kind: crate::review::ReviewTargetKind::Benchmark,
            target_label: "suite:scenario".into(),
            identity_name: "id".into(),
            session_id: "s".into(),
            selected_base_type: "bt".into(),
            topology: "single-process".into(),
            objective_metadata: "meta".into(),
            execution_summary: "exec".into(),
            reflection_summary: "refl".into(),
            summary: "sum".into(),
            measurement_notes: vec![],
            evidence_summary: crate::review::ReviewEvidenceSummary {
                memory_records: 0,
                evidence_records: 0,
                decision_records: 0,
                benchmark_records: 0,
                exported_state: "stopped".into(),
                session_phase: None,
                failed_signals: vec![],
            },
            proposals: vec![],
        };
        let output = render_review_context_directives(&review);
        assert!(output.contains("review-id: rev-100"));
        assert!(output.contains("review-target: suite:scenario"));
    }
}
