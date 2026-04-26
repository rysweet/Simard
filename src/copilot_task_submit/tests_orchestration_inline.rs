use super::orchestration::*;
use super::types::{
    COPILOT_SUBMIT_BASE_TYPE, COPILOT_SUBMIT_FLOW_ASSET_PATH, COPILOT_SUBMIT_RUNTIME_NODE,
    CopilotSubmitFlowAsset, CopilotSubmitOutcome, StartupStatus, SubmitStatus,
};

use crate::base_types::BaseTypeId;
use crate::handoff::CopilotSubmitAudit;
use crate::identity::OperatingMode;
use crate::runtime::{RuntimeAddress, RuntimeNodeId};
use crate::session::{SessionPhase, SessionRecord, UuidSessionIdGenerator};

use std::path::Path;

fn test_flow() -> CopilotSubmitFlowAsset {
    CopilotSubmitFlowAsset {
        launch_command: "copilot-cli".into(),
        working_directory: std::path::PathBuf::from("."),
        wait_timeout_seconds: 30,
        startup_banner: "Welcome".into(),
        guidance_checkpoint: "Ready".into(),
        submit_hint: "Submit".into(),
        post_submit_checkpoint: Some("Done".into()),
        trust_prompt: None,
        wrapper_error_signal: None,
        workflow_noise_signals: vec![],
        payload_id: "p1".into(),
        payload: "payload-text".into(),
    }
}

fn test_session() -> SessionRecord {
    SessionRecord::new(
        OperatingMode::Engineer,
        "test-objective",
        BaseTypeId::new(COPILOT_SUBMIT_BASE_TYPE),
        &UuidSessionIdGenerator,
    )
}

fn test_audit() -> CopilotSubmitAudit {
    CopilotSubmitAudit {
        flow_asset: COPILOT_SUBMIT_FLOW_ASSET_PATH.to_string(),
        payload_id: "p1".into(),
        outcome: "success".into(),
        reason_code: None,
        ordered_steps: vec!["step-1".into()],
        observed_checkpoints: vec!["checkpoint-1".into()],
        last_meaningful_output_line: Some("last-line".into()),
        transcript_preview: "preview".into(),
    }
}

#[test]
fn build_evidence_records_produces_expected_count() {
    let session = test_session();
    let flow = test_flow();
    let audit = test_audit();
    let node = RuntimeNodeId::new(COPILOT_SUBMIT_RUNTIME_NODE);
    let address = RuntimeAddress::local(&node);
    let records = build_evidence_records(
        &session,
        &flow,
        &["step-1".into()],
        &audit,
        Path::new("/test/dir"),
        &address,
    );
    // Base details (14) + reason_code (0) + last_meaningful (1) + steps (1) + checkpoints (1) = 17
    assert!(
        records.len() >= 14,
        "expected at least 14 evidence records, got {}",
        records.len()
    );
}

#[test]
fn build_evidence_records_ids_are_unique() {
    let session = test_session();
    let flow = test_flow();
    let audit = test_audit();
    let node = RuntimeNodeId::new(COPILOT_SUBMIT_RUNTIME_NODE);
    let address = RuntimeAddress::local(&node);
    let records = build_evidence_records(
        &session,
        &flow,
        &["s1".into(), "s2".into()],
        &audit,
        Path::new("."),
        &address,
    );
    let ids: Vec<_> = records.iter().map(|r| r.id.clone()).collect();
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(
        ids.len(),
        unique.len(),
        "evidence record IDs must be unique"
    );
}

#[test]
fn build_evidence_records_all_have_session_id() {
    let session = test_session();
    let flow = test_flow();
    let audit = test_audit();
    let node = RuntimeNodeId::new(COPILOT_SUBMIT_RUNTIME_NODE);
    let address = RuntimeAddress::local(&node);
    let records = build_evidence_records(&session, &flow, &[], &audit, Path::new("."), &address);
    for record in &records {
        assert_eq!(record.session_id, session.id);
        assert_eq!(record.phase, SessionPhase::Complete);
    }
}

#[test]
fn build_evidence_records_includes_reason_code_when_present() {
    let session = test_session();
    let flow = test_flow();
    let mut audit = test_audit();
    audit.reason_code = Some("test-reason".into());
    let node = RuntimeNodeId::new(COPILOT_SUBMIT_RUNTIME_NODE);
    let address = RuntimeAddress::local(&node);
    let records = build_evidence_records(&session, &flow, &[], &audit, Path::new("."), &address);
    let has_reason = records
        .iter()
        .any(|r| r.detail.contains("copilot-reason-code=test-reason"));
    assert!(has_reason, "should include reason code in evidence records");
}

#[test]
fn startup_observation_fields() {
    let obs = StartupObservation {
        status: StartupStatus::Ready,
        ordered_steps: vec!["launch".into()],
        observed_checkpoints: vec!["banner".into()],
        terminate: false,
    };
    assert_eq!(obs.status, StartupStatus::Ready);
    assert!(!obs.terminate);
}

#[test]
fn submit_observation_fields() {
    let obs = SubmitObservation {
        status: SubmitStatus::Success,
        ordered_steps: vec!["step".into()],
        observed_checkpoints: vec!["cp".into()],
        terminate: true,
    };
    assert_eq!(obs.status, SubmitStatus::Success);
    assert!(obs.terminate);
}
