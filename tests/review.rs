use std::fs;
use std::path::PathBuf;

use simard::{
    BaseTypeId, EvidenceRecord, EvidenceSource, ImprovementProposal, MemoryRecord, MemoryScope,
    ReviewRequest, ReviewTargetKind, RuntimeAddress, RuntimeHandoffSnapshot, RuntimeNodeId,
    RuntimeState, RuntimeTopology, SessionId, SessionPhase, SessionRecord, build_review_artifact,
    latest_review_artifact, persist_review_artifact,
};
use uuid::Uuid;

fn fixture_handoff() -> RuntimeHandoffSnapshot {
    let session_id = SessionId::from_uuid(Uuid::now_v7());
    RuntimeHandoffSnapshot {
        exported_state: RuntimeState::Ready,
        identity_name: "simard-engineer".to_string(),
        selected_base_type: BaseTypeId::new("local-harness"),
        topology: RuntimeTopology::SingleProcess,
        source_runtime_node: RuntimeNodeId::local(),
        source_mailbox_address: RuntimeAddress::local(&RuntimeNodeId::local()),
        session: Some(SessionRecord {
            id: session_id.clone(),
            mode: simard::OperatingMode::Engineer,
            objective: "objective-metadata(chars=64, words=9, lines=1)".to_string(),
            phase: SessionPhase::Complete,
            selected_base_type: BaseTypeId::new("local-harness"),
            evidence_ids: vec!["ev-1".to_string(), "ev-2".to_string()],
            memory_keys: vec!["mem-1".to_string(), "mem-2".to_string()],
        }),
        memory_records: vec![
            MemoryRecord {
                key: "mem-1".to_string(),
                scope: MemoryScope::SessionScratch,
                value: "objective-metadata(chars=64, words=9, lines=1)".to_string(),
                session_id: session_id.clone(),
                recorded_in: SessionPhase::Preparation,
            },
            MemoryRecord {
                key: "mem-2".to_string(),
                scope: MemoryScope::SessionSummary,
                value: "summary".to_string(),
                session_id: session_id.clone(),
                recorded_in: SessionPhase::Persistence,
            },
        ],
        evidence_records: vec![
            EvidenceRecord {
                id: "ev-1".to_string(),
                session_id: session_id.clone(),
                phase: SessionPhase::Execution,
                detail: "planned a bounded change".to_string(),
                source: EvidenceSource::Runtime,
            },
            EvidenceRecord {
                id: "ev-2".to_string(),
                session_id,
                phase: SessionPhase::Execution,
                detail: "captured one execution outcome".to_string(),
                source: EvidenceSource::Runtime,
            },
        ],
    }
}

fn proposal_titles(proposals: &[ImprovementProposal]) -> Vec<&str> {
    proposals
        .iter()
        .map(|proposal| proposal.title.as_str())
        .collect()
}

#[test]
fn session_review_emits_evidence_and_benchmark_proposals() {
    let handoff = fixture_handoff();
    let review = build_review_artifact(
        ReviewRequest {
            target_kind: ReviewTargetKind::Session,
            target_label: "operator-review".to_string(),
            execution_summary: "completed a bounded engineering session".to_string(),
            reflection_summary: "brief reflection".to_string(),
            measurement_notes: Vec::new(),
            signals: Vec::new(),
        },
        &handoff,
    )
    .expect("review artifact should build");

    let titles = proposal_titles(&review.proposals);
    assert!(titles.contains(&"Capture denser execution evidence"));
    assert!(titles.contains(&"Promote this pattern into a repeatable benchmark"));
    assert!(review.summary.contains("operator-review"));
}

