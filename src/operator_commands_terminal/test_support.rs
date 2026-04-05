use crate::evidence::EvidenceSource;
use crate::session::{SessionId, SessionPhase};
use crate::{
    BaseTypeId, EvidenceRecord, OperatingMode, RuntimeAddress, RuntimeHandoffSnapshot,
    RuntimeNodeId, RuntimeState, RuntimeTopology,
};

pub(super) fn make_evidence(detail: &str) -> EvidenceRecord {
    EvidenceRecord {
        id: "ev-test".to_string(),
        session_id: SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap(),
        phase: SessionPhase::Execution,
        detail: detail.to_string(),
        source: EvidenceSource::Runtime,
    }
}

pub(super) fn make_session_record() -> crate::session::SessionRecord {
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

pub(super) fn required_evidence_records() -> Vec<EvidenceRecord> {
    vec![
        make_evidence("backend-implementation=test-adapter"),
        make_evidence("shell=/bin/bash"),
        make_evidence("terminal-working-directory=/home/user/project"),
        make_evidence("terminal-command-count=5"),
        make_evidence("terminal-transcript-preview=$ echo hello"),
    ]
}

pub(super) fn make_handoff(
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
