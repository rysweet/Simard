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

/// Round-trip tests for issue #2091: promotion must preserve every evidence
/// reference from the originating proposal onto the resulting `GoalUpdate`
/// (and onto the persisted `GoalRecord` derived from that update).
#[cfg(test)]
mod evidence_round_trip {
    use crate::goals::{GoalRecord, GoalUpdate};
    use crate::improvements::{EvidenceRef, ImprovementPromotionPlan};
    use crate::session::{SessionId, SessionPhase};

    fn plan_with_evidence() -> ImprovementPromotionPlan {
        // Each proposal carries a structured benchmark evidence string and a
        // free-form raw entry; promotion must preserve both as typed
        // EvidenceRefs without dropping or reordering signal.
        let raw = [
            "review-id: rev-evidence-1",
            "review-target: suite-a:scenario-x",
            "proposal: Strengthen scenario X | category=correctness | rationale=two checks failed | suggested_change=tighten prompt | evidence=benchmark-scenario:suite-a/scenario-x ;; check-failure:suite-a/scenario-x/check-1:expected-action-missing ;; ad-hoc note from operator",
            "approve: Strengthen scenario X | priority=2 | status=active | rationale=fix now",
        ]
        .join("\n");
        ImprovementPromotionPlan::parse(&raw).unwrap()
    }

    #[test]
    fn approved_goal_updates_preserve_proposal_evidence() {
        let plan = plan_with_evidence();
        let updates = plan.approved_goal_updates().unwrap();
        assert_eq!(updates.len(), 1, "one approval → one GoalUpdate");
        let update = &updates[0];

        // Evidence must include the parsed benchmark scenario, the parsed
        // check failure, the raw note, plus a synthetic review reference
        // appended by the promotion plan (review-level provenance).
        assert!(
            update.evidence.len() >= 4,
            "GoalUpdate must carry every proposal evidence ref plus the review reference; got {:?}",
            update.evidence
        );

        let has_scenario = update.evidence.iter().any(|ev| {
            matches!(
                ev,
                EvidenceRef::BenchmarkScenario { suite_id, scenario_id, .. }
                if suite_id == "suite-a" && scenario_id == "scenario-x"
            )
        });
        let has_check_failure = update.evidence.iter().any(|ev| {
            matches!(
                ev,
                EvidenceRef::BenchmarkCheckFailure { suite_id, scenario_id, check_id, .. }
                if suite_id == "suite-a" && scenario_id == "scenario-x" && check_id == "check-1"
            )
        });
        let has_raw_note = update
            .evidence
            .iter()
            .any(|ev| matches!(ev, EvidenceRef::Raw { label } if label.contains("operator")));
        let has_review = update.evidence.iter().any(|ev| {
            matches!(
                ev,
                EvidenceRef::Review { review_id, target_label }
                if review_id == "rev-evidence-1"
                    && target_label.as_deref() == Some("suite-a:scenario-x")
            )
        });

        assert!(
            has_scenario,
            "scenario evidence missing: {:?}",
            update.evidence
        );
        assert!(
            has_check_failure,
            "check failure evidence missing: {:?}",
            update.evidence
        );
        assert!(
            has_raw_note,
            "raw note evidence missing: {:?}",
            update.evidence
        );
        assert!(
            has_review,
            "review-level evidence missing: {:?}",
            update.evidence
        );
    }

    #[test]
    fn goal_record_from_update_carries_evidence() {
        let plan = plan_with_evidence();
        let update = plan.approved_goal_updates().unwrap().pop().unwrap();
        let session = SessionId::parse("session-01234567-89ab-cdef-0123-456789abcdef").unwrap();
        let expected_evidence = update.evidence.clone();

        let record = GoalRecord::from_update(
            update,
            "operator".to_string(),
            session,
            SessionPhase::Persistence,
        )
        .expect("from_update should succeed for a valid GoalUpdate");

        assert_eq!(
            record.evidence, expected_evidence,
            "persisted GoalRecord must mirror the originating GoalUpdate's evidence"
        );
        assert!(
            !record.evidence.is_empty(),
            "evidence chain dropped during goal record persistence"
        );
    }

