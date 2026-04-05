use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::base_types::BaseTypeId;
use crate::evidence::{EvidenceRecord, EvidenceSource};
use crate::identity::OperatingMode;
use crate::memory::{MemoryRecord, MemoryScope};
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeState, RuntimeTopology};
use crate::session::{SessionId, SessionPhase, SessionRecord};
use tempfile::TempDir;

use super::*;

fn make_session_id() -> SessionId {
    SessionId::parse("session-00000000-0000-0000-0000-000000000001").unwrap()
}

fn make_session_record() -> SessionRecord {
    SessionRecord {
        id: make_session_id(),
        mode: OperatingMode::Engineer,
        objective: "test objective".to_string(),
        phase: SessionPhase::Complete,
        selected_base_type: BaseTypeId::new("test-base"),
        evidence_ids: vec![],
        memory_keys: vec![],
    }
}

fn make_handoff() -> crate::handoff::RuntimeHandoffSnapshot {
    let node = RuntimeNodeId::new("test-node");
    crate::handoff::RuntimeHandoffSnapshot {
        exported_state: RuntimeState::Active,
        identity_name: "test-identity".to_string(),
        selected_base_type: BaseTypeId::new("test-base"),
        topology: RuntimeTopology::SingleProcess,
        source_runtime_node: node.clone(),
        source_mailbox_address: RuntimeAddress::local(&node),
        session: Some(make_session_record()),
        memory_records: vec![],
        evidence_records: vec![],
        copilot_submit_audit: None,
    }
}

fn make_handoff_with_records() -> crate::handoff::RuntimeHandoffSnapshot {
    let sid = make_session_id();
    let mut handoff = make_handoff();
    handoff.memory_records = vec![
        MemoryRecord {
            key: "decision-1".to_string(),
            scope: MemoryScope::Decision,
            value: "decided something".to_string(),
            session_id: sid.clone(),
            recorded_in: SessionPhase::Execution,
        },
        MemoryRecord {
            key: "bench-1".to_string(),
            scope: MemoryScope::Benchmark,
            value: "benchmark data".to_string(),
            session_id: sid.clone(),
            recorded_in: SessionPhase::Execution,
        },
        MemoryRecord {
            key: "scratch-1".to_string(),
            scope: MemoryScope::SessionScratch,
            value: "scratch".to_string(),
            session_id: sid.clone(),
            recorded_in: SessionPhase::Execution,
        },
    ];
    handoff.evidence_records = vec![
        EvidenceRecord {
            id: "ev-1".to_string(),
            session_id: sid.clone(),
            phase: SessionPhase::Execution,
            detail: "first evidence".to_string(),
            source: EvidenceSource::Runtime,
        },
        EvidenceRecord {
            id: "ev-2".to_string(),
            session_id: sid.clone(),
            phase: SessionPhase::Execution,
            detail: "second evidence".to_string(),
            source: EvidenceSource::Runtime,
        },
    ];
    handoff
}

fn make_request() -> ReviewRequest {
    ReviewRequest {
        target_kind: ReviewTargetKind::Session,
        target_label: "test-session".to_string(),
        execution_summary: "Executed successfully".to_string(),
        reflection_summary: "Reflection on the session with enough detail to pass \
            the length check for the improvement proposal threshold"
            .to_string(),
        measurement_notes: vec!["latency=100ms".to_string()],
        signals: vec![
            ReviewSignal {
                id: "signal-pass".to_string(),
                passed: true,
                detail: "all good".to_string(),
            },
            ReviewSignal {
                id: "signal-fail".to_string(),
                passed: false,
                detail: "something went wrong".to_string(),
            },
        ],
    }
}

fn make_artifact_direct(review_id: &str, timestamp: u128) -> ReviewArtifact {
    ReviewArtifact {
        review_id: review_id.to_string(),
        reviewed_at_unix_ms: timestamp,
        target_kind: ReviewTargetKind::Session,
        target_label: "label".to_string(),
        identity_name: "id".to_string(),
        session_id: "s1".to_string(),
        selected_base_type: "base".to_string(),
        topology: "single".to_string(),
        objective_metadata: "obj".to_string(),
        execution_summary: "exec".to_string(),
        reflection_summary: "refl".to_string(),
        summary: "summary".to_string(),
        measurement_notes: vec![],
        evidence_summary: ReviewEvidenceSummary {
            memory_records: 0,
            evidence_records: 0,
            decision_records: 0,
            benchmark_records: 0,
            exported_state: "active".to_string(),
            session_phase: None,
            failed_signals: vec![],
        },
        proposals: vec![],
    }
}

// 1. review_artifacts_dir returns correct path
#[test]
fn review_artifacts_dir_appends_subdir() {
    let root = PathBuf::from("/state/root");
    assert_eq!(review_artifacts_dir(&root), root.join("review-artifacts"));
}

// 2. build_review_artifact with mixed signals extracts failed ones correctly
#[test]
fn build_artifact_extracts_failed_signals() {
    let artifact = build_review_artifact(make_request(), &make_handoff()).unwrap();
    let failed = &artifact.evidence_summary.failed_signals;
    assert_eq!(failed.len(), 1);
    assert!(failed[0].contains("signal-fail"));
    assert!(failed[0].contains("something went wrong"));
}