#[test]
fn benchmark_review_turns_measurement_notes_into_concrete_proposals() {
    let handoff = fixture_handoff();
    let review = build_review_artifact(
        ReviewRequest {
            target_kind: ReviewTargetKind::Benchmark,
            target_label: "starter:composite-session-review".to_string(),
            execution_summary: "completed benchmark scenario".to_string(),
            reflection_summary: "reflection mentions execution but still keeps measurement gaps visible".to_string(),
            measurement_notes: vec![
                "unnecessary_action_count remains unmeasured until the benchmark runner can classify shell/tool actions directly".to_string(),
                "retry_count is currently zero because the benchmark runner does not yet re-plan or retry failed scenarios automatically".to_string(),
            ],
            signals: vec![simard::ReviewSignal {
                id: "runtime-evidence-produced".to_string(),
                passed: false,
                detail: "runtime recorded 2 evidence records before benchmark capture; expected at least 4".to_string(),
            }],
        },
        &handoff,
    )
    .expect("benchmark review should build");

    let titles = proposal_titles(&review.proposals);
    assert!(titles.contains(&"Capture denser execution evidence"));
    assert!(titles.contains(&"Measure unnecessary action count explicitly"));
    assert!(titles.contains(&"Track bounded retries in benchmark runs"));
}

#[test]
fn persisted_review_artifact_lands_under_review_artifacts_directory() {
    let handoff = fixture_handoff();
    let review = build_review_artifact(
        ReviewRequest {
            target_kind: ReviewTargetKind::Session,
            target_label: "operator-review".to_string(),
            execution_summary: "completed a bounded engineering session".to_string(),
            reflection_summary: "reflection covers the boundary clearly enough for testing"
                .to_string(),
            measurement_notes: Vec::new(),
            signals: Vec::new(),
        },
        &handoff,
    )
    .expect("review artifact should build");
    let temp_root = std::env::temp_dir().join(format!("simard-review-test-{}", Uuid::now_v7()));
    let artifact_path =
        persist_review_artifact(&temp_root, &review).expect("review artifact should persist");

    assert!(artifact_path.ends_with(format!("review-artifacts/{}.json", review.review_id)));
    let payload = fs::read_to_string(&artifact_path).expect("artifact should be readable");
    assert!(payload.contains(&review.review_id));

    if temp_root.exists() {
        fs::remove_dir_all(PathBuf::from(&temp_root)).expect("temp root should be removable");
    }
}

#[test]
fn latest_review_artifact_returns_newest_persisted_review_across_runs() {
    let handoff = fixture_handoff();
    let temp_root = std::env::temp_dir().join(format!("simard-review-test-{}", Uuid::now_v7()));
    let mut older_review = build_review_artifact(
        ReviewRequest {
            target_kind: ReviewTargetKind::Session,
            target_label: "operator-review".to_string(),
            execution_summary: "completed a bounded engineering session".to_string(),
            reflection_summary: "reflection covers the first pass clearly enough for testing"
                .to_string(),
            measurement_notes: Vec::new(),
            signals: Vec::new(),
        },
        &handoff,
    )
    .expect("older review should build");
    older_review.review_id = "older-review".to_string();
    older_review.reviewed_at_unix_ms = 10;
    persist_review_artifact(&temp_root, &older_review).expect("older review should persist");

    let mut newer_review = build_review_artifact(
        ReviewRequest {
            target_kind: ReviewTargetKind::Benchmark,
            target_label: "starter:composite-session-review".to_string(),
            execution_summary: "completed benchmark scenario".to_string(),
            reflection_summary: "reflection covers the benchmark replay with explicit evidence and operator follow-up"
                .to_string(),
            measurement_notes: vec![
                "retry_count is currently zero because the benchmark runner does not yet re-plan or retry failed scenarios automatically".to_string(),
            ],
            signals: Vec::new(),
        },
        &handoff,
    )
    .expect("newer review should build");
    newer_review.review_id = "newer-review".to_string();
    newer_review.reviewed_at_unix_ms = 20;
    persist_review_artifact(&temp_root, &newer_review).expect("newer review should persist");

    let (artifact_path, loaded_review) = latest_review_artifact(&temp_root)
        .expect("latest review lookup should succeed")
        .expect("latest review should exist");
    assert!(artifact_path.ends_with("review-artifacts/newer-review.json"));
    assert_eq!(loaded_review.review_id, "newer-review");
    assert_eq!(loaded_review.target_kind, ReviewTargetKind::Benchmark);
    assert!(
        proposal_titles(&loaded_review.proposals)
            .contains(&"Track bounded retries in benchmark runs")
    );

    if temp_root.exists() {
        fs::remove_dir_all(PathBuf::from(&temp_root)).expect("temp root should be removable");
    }
}