    #[test]
    fn promotion_appends_review_evidence_even_when_proposal_has_none() {
        // The proposal carries NO `evidence=` segment, so the only evidence on
        // the promoted GoalUpdate must be the review-level ref the plan appends.
        let raw = [
            "review-id: rev-no-evidence",
            "review-target: suite-b:scenario-y",
            "proposal: Title only | category=c | rationale=r | suggested_change=s",
            "approve: Title only | priority=1 | status=active | rationale=push it",
        ]
        .join("\n");
        let plan = ImprovementPromotionPlan::parse(&raw).unwrap();
        let updates = plan.approved_goal_updates().unwrap();
        assert_eq!(
            updates[0].evidence.len(),
            1,
            "an evidence-free proposal must yield exactly the appended review ref: {:?}",
            updates[0].evidence
        );
        assert!(
            matches!(
                &updates[0].evidence[0],
                EvidenceRef::Review { review_id, target_label }
                    if review_id == "rev-no-evidence"
                        && target_label.as_deref() == Some("suite-b:scenario-y")
            ),
            "appended evidence must be the review-level ref: {:?}",
            updates[0].evidence
        );
    }

    #[test]
    fn goal_update_serde_round_trip_preserves_evidence() {
        let update = GoalUpdate::new(
            "title".to_string(),
            "rationale".to_string(),
            crate::goals::GoalStatus::Active,
            3,
        )
        .unwrap()
        .with_evidence(vec![
            EvidenceRef::BenchmarkScenario {
                suite_id: "suite".to_string(),
                scenario_id: "scen".to_string(),
                session_id: Some("sess-1".to_string()),
            },
            EvidenceRef::Review {
                review_id: "rev-1".to_string(),
                target_label: Some("session".to_string()),
            },
        ]);

        let json = serde_json::to_string(&update).unwrap();
        let parsed: GoalUpdate = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, update);
        assert_eq!(parsed.evidence.len(), 2);
    }

    #[test]
    fn legacy_goal_update_json_without_evidence_field_still_deserializes() {
        // GoalUpdate records written before the evidence field existed must
        // still load cleanly with an empty evidence vector (spec line 696
        // requires evidence be preserved going *forward* without breaking
        // historical persistence).
        let legacy = r#"{
            "slug": "old-title",
            "title": "old title",
            "rationale": "old rationale",
            "status": "active",
            "priority": 1
        }"#;
        let update: GoalUpdate = serde_json::from_str(legacy).unwrap();
        assert_eq!(update.title, "old title");
        assert!(update.evidence.is_empty());
    }

    #[test]
    fn empty_evidence_is_omitted_from_serialised_output() {
        // The PR claims byte-identical on-the-wire output until evidence is
        // populated. `skip_serializing_if = "Vec::is_empty"` must actually omit
        // the `evidence` key for an empty vector on both GoalUpdate and the
        // persisted GoalRecord, so legacy readers see no new field.
        let update = GoalUpdate::new(
            "title".to_string(),
            "rationale".to_string(),
            crate::goals::GoalStatus::Active,
            1,
        )
        .unwrap();
        let update_json = serde_json::to_string(&update).unwrap();
        assert!(
            !update_json.contains("evidence"),
            "empty evidence must be omitted from GoalUpdate JSON: {update_json}"
        );

        let session = SessionId::parse("session-01234567-89ab-cdef-0123-456789abcdef").unwrap();
        let record = GoalRecord::from_update(
            update,
            "operator".to_string(),
            session,
            SessionPhase::Persistence,
        )
        .unwrap();
        let record_json = serde_json::to_string(&record).unwrap();
        assert!(
            !record_json.contains("evidence"),
            "empty evidence must be omitted from GoalRecord JSON: {record_json}"
        );
    }
}
