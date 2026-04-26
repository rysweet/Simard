use std::path::PathBuf;

use crate::CopilotSubmitAudit;

use super::read_view::*;
use crate::evidence::EvidenceSource;
use crate::session::{SessionId, SessionPhase};
use crate::{
    BaseTypeId, EvidenceRecord, OperatingMode, RuntimeAddress, RuntimeHandoffSnapshot,
    RuntimeNodeId, RuntimeState, RuntimeTopology,
};

fn make_evidence(detail: &str) -> EvidenceRecord {
    EvidenceRecord {
        id: "ev-test".to_string(),
        session_id: SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap(),
        phase: SessionPhase::Execution,
        detail: detail.to_string(),
        source: EvidenceSource::Runtime,
    }
}

fn make_session() -> crate::session::SessionRecord {
    crate::session::SessionRecord {
        id: SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap(),
        mode: OperatingMode::Engineer,
        objective: "objective-metadata(chars=42, words=8, lines=2)".to_string(),
        phase: SessionPhase::Complete,
        selected_base_type: BaseTypeId::from("terminal-shell"),
        evidence_ids: vec![],
        memory_keys: vec![],
    }
}

fn required_evidence() -> Vec<EvidenceRecord> {
    vec![
        make_evidence("backend-implementation=test-adapter"),
        make_evidence("shell=/bin/bash"),
        make_evidence("terminal-working-directory=/home/user/project"),
        make_evidence("terminal-command-count=5"),
        make_evidence("terminal-transcript-preview=$ echo hello"),
    ]
}

fn make_handoff(
    session: Option<crate::session::SessionRecord>,
    evidence: Vec<EvidenceRecord>,
) -> RuntimeHandoffSnapshot {
    RuntimeHandoffSnapshot {
        exported_state: RuntimeState::Ready,
        identity_name: "simard-engineer".to_string(),
        selected_base_type: BaseTypeId::from("terminal-shell"),
        topology: RuntimeTopology::SingleProcess,
        source_runtime_node: RuntimeNodeId::new("test-node"),
        source_mailbox_address: RuntimeAddress::new("test-addr"),
        session,
        memory_records: vec![],
        evidence_records: evidence,
        copilot_submit_audit: None,
    }
}

#[test]
fn from_handoff_succeeds_with_required_fields() {
    let handoff = make_handoff(Some(make_session()), required_evidence());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state"),
        handoff,
        "test-handoff.json".to_string(),
        None,
    );
    assert!(view.is_ok(), "expected success, got: {:?}", view.err());
}

#[test]
fn from_handoff_extracts_identity_and_topology() {
    let handoff = make_handoff(Some(make_session()), required_evidence());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state"),
        handoff,
        "test-handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.identity, "simard-engineer");
    assert_eq!(view.selected_base_type, "terminal-shell");
    assert_eq!(view.topology, "single-process");
}

#[test]
fn from_handoff_fails_without_session() {
    let handoff = make_handoff(None, required_evidence());
    let result = TerminalReadView::from_handoff(
        PathBuf::from("/test/state"),
        handoff,
        "test-handoff.json".to_string(),
        None,
    );
    assert!(result.is_err(), "should fail without session");
}

#[test]
fn from_handoff_uses_default_wait_values() {
    let handoff = make_handoff(Some(make_session()), required_evidence());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state"),
        handoff,
        "test-handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.wait_count, "0");
    assert_eq!(view.wait_timeout_seconds, "5");
}

#[test]
fn from_handoff_captures_step_and_checkpoint_counts() {
    let mut evidence = required_evidence();
    evidence.push(make_evidence("terminal-step-1=run cargo check"));
    evidence.push(make_evidence("terminal-step-2=run cargo test"));
    evidence.push(make_evidence("terminal-checkpoint-1=tests pass"));
    let handoff = make_handoff(Some(make_session()), evidence);
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state"),
        handoff,
        "test-handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.step_count, 2);
    assert_eq!(view.steps.len(), 2);
    assert_eq!(view.checkpoints.len(), 1);
}

#[test]
fn from_handoff_captures_copilot_submit_audit() {
    let mut handoff = make_handoff(Some(make_session()), required_evidence());
    handoff.copilot_submit_audit = Some(CopilotSubmitAudit {
        flow_asset: "test-flow".to_string(),
        payload_id: "payload-1".to_string(),
        outcome: "success".to_string(),
        reason_code: Some("ok".to_string()),
        ordered_steps: vec!["step1".to_string()],
        observed_checkpoints: vec![],
        last_meaningful_output_line: None,
        transcript_preview: "preview".to_string(),
    });
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state"),
        handoff,
        "test-handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert!(view.copilot_submit_audit.is_some());
    assert_eq!(
        view.copilot_submit_audit.as_ref().unwrap().flow_asset,
        "test-flow"
    );
}

#[test]
fn from_handoff_missing_required_evidence_fails() {
    // Missing "backend-implementation=" evidence record
    let evidence = vec![
        make_evidence("shell=/bin/bash"),
        make_evidence("terminal-working-directory=/home/user/project"),
        make_evidence("terminal-command-count=5"),
        make_evidence("terminal-transcript-preview=$ echo hello"),
    ];
    let handoff = make_handoff(Some(make_session()), evidence);
    let result = TerminalReadView::from_handoff(
        PathBuf::from("/test/state"),
        handoff,
        "test-handoff.json".to_string(),
        None,
    );
    assert!(
        result.is_err(),
        "should fail without backend-implementation evidence"
    );
}

#[test]
fn from_handoff_preserves_continuity_source() {
    let handoff = make_handoff(Some(make_session()), required_evidence());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state"),
        handoff,
        "test-handoff.json".to_string(),
        Some("previous-session.json".to_string()),
    )
    .unwrap();
    assert_eq!(
        view.continuity_source.as_deref(),
        Some("previous-session.json")
    );
}

#[test]
fn from_handoff_none_continuity_source() {
    let handoff = make_handoff(Some(make_session()), required_evidence());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state"),
        handoff,
        "test-handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert!(view.continuity_source.is_none());
}

#[test]
fn from_handoff_counts_memory_and_evidence_records() {
    let evidence = required_evidence();
    let evidence_len = evidence.len();
    let handoff = make_handoff(Some(make_session()), evidence);
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state"),
        handoff,
        "test-handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.memory_record_count, 0);
    assert_eq!(view.evidence_record_count, evidence_len);
}

#[test]
fn from_handoff_no_steps_or_checkpoints() {
    let handoff = make_handoff(Some(make_session()), required_evidence());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state"),
        handoff,
        "test-handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.step_count, 0);
    assert!(view.steps.is_empty());
    assert!(view.checkpoints.is_empty());
}