#[test]
fn build_artifact_counts_memory_scopes() {
    let handoff = make_handoff_with_records();
    let artifact = build_review_artifact(make_request(), &handoff).unwrap();
    assert_eq!(artifact.evidence_summary.memory_records, 3);
    assert_eq!(artifact.evidence_summary.evidence_records, 2);
    assert_eq!(artifact.evidence_summary.decision_records, 1);
    assert_eq!(artifact.evidence_summary.benchmark_records, 1);
}

// 3. build_review_artifact generates a review_id and sets reviewed_at_unix_ms
#[test]
fn build_artifact_sets_id_and_timestamp() {
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let artifact = build_review_artifact(make_request(), &make_handoff()).unwrap();
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();

    assert!(!artifact.review_id.is_empty());
    assert!(artifact.review_id.contains("review"));
    assert!(artifact.reviewed_at_unix_ms >= before);
    assert!(artifact.reviewed_at_unix_ms <= after);
}

#[test]
fn build_artifact_fails_without_session() {
    let mut handoff = make_handoff();
    handoff.session = None;
    let result = build_review_artifact(make_request(), &handoff);
    assert!(result.is_err());
}

// 4. concise_record contains target label and signal counts
#[test]
fn concise_record_includes_target_and_counts() {
    let artifact = build_review_artifact(make_request(), &make_handoff()).unwrap();
    let record = artifact.concise_record();
    assert!(record.contains("test-session"), "missing target label");
    assert!(
        record.contains("failed_signals=1"),
        "missing failed signal count"
    );
    assert!(
        record.contains("evidence_records=0"),
        "missing evidence count"
    );
}

#[test]
fn concise_record_includes_proposal_text() {
    let artifact = build_review_artifact(make_request(), &make_handoff()).unwrap();
    let record = artifact.concise_record();
    assert!(record.contains("proposals=["), "missing proposals bracket");
    // The artifact should have at least one proposal
    assert!(!artifact.proposals.is_empty());
    assert!(
        record.contains(&artifact.proposals[0].title),
        "missing first proposal title"
    );
}

// 5. render_review_text contains header, proposals section, measurement notes
#[test]
fn render_review_text_has_sections() {
    let artifact = build_review_artifact(make_request(), &make_handoff()).unwrap();
    let text = render_review_text(&artifact);
    assert!(text.contains("Review:"), "missing Review header");
    assert!(text.contains("Target: test-session"), "missing target");
    assert!(text.contains("Identity: test-identity"), "missing identity");
    assert!(text.contains("Proposals:"), "missing Proposals section");
    assert!(
        text.contains("Measurement notes:"),
        "missing Measurement notes section"
    );
    assert!(text.contains("latency=100ms"), "missing measurement note");
}

#[test]
fn render_review_text_shows_target_kind() {
    let artifact = build_review_artifact(make_request(), &make_handoff()).unwrap();
    let text = render_review_text(&artifact);
    assert!(text.contains("Target kind: session"), "missing target kind");
}

// 6. persist + load roundtrip
#[test]
fn persist_and_load_roundtrip() {
    let dir = TempDir::new().unwrap();
    let artifact = build_review_artifact(make_request(), &make_handoff()).unwrap();

    let path = persist_review_artifact(dir.path(), &artifact).unwrap();
    assert!(path.exists());
    assert!(path.extension().unwrap() == "json");

    let loaded = load_review_artifact(&path).unwrap();
    assert_eq!(artifact, loaded);
}

// 7. latest_review_artifact returns None on empty dir, newest when multiple exist
#[test]
fn latest_review_artifact_none_when_no_dir() {
    let dir = TempDir::new().unwrap();
    let result = latest_review_artifact(dir.path()).unwrap();
    assert!(result.is_none());
}

#[test]
fn latest_review_artifact_none_when_dir_empty() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(review_artifacts_dir(dir.path())).unwrap();
    let result = latest_review_artifact(dir.path()).unwrap();
    assert!(result.is_none());
}

#[test]
fn latest_review_artifact_returns_newest() {
    let dir = TempDir::new().unwrap();
    let older = make_artifact_direct("older-review", 1000);
    let newer = make_artifact_direct("newer-review", 2000);

    persist_review_artifact(dir.path(), &older).unwrap();
    persist_review_artifact(dir.path(), &newer).unwrap();

    let (path, latest) = latest_review_artifact(dir.path()).unwrap().unwrap();
    assert_eq!(latest.review_id, "newer-review");
    assert_eq!(latest.reviewed_at_unix_ms, 2000);
    assert!(path.to_string_lossy().contains("newer-review"));
}

// 8. ReviewTargetKind as_str shows "session" / "benchmark"
#[test]
fn target_kind_as_str_values() {
    assert_eq!(ReviewTargetKind::Session.as_str(), "session");
    assert_eq!(ReviewTargetKind::Benchmark.as_str(), "benchmark");
}

#[test]
fn target_kind_serde_roundtrip() {
    let session_json = serde_json::to_string(&ReviewTargetKind::Session).unwrap();
    assert_eq!(session_json, "\"session\"");
    let benchmark_json = serde_json::to_string(&ReviewTargetKind::Benchmark).unwrap();
    assert_eq!(benchmark_json, "\"benchmark\"");

    let parsed: ReviewTargetKind = serde_json::from_str("\"session\"").unwrap();
    assert_eq!(parsed, ReviewTargetKind::Session);
    let parsed: ReviewTargetKind = serde_json::from_str("\"benchmark\"").unwrap();
    assert_eq!(parsed, ReviewTargetKind::Benchmark);
}
